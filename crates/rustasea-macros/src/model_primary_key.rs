//! Composite primary-key parsing and emission for `#[derive(Model)]`.
//!
//! Split out of [`crate::model`] to keep the derive within the file-size
//! standard. This module owns the `#[model(primary_key = ["a", "b"])]` container
//! grammar and the `primary_key` / `assign_id` / `primary_key_columns` /
//! `primary_key_values` emissions for a composite-key model. A model with no
//! `primary_key` declaration keeps the single `id: Uuid` behaviour, so the
//! emissions here are only reached when the attribute is present.

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{DeriveInput, FieldsNamed, Type};

use crate::model_helpers::column_name;

/// Parse `#[model(primary_key = ["tenant_id", "user_id"])]` from the container.
///
/// Returns the declared column list (empty when the attribute is absent). Every
/// element must be a string literal; a non-string element is a spanned error.
pub(crate) fn parse_primary_key(input: &DeriveInput) -> syn::Result<Vec<String>> {
    let mut columns: Vec<String> = Vec::new();
    for attr in &input.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
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
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default();
            if key != "primary_key" {
                continue;
            }
            let syn::Expr::Array(array) = &pair.value else {
                return Err(syn::Error::new_spanned(
                    &pair.value,
                    "`primary_key` must be an array of column strings, \
                     e.g. #[model(primary_key = [\"tenant_id\", \"user_id\"])]",
                ));
            };
            for element in &array.elems {
                match element {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(text),
                        ..
                    }) => columns.push(text.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`primary_key` entries must be string literals",
                        ))
                    }
                }
            }
            if columns.is_empty() {
                return Err(syn::Error::new_spanned(
                    &pair.value,
                    "`primary_key` must declare at least one column",
                ));
            }
        }
    }
    Ok(columns)
}

/// A declared primary-key column resolved to its struct field.
struct KeyField {
    /// The struct field identifier.
    ident: syn::Ident,
    /// The field's declared Rust type.
    ty: Type,
}

/// Resolve each declared column to its struct field, erroring when missing.
fn resolve_key_fields(fields: &FieldsNamed, columns: &[String]) -> syn::Result<Vec<KeyField>> {
    let mut resolved: Vec<KeyField> = Vec::with_capacity(columns.len());
    for column in columns {
        let field = fields.named.iter().find(|field| {
            field
                .ident
                .as_ref()
                .is_some_and(|ident| &column_name(&ident.to_string()) == column)
        });
        let Some(field) = field else {
            return Err(syn::Error::new_spanned(
                fields,
                format!("`primary_key` column `{column}` has no matching struct field"),
            ));
        };
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new_spanned(field, "primary-key fields must be named"))?;
        resolved.push(KeyField {
            ident,
            ty: field.ty.clone(),
        });
    }
    Ok(resolved)
}

/// Whether a field type is a `Uuid` (bound as `Value::Uuid`).
fn is_uuid(ty: &Type) -> bool {
    ty.to_token_stream().to_string().contains("Uuid")
}

/// Whether a field type is an integer (bound as `Value::Int`).
fn is_integer(ty: &Type) -> bool {
    let text = ty.to_token_stream().to_string();
    [
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    ]
    .iter()
    .any(|int| text == *int || text.ends_with(&format!("::{int}")))
}

/// The `Value` expression for one primary-key field.
fn value_expr(field: &KeyField) -> TokenStream {
    let ident = &field.ident;
    if is_uuid(&field.ty) {
        quote! { rustasea_orm::Value::Uuid(self.#ident) }
    } else if is_integer(&field.ty) {
        quote! { rustasea_orm::Value::Int(self.#ident as i64) }
    } else {
        quote! { rustasea_orm::Value::Text(self.#ident.clone().to_string()) }
    }
}

/// Build the composite primary-key method emissions for `#[derive(Model)]`.
///
/// Emits `primary_key_columns`, `primary_key_values`, `primary_key` (first
/// column when it is a `Uuid`, else `Uuid::nil()`) and `assign_id` (a no-op
/// returning `Uuid::nil()` — a composite key has no single generated id).
pub(crate) fn build_composite_primary_key(
    fields: &FieldsNamed,
    columns: &[String],
) -> syn::Result<TokenStream> {
    let resolved = resolve_key_fields(fields, columns)?;
    let column_literals = columns.iter().map(|column| quote! { #column });
    let value_exprs = resolved.iter().map(value_expr);

    let first = &resolved[0];
    let first_ident = &first.ident;
    let primary_key_body = if is_uuid(&first.ty) {
        quote! { self.#first_ident }
    } else {
        quote! { uuid::Uuid::nil() }
    };

    Ok(quote! {
        /// The declared primary-key column names.
        fn primary_key_columns() -> &'static [&'static str] {
            &[#(#column_literals),*]
        }

        /// The primary-key values in `primary_key_columns` order.
        fn primary_key_values(&self) -> Vec<rustasea_orm::Value> {
            vec![#(#value_exprs),*]
        }

        /// The first primary-key column when it is a `Uuid`, else `Uuid::nil()`.
        fn primary_key(&self) -> uuid::Uuid {
            #primary_key_body
        }

        /// A composite key has no single generated id, so this is a no-op.
        fn assign_id(&mut self) -> uuid::Uuid {
            uuid::Uuid::nil()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the container grammar extracts a column list.
    #[test]
    fn parses_primary_key_columns() {
        let input: DeriveInput = syn::parse_quote! {
            #[model(primary_key = ["tenant_id", "user_id"])]
            struct Membership { tenant_id: Uuid, user_id: Uuid }
        };
        let columns = parse_primary_key(&input).unwrap();
        assert_eq!(
            columns,
            vec!["tenant_id".to_string(), "user_id".to_string()]
        );
    }

    /// Verifies an absent attribute yields no columns.
    #[test]
    fn absent_attribute_yields_empty() {
        let input: DeriveInput = syn::parse_quote! {
            struct User { id: Uuid }
        };
        assert!(parse_primary_key(&input).unwrap().is_empty());
    }

    /// Verifies integer and uuid type detection drive the bind expression.
    #[test]
    fn detects_key_field_types() {
        let uuid: Type = syn::parse_quote! { uuid::Uuid };
        let int: Type = syn::parse_quote! { i64 };
        let text: Type = syn::parse_quote! { String };
        assert!(is_uuid(&uuid));
        assert!(is_integer(&int));
        assert!(!is_integer(&text));
        assert!(!is_uuid(&text));
    }
}
