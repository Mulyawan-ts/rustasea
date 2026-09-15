//! `#[logs_activity(...)]` parsing and column-policy emission for `#[derive(Model)]`.
//!
//! Split out of [`crate::model`] to keep both modules within the file-size
//! standard. This module owns the container-level activity opt-in grammar, the
//! field-level `#[logs_activity(skip)]` grammar, and the `ActivityColumns`
//! expression the derive emits.

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

/// Resolved `#[logs_activity(...)]` container configuration.
#[derive(Default)]
pub(crate) struct LogsActivityConfig {
    /// Whether the container attribute was present at all.
    pub(crate) enabled: bool,
    /// Columns to log exclusively (`only = "a,b"`).
    pub(crate) only: Vec<String>,
    /// Columns to exclude (`except = "password"`).
    pub(crate) except: Vec<String>,
    /// Whether a no-op update should write no row (`only_dirty = "true"`).
    pub(crate) only_dirty: bool,
}

/// Split a comma-separated column list, trimming each entry.
fn split_columns(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse the container-level `#[logs_activity(...)]` attribute.
///
/// Grammar: bare `#[logs_activity]`, or a list carrying `only = "a,b"`,
/// `except = "a,b"`, and/or `only_dirty = "true"`. `only` and `except` are
/// mutually exclusive; an unknown key is a spanned error.
pub(crate) fn parse_logs_activity(input: &DeriveInput) -> syn::Result<LogsActivityConfig> {
    let mut config = LogsActivityConfig::default();
    for attr in &input.attrs {
        if !attr.path().is_ident("logs_activity") {
            continue;
        }
        config.enabled = true;
        match &attr.meta {
            syn::Meta::Path(_) => {}
            syn::Meta::List(list) => {
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
                        _ => {
                            return Err(syn::Error::new_spanned(
                                &pair.value,
                                "#[logs_activity] values must be string literals",
                            ))
                        }
                    };
                    match key.as_str() {
                        "only" => config.only = split_columns(&value),
                        "except" => config.except = split_columns(&value),
                        "only_dirty" => config.only_dirty = value == "true",
                        other => {
                            return Err(syn::Error::new_spanned(
                                &pair,
                                format!(
                                    "#[logs_activity] does not support `{other}`; expected \
                                     `only`, `except`, or `only_dirty`"
                                ),
                            ))
                        }
                    }
                }
            }
            syn::Meta::NameValue(_) => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[logs_activity] takes a list, e.g. #[logs_activity(except = \"password\")]",
                ))
            }
        }
    }
    if !config.only.is_empty() && !config.except.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "#[logs_activity] cannot combine `only` and `except`",
        ));
    }
    Ok(config)
}

/// Whether a field carries `#[logs_activity(skip)]`, validating the grammar.
pub(crate) fn field_skips_activity(field: &syn::Field) -> syn::Result<bool> {
    let mut skip = false;
    for attr in &field.attrs {
        if !attr.path().is_ident("logs_activity") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "field #[logs_activity] must be #[logs_activity(skip)]",
            ));
        };
        let path = list.parse_args::<syn::Path>()?;
        if path.is_ident("skip") {
            skip = true;
        } else {
            return Err(syn::Error::new_spanned(
                &path,
                "field #[logs_activity] only supports `skip`",
            ));
        }
    }
    Ok(skip)
}

/// Build the `ActivityColumns` expression for a `#[logs_activity(...)]` model.
///
/// Field-level `#[logs_activity(skip)]` columns are folded into the exclusion
/// set; with neither `only` nor `except` declared and no skipped fields the
/// policy is `All`, otherwise the effective set is `Only` (explicit `only`,
/// minus skips) or `Except` (explicit `except`, plus skips).
pub(crate) fn build_activity_columns(
    activity: &LogsActivityConfig,
    skipped: &[String],
) -> TokenStream {
    if !activity.only.is_empty() {
        let mut columns = activity.only.clone();
        columns.retain(|column| !skipped.contains(column));
        let columns = columns.iter().map(|column| quote! { #column.to_string() });
        return quote! {
            rustasea_orm::activity::ActivityColumns::only(vec![#(#columns),*])
        };
    }

    let mut excluded = activity.except.clone();
    for column in skipped {
        if !excluded.contains(column) {
            excluded.push(column.clone());
        }
    }
    if excluded.is_empty() {
        return quote! { rustasea_orm::activity::ActivityColumns::all() };
    }
    let columns = excluded.iter().map(|column| quote! { #column.to_string() });
    quote! {
        rustasea_orm::activity::ActivityColumns::except(vec![#(#columns),*])
    }
}
