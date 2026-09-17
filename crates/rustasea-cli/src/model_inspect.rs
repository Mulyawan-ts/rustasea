//! Source-level model introspection for `show:model` (GAP-029, FR-109).
//!
//! `show:model` reads the generated `app/models/{snake}.rs` and parses it with
//! `syn` instead of running the ORM: a console process has no live database
//! handle, so the inspector reports the model's *declared* metadata: the Rust
//! struct name, its attributes (name/type/nullable), declared casts, the
//! soft-delete and timestamps flags, and a best-effort list of relations read
//! from the `relations()` body.
//!
//! The table name and the soft-delete/timestamps inference mirror the
//! `#[derive(Model)]` expansion exactly (`crates/rustasea-macros/src/model.rs`,
//! `model_helpers.rs`): an explicit `#[model(table = "...")]` wins, otherwise
//! the `snake_plural(type_name)` convention applies, and the tracked flags are
//! only true when the corresponding column fields are present.
//!
//! The emitted JSON matches the `model-inspector` contract
//! (`.agents/documents/application/testing/contracts/model-inspector.schema.json`).

use std::collections::BTreeMap;
use std::path::Path;

use quote::ToTokens;
use serde::Serialize;
use syn::punctuated::Punctuated;
use syn::{Fields, File, Item, Meta, Token, Type};

use rustasea_orm::naming::to_snake_case;
use rustasea_orm::snake_plural;

use crate::generator::Generator;

mod impl_model;
mod relations;

/// One struct field as a model attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttributeInfo {
    /// Column name (`snake_case`, mirroring the derive's column mapping).
    pub name: String,
    /// Declared Rust type, normalized (`Option<DateTime<Utc>>`).
    #[serde(rename = "type")]
    pub type_name: String,
    /// Whether the field type is `Option<...>`.
    pub nullable: bool,
}

/// One declared relation (`name` + contract `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelationInfo {
    /// Relation name used in `Model::with("...")`.
    pub name: String,
    /// Contract kind: `HasMany`, `BelongsTo`, `HasOne` or `BelongsToMany`.
    pub kind: String,
}

/// The introspected model surface emitted by `show:model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInspection {
    /// Struct / model name (`User`).
    pub model: String,
    /// Resolved table name (`users`).
    pub table: String,
    /// Declared attributes in source order.
    pub attributes: Vec<AttributeInfo>,
    /// Best-effort declared relations.
    pub relations: Vec<RelationInfo>,
    /// Field-to-cast map from `#[model(cast = "...")]` / `cast_with`.
    pub casts: BTreeMap<String, String>,
    /// Whether the model soft-deletes via `deleted_at`.
    pub soft_delete: bool,
    /// Whether the model tracks `created_at`/`updated_at`.
    pub timestamps: bool,
}

/// Failure modes for source-level model introspection.
#[derive(Debug, thiserror::Error)]
pub enum ModelInspectError {
    /// No `app/models/{snake}.rs` exists under the project root.
    #[error("model source not found: {path} (searched for `{name}`)")]
    FileNotFound {
        /// Absolute path that was searched.
        path: String,
        /// Requested model name.
        name: String,
    },

    /// The model file exists but could not be read.
    #[error("failed to read {path}: {source}")]
    Read {
        /// Absolute path that failed.
        path: String,
        /// Underlying io error.
        source: std::io::Error,
    },

    /// The file is not valid Rust (or not a parseable file).
    #[error("failed to parse model source: {message}")]
    Parse {
        /// Parser diagnostic.
        message: String,
    },

    /// No struct was found to introspect.
    #[error("no model struct found for `{name}`")]
    StructNotFound {
        /// Requested model name.
        name: String,
    },
}

/// Inspect a model source string.
///
/// Selects the model struct (the `#[derive(...Model...)]` target, else the
/// struct named by an `impl Model for X` block, else the first struct), then
/// derives its table, attributes, casts, tracked flags and best-effort
/// relations. `fallback_name` names the model in a
/// [`ModelInspectError::StructNotFound`].
///
/// Two declaration styles are recognised: the `#[derive(Model)]` macro (see
/// `crates/rustasea-macros/src/model.rs`) and the hand-written `impl Model`
/// emitted by `make:model` (`crates/rustasea-cli/src/generators/kinds/model.rs`)
/// and the app scaffold (`crates/rustasea-scaffold/src/templates/app_domain.rs`).
pub fn inspect_source(
    source: &str,
    fallback_name: &str,
) -> Result<ModelInspection, ModelInspectError> {
    let file: File = syn::parse_file(source).map_err(|error| ModelInspectError::Parse {
        message: error.to_string(),
    })?;

    let target = impl_model::model_impl_target(&file);
    let item = select_struct(&file, target.as_deref()).ok_or_else(|| {
        ModelInspectError::StructNotFound {
            name: fallback_name.to_string(),
        }
    })?;

    let model = item.ident.to_string();
    let derived = derives_model(&item.attrs);
    let impl_meta = impl_model::parse_impl_model(&file, &model);
    let (table_override, soft_deletes, timestamps_attr) = parse_container(&item.attrs);

    let mut attributes = Vec::new();
    let mut casts = BTreeMap::new();
    let mut has_created = false;
    let mut has_updated = false;
    let mut has_deleted = false;
    let mut has_timestamps_marker = false;
    let mut has_soft_delete_marker = false;

    if let Fields::Named(named) = &item.fields {
        for field in &named.named {
            let Some(ident) = &field.ident else {
                continue;
            };
            let column = to_snake_case(&ident.to_string());
            has_created |= column == "created_at";
            has_updated |= column == "updated_at";
            has_deleted |= column == "deleted_at";
            has_timestamps_marker |= type_ends_with(&field.ty, "Timestamps");
            has_soft_delete_marker |= type_ends_with(&field.ty, "SoftDeletes");
            attributes.push(AttributeInfo {
                name: column.clone(),
                type_name: normalize_type(&field.ty.to_token_stream().to_string()),
                nullable: is_option_type(&field.ty),
            });
            if let Some(cast) = field_cast(field) {
                casts.insert(column, cast);
            }
        }
    }

    let (table, soft_delete, timestamps) = if derived {
        // The derive reports a flag only when the container attribute permits it
        // *and* the corresponding columns are declared (`soft = soft_deletes &&
        // has_deleted`, `ts = timestamps && has_created && has_updated`).
        let table = table_override.unwrap_or_else(|| snake_plural(&model));
        (
            table,
            soft_deletes && has_deleted,
            timestamps_attr && has_created && has_updated,
        )
    } else {
        // A hand-written `impl Model` supplies the flags directly (its trait
        // defaults are `true`); fall back to the declared marker fields or the
        // `deleted_at` / `created_at` + `updated_at` columns when it does not.
        let type_name = impl_meta.type_name.clone().unwrap_or_else(|| model.clone());
        let table = table_override
            .or(impl_meta.table_name.clone())
            .unwrap_or_else(|| snake_plural(&type_name));
        let soft = impl_meta
            .uses_soft_deletes
            .unwrap_or(has_deleted || has_soft_delete_marker);
        let ts = impl_meta
            .uses_timestamps
            .unwrap_or((has_created && has_updated) || has_timestamps_marker);
        (table, soft, ts)
    };

    let relations = relations_from_file(&file, &model);

    Ok(ModelInspection {
        model,
        table,
        attributes,
        relations,
        casts,
        soft_delete,
        timestamps,
    })
}

/// Inspect `root/app/models/{snake(name)}.rs`.
///
/// Reports the searched path in [`ModelInspectError::FileNotFound`] so an
/// operator can see exactly which file was expected.
pub fn inspect_model(root: &Path, name: &str) -> Result<ModelInspection, ModelInspectError> {
    let snake = Generator::snake(name);
    let path = root.join(format!("app/models/{snake}.rs"));
    if !path.is_file() {
        return Err(ModelInspectError::FileNotFound {
            path: path.display().to_string(),
            name: name.to_string(),
        });
    }
    let source = std::fs::read_to_string(&path).map_err(|source| ModelInspectError::Read {
        path: path.display().to_string(),
        source,
    })?;
    inspect_source(&source, name)
}

/// Select the model struct.
///
/// Preference order: the `#[derive(...Model...)]` target, else the struct named
/// by an `impl Model for X` block (`target`), else the first struct. The derive
/// marker is authoritative because a file may carry unrelated `impl Model`
/// blocks for helper types.
fn select_struct<'a>(file: &'a File, target: Option<&str>) -> Option<&'a syn::ItemStruct> {
    let mut first: Option<&syn::ItemStruct> = None;
    let mut derived: Option<&syn::ItemStruct> = None;
    let mut targeted: Option<&syn::ItemStruct> = None;
    for item in &file.items {
        let Item::Struct(strukt) = item else {
            continue;
        };
        if first.is_none() {
            first = Some(strukt);
        }
        if derived.is_none() && derives_model(&strukt.attrs) {
            derived = Some(strukt);
        }
        if targeted.is_none() {
            if let Some(target) = target {
                if strukt.ident == target {
                    targeted = Some(strukt);
                }
            }
        }
    }
    derived.or(targeted).or(first)
}

/// Whether an attribute list carries `#[derive(...Model...)]`.
fn derives_model(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let Ok(paths) = attr.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
        else {
            return false;
        };
        paths
            .iter()
            .any(|path| path.segments.last().is_some_and(|s| s.ident == "Model"))
    })
}

/// Parse the container `#[model(...)]` metadata (table/soft_deletes/timestamps).
fn parse_container(attrs: &[syn::Attribute]) -> (Option<String>, bool, bool) {
    let mut table = None;
    let mut soft_deletes = true;
    let mut timestamps = true;
    for attr in attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let Ok(pairs) =
            list.parse_args_with(Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated)
        else {
            continue;
        };
        for pair in pairs {
            let key = last_segment(&pair.path);
            let Some(value) = string_value(&pair.value) else {
                continue;
            };
            match key.as_str() {
                "table" => table = Some(value),
                "soft_deletes" => soft_deletes = value != "none",
                "timestamps" => timestamps = value != "none",
                _ => {}
            }
        }
    }
    (table, soft_deletes, timestamps)
}

/// Read the `#[model(cast = "...")]` / `cast_with` value on a field.
fn field_cast(field: &syn::Field) -> Option<String> {
    let mut result = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let Ok(pairs) =
            list.parse_args_with(Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated)
        else {
            continue;
        };
        for pair in pairs {
            let key = last_segment(&pair.path);
            let Some(value) = string_value(&pair.value) else {
                continue;
            };
            if key == "cast" || key == "cast_with" {
                result = Some(value);
            }
        }
    }
    result
}

/// The last path segment of a `#[model(...)]` key (`soft_deletes`).
fn last_segment(path: &syn::Path) -> String {
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default()
}

/// The string literal value of a `key = "value"` pair, if any.
fn string_value(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        _ => None,
    }
}

/// Whether a type is exactly `Option<...>` (the nullable marker).
fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        path.path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option")
    } else {
        false
    }
}

/// Normalize a tokenized type by dropping spaces around punctuation only.
///
/// `Option < DateTime < Utc > >` becomes `Option<DateTime<Utc>>` while
/// identifier-separating spaces (`dyn Fn`) are preserved.
fn normalize_type(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let tight = |c: char| "<>,&()[]:;".contains(c);
    let mut out = String::with_capacity(raw.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let previous_tight = out.chars().last().is_some_and(tight);
            let next_tight = chars[index + 1..]
                .iter()
                .find(|&&c| c != ' ')
                .is_some_and(|&c| tight(c));
            if previous_tight || next_tight {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Collect relations from the model's `relations()` body (best effort).
fn relations_from_file(file: &File, model: &str) -> Vec<RelationInfo> {
    let mut fallback: Option<&syn::Block> = None;
    for item in &file.items {
        let Item::Impl(imp) = item else {
            continue;
        };
        let self_is_model = type_ends_with(&imp.self_ty, model);
        for impl_item in &imp.items {
            let syn::ImplItem::Fn(function) = impl_item else {
                continue;
            };
            if function.sig.ident != "relations" {
                continue;
            }
            if self_is_model {
                let mut out = Vec::new();
                relations::scan_tokens(function.block.to_token_stream(), &mut out);
                return out;
            }
            fallback = Some(&function.block);
        }
    }
    let mut out = Vec::new();
    if let Some(block) = fallback {
        relations::scan_tokens(block.to_token_stream(), &mut out);
    }
    out
}

/// Whether a type path ends in `name` (`impl Model for User` → `User`).
fn type_ends_with(ty: &Type, name: &str) -> bool {
    if let Type::Path(path) = ty {
        path.path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name)
    } else {
        false
    }
}

#[cfg(test)]
mod tests;
