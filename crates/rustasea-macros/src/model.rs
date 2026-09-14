//! `#[derive(Model)]` — ORM model contract expansion (M2).
//!
//! Emits a `rustasea_orm::model::Model` impl for a plain data struct holding
//! row fields. Column metadata (soft deletes, timestamps, the `id` primary
//! key, update tracking) is inferred from the struct fields, matching the
//! existing hand-written implementations in `rustasea_orm::factory::User`.
//!
//! Supported annotations (all optional):
//!
//! ```rust,ignore
//! #[derive(Model)]
//! #[model(table = "people")]        // default: snake_plural(TypeName)
//! #[model(soft_deletes = "none")]   // default: auto (field `deleted_at`)
//! #[model(timestamps = "none")]     // default: auto (fields created_at/updated_at)
//! struct User {
//!     id: Uuid,
//!     #[model(cast = "json")]       // built-in cast (boolean/integer/float/
//!     settings: Settings,           //  string/datetime/json/encrypted)
//!     #[model(cast_with = "MoneyCast")] // custom `CastsAttributes<T>` impl
//!     balance: Money,
//! }
//! ```
//!
//! Every cast is wired into `Model::casts`, which the ORM applies on hydration
//! and persistence. A cast on an `Option<T>` field is wrapped in
//! `NullableCast` so `NULL` maps to `None`.

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{Data, DeriveInput, Fields, Type};

use crate::model_helpers::{column_name, is_created_at, is_deleted_at, is_updated_at};

/// A resolved cast declaration for one model field.
struct FieldCast {
    /// Column name (snake_case).
    column: String,
    /// The declared cast — a built-in name or a custom type path.
    spec: CastSpec,
    /// The Rust type the cast converts to (`Option<T>` when nullable).
    target_ty: Type,
}

/// The cast target: a built-in name or a caller-supplied type path.
enum CastSpec {
    /// One of the built-in cast names.
    Builtin(String),
    /// A custom `CastsAttributes<T>` type path.
    Custom(syn::Path),
}

/// Resolve a built-in cast name to its `rustasea_orm` type path tokens.
///
/// Returns `None` for an unknown name so the caller can raise a spanned error.
fn builtin_cast_path(name: &str) -> Option<TokenStream> {
    let ident = match name {
        "boolean" | "bool" => "BooleanCast",
        "integer" | "int" => "IntegerCast",
        "float" | "double" => "FloatCast",
        "string" => "StringCast",
        "datetime" | "date" => "DateTimeCast",
        "json" | "array" | "object" => "JsonCast",
        "encrypted" => "EncryptedCast",
        _ => return None,
    };
    let ident = syn::Ident::new(ident, proc_macro2::Span::call_site());
    Some(quote! { rustasea_orm::casts::#ident })
}

/// Whether a field type is exactly `Option<...>` (the nullable-cast trigger).
fn is_option_type(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Option")
}

/// Parse the `#[model(...)]` cast declarations on a single field.
///
/// Supports `cast = "json"` (built-in) and `cast_with = "MyCast"` (custom). An
/// unknown built-in name or an unparsable custom path is a spanned error.
/// Returns the resolved spec plus the field's declared type (the cast target).
fn parse_field_cast(field: &syn::Field) -> syn::Result<Option<(CastSpec, Type)>> {
    let mut spec: Option<CastSpec> = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
        let pairs = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated,
        )?;
        for pair in pairs {
            let key = pair
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let value = match &pair.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => s.value(),
                _ => continue,
            };
            match key.as_str() {
                "cast" => {
                    if builtin_cast_path(&value).is_none() {
                        return Err(syn::Error::new_spanned(
                            &pair.value,
                            format!(
                                "unknown cast `{value}`; expected one of boolean, integer, \
                                 float, string, datetime, json, encrypted (use `cast_with` \
                                 for a custom cast)"
                            ),
                        ));
                    }
                    spec = Some(CastSpec::Builtin(value));
                }
                "cast_with" => {
                    let path: syn::Path = syn::parse_str(&value).map_err(|_| {
                        syn::Error::new_spanned(
                            &pair.value,
                            format!("`cast_with = \"{value}\"` is not a valid type path"),
                        )
                    })?;
                    spec = Some(CastSpec::Custom(path));
                }
                _ => {}
            }
        }
    }
    Ok(spec.map(|spec| (spec, field.ty.clone())))
}

/// Parse the `#[model(...)]` container metadata.
fn parse_container(input: &DeriveInput) -> (Option<String>, bool, bool) {
    let mut table: Option<String> = None;
    let mut soft_deletes = true;
    let mut timestamps = true;
    for attr in &input.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        if let syn::Meta::List(list) = &attr.meta {
            let pairs = match list.parse_args_with(
                syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated,
            ) {
                Ok(pairs) => pairs,
                Err(_) => continue,
            };
            for pair in pairs {
                let key = pair
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                let value = match &pair.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) => s.value(),
                    _ => continue,
                };
                match key.as_str() {
                    "table" => table = Some(value),
                    "soft_deletes" => soft_deletes = value != "none",
                    "timestamps" => timestamps = value != "none",
                    _ => {}
                }
            }
        }
    }
    (table, soft_deletes, timestamps)
}

/// Build the `Model` impl for `#[derive(Model)]`.
pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let (table, soft_deletes, timestamps) = parse_container(input);

    // Reject non-struct targets early with a spanned error.
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => named,
            _ => {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[derive(Model)] requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "#[derive(Model)] can only be applied to structs",
            ));
        }
    };

    let mut has_id = false;
    let mut has_created = false;
    let mut has_updated = false;
    let mut has_deleted = false;
    let mut id_field = syn::Ident::new("id", proc_macro2::Span::call_site());
    let mut touch_fields: Vec<TokenStream> = Vec::new();
    let mut casts: Vec<FieldCast> = Vec::new();

    for field in &fields.named {
        let ident = match &field.ident {
            Some(ident) => ident.clone(),
            None => continue,
        };
        let col = column_name(&ident.to_string());
        match col.as_str() {
            "id" if field.ty.to_token_stream().to_string().contains("Uuid") => {
                has_id = true;
                id_field = ident.clone();
            }
            _ => {}
        }
        if is_created_at(&col) {
            has_created = true;
        }
        if is_updated_at(&col) {
            has_updated = true;
            touch_fields.push(quote! { self.#ident = chrono::Utc::now(); });
        }
        if is_deleted_at(&col) {
            has_deleted = true;
        }
        if let Some((spec, target_ty)) = parse_field_cast(field)? {
            casts.push(FieldCast {
                column: col,
                spec,
                target_ty,
            });
        }
    }

    if !has_id {
        return Err(syn::Error::new_spanned(
            input,
            "#[derive(Model)] requires an `id: uuid::Uuid` primary key field",
        ));
    }

    let soft = soft_deletes && has_deleted;
    let ts = timestamps && has_created && has_updated;
    let type_name = name.to_string();

    // An explicit `#[model(table = "...")]` wins; otherwise snake_plural.
    let table_expr = match table {
        Some(table) => quote! { #table.to_string() },
        None => {
            let table = crate::model_helpers::table_name(&type_name);
            quote! { #table.to_string() }
        }
    };

    let uses_soft = if soft {
        quote! { true }
    } else {
        quote! { false }
    };
    let uses_ts = if ts {
        quote! { true }
    } else {
        quote! { false }
    };
    let touch_body = if ts && !touch_fields.is_empty() {
        quote! { #(#touch_fields)* }
    } else {
        quote! {}
    };

    // Build one `CastBinding` per declared cast. The closure bridges the typed
    // `CastsAttributes` impl and the ORM's JSON pipeline, so model crates never
    // depend on `serde_json` directly.
    let cast_bindings: Vec<TokenStream> = casts
        .iter()
        .map(|cast| {
            let column = &cast.column;
            let target = &cast.target_ty;
            let base = match &cast.spec {
                CastSpec::Builtin(name) => builtin_cast_path(name).unwrap_or_else(|| {
                    quote! { compile_error!("unresolved built-in cast") }
                }),
                CastSpec::Custom(path) => quote! { #path },
            };
            let cast_expr = if is_option_type(target) {
                quote! { rustasea_orm::casts::NullableCast(#base) }
            } else {
                base
            };
            quote! {
                rustasea_orm::casts::CastBinding {
                    column: #column,
                    get: |key, value| {
                        let cast = #cast_expr;
                        let typed: #target =
                            rustasea_orm::casts::CastsAttributes::get(&cast, key, &value)?;
                        rustasea_orm::casts::serialize_to_json(key, &typed)
                    },
                    set: |key, value| {
                        let cast = #cast_expr;
                        let typed: #target =
                            rustasea_orm::casts::deserialize_from_json(key, value)?;
                        rustasea_orm::casts::CastsAttributes::set(&cast, key, &typed)
                    },
                }
            }
        })
        .collect();

    let casts_impl = if cast_bindings.is_empty() {
        quote! {}
    } else {
        quote! {
            /// Per-column attribute casts declared via `#[model(cast)]`.
            fn casts() -> Vec<rustasea_orm::casts::CastBinding> {
                vec![#(#cast_bindings),*]
            }
        }
    };

    Ok(quote! {
        #[automatically_derived]
        impl rustasea_orm::model::Model for #name {
            /// Type name driving default table derivation.
            fn type_name() -> &'static str {
                #type_name
            }

            /// Table name (`snake_plural` convention, overridable via `#[model(table)]`).
            fn table_name() -> String {
                #table_expr
            }

            /// Whether this model soft-deletes via `deleted_at`.
            fn uses_soft_deletes() -> bool {
                #uses_soft
            }

            /// Whether this model maintains `created_at`/`updated_at`.
            fn uses_timestamps() -> bool {
                #uses_ts
            }

            /// Primary key value.
            fn primary_key(&self) -> uuid::Uuid {
                self.#id_field
            }

            /// Assign a fresh client-generated UUID (v7) before persistence.
            fn assign_id(&mut self) -> uuid::Uuid {
                self.#id_field = uuid::Uuid::now_v7();
                self.#id_field
            }

            /// Bump `updated_at` on the instance (in-memory, pre-save).
            fn touch(&mut self) {
                #touch_body
            }

            #casts_impl
        }
    })
}
