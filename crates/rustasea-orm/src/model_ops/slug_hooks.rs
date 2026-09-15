//! Slug-generation hooks for the write path — compute a unique slug before
//! insert/update (ADOPT-016).
//!
//! Split out of [`super`] to keep the write path within the file-size standard.
//! Every helper is `pub(crate)` and gated on the model opting in
//! ([`Model::sluggable`]) **and** the model exposing source values, so a model
//! that does not opt in pays no serialization or query cost.
//!
//! The pipeline is: read [`Model::slug_options`], build the base slug from
//! [`Model::slug_source_values`], then — when uniqueness is enabled — search for
//! a free candidate (`base`, `base-2`, `base-3`, …) through
//! [`Model::query_with_trashed`]. Trashed rows are included so a soft-deleted
//! row's slug is never reused. An empty base slug is a typed
//! [`OrmError::Slug`] error rather than a silent empty column.

use crate::db::DbPool;
use crate::error::{OrmError, Result};
use crate::model::Model;
use crate::model_ops::ModelOps;
use crate::sluggable::SlugOptions;
use crate::types::Value;
use serde::de::DeserializeOwned;
use uuid::Uuid;

/// Upper bound on the deterministic uniqueness search (`base`, `base-2`, …).
///
/// A model whose slug space is exhausted this far surfaces a typed
/// [`OrmError::Slug`] rather than looping forever.
const MAX_SLUG_ATTEMPTS: u32 = 1000;

/// Look up a model by its slug — the query half of implicit route binding.
///
/// Blanket-implemented for every [`Model`] that can be hydrated, so a route
/// handler can resolve `{post:slug}` with `Post::find_by_slug(&pool, slug)`.
/// The query uses [`Model::query`], so soft-deleted rows are excluded — the
/// correct behaviour for a route key.
#[allow(async_fn_in_trait)]
pub trait SluggableFind: ModelOps + Sized {
    /// The row whose slug column equals `slug`, if any (trashed rows excluded).
    async fn find_by_slug(pool: &DbPool, slug: &str) -> Result<Option<Self>>
    where
        Self: DeserializeOwned,
    {
        let column = Self::slug_options().slug_column;
        match Self::query().where_eq(&column, slug).first(pool).await? {
            Some(row) => Ok(Some(crate::casts::hydrate::<Self>(row)?)),
            None => Ok(None),
        }
    }

    /// Like [`SluggableFind::find_by_slug`] but returns [`OrmError::NotFound`].
    async fn find_by_slug_or_fail(pool: &DbPool, slug: &str) -> Result<Self>
    where
        Self: DeserializeOwned,
    {
        Self::find_by_slug(pool, slug)
            .await?
            .ok_or(OrmError::NotFound)
    }
}

impl<T: Model + DeserializeOwned> SluggableFind for T {}

/// Generate and assign a slug before an insert.
///
/// No-op for a model that has not opted into [`Model::sluggable`]. Otherwise the
/// base slug is derived from [`Model::slug_source_values`] and made unique
/// against the whole table (including trashed rows) before being stored via
/// [`Model::set_slug`].
///
/// # Errors
///
/// Returns [`OrmError::Slug`] when the source columns produce no slug text, or
/// when the uniqueness search exhausts [`MAX_SLUG_ATTEMPTS`].
pub(crate) async fn prepare_insert<T: Model>(pool: &DbPool, data: &mut T) -> Result<()> {
    if !T::sluggable() {
        return Ok(());
    }
    let options = T::slug_options();
    let base = base_slug::<T>(&options, data)?;
    let slug = unique_slug::<T>(pool, &options, &base, None).await?;
    data.set_slug(&slug);
    Ok(())
}

/// Generate and assign a slug before an update when the source changed.
///
/// No-op for a model that has not opted in, when
/// [`on_update`](SlugOptions::on_update) is false, or when the source values are
/// unchanged versus `old_source_values`. `old_source_values` is the pre-read
/// value of [`Model::slug_source_values`] for the persisted row (the caller
/// reads it once before mutating); passing `None` forces regeneration.
///
/// # Errors
///
/// Returns [`OrmError::Slug`] when the source columns produce no slug text, or
/// when the uniqueness search exhausts [`MAX_SLUG_ATTEMPTS`].
pub(crate) async fn prepare_update<T: Model>(
    pool: &DbPool,
    data: &mut T,
    old_source_values: Option<&[(String, String)]>,
) -> Result<()> {
    if !T::sluggable() {
        return Ok(());
    }
    let options = T::slug_options();
    if !options.on_update {
        return Ok(());
    }
    let current = data.slug_source_values();
    if let Some(old) = old_source_values {
        if old == current.as_slice() {
            return Ok(());
        }
    }
    let base = base_slug::<T>(&options, data)?;
    let own = Some(data.primary_key());
    let slug = unique_slug::<T>(pool, &options, &base, own).await?;
    data.set_slug(&slug);
    Ok(())
}

/// Derive the base slug from the model's source values.
///
/// An empty result is a typed error: a silent empty slug would leave the row
/// unreachable by slug and violate the column's intent.
fn base_slug<T: Model>(options: &SlugOptions, data: &T) -> Result<String> {
    let base = options.generate(&data.slug_source_values());
    if base.is_empty() {
        let column = options
            .source
            .first()
            .cloned()
            .unwrap_or_else(|| options.slug_column.clone());
        return Err(OrmError::Slug(format!(
            "slug source column `{column}` produced an empty slug"
        )));
    }
    Ok(base)
}

/// Resolve a unique slug, suffixing `-2`, `-3`, … when a candidate is taken.
///
/// When [`unique`](SlugOptions::unique) is false the base is returned verbatim.
/// Otherwise each candidate is probed against the model table (including trashed
/// rows). `own` is the primary key of the row being updated: the probe excludes
/// that row, so an update that keeps its slug is stable (it does not collide
/// with itself).
async fn unique_slug<T: Model>(
    pool: &DbPool,
    options: &SlugOptions,
    base: &str,
    own: Option<Uuid>,
) -> Result<String> {
    if !options.unique {
        return Ok(base.to_string());
    }
    for attempt in 1..=MAX_SLUG_ATTEMPTS {
        let candidate = candidate_for(base, attempt, options.separator);
        let mut query = T::query_with_trashed().where_eq(&options.slug_column, candidate.as_str());
        if let Some(id) = own {
            query = query.where_op("id", "!=", Value::Uuid(id))?;
        }
        if query.first(pool).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(OrmError::Slug(format!(
        "exhausted {MAX_SLUG_ATTEMPTS} slug candidates for `{base}`"
    )))
}

/// The candidate slug for `attempt` (1 = base, 2 = `base-2`, …).
fn candidate_for(base: &str, attempt: u32, separator: char) -> String {
    if attempt <= 1 {
        base.to_string()
    } else {
        format!("{base}{separator}{attempt}")
    }
}
