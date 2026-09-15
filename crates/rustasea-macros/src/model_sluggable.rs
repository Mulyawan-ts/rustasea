//! `#[sluggable(...)]` parsing and slug-impl emission for `#[derive(Model)]`.
//!
//! Split out of [`crate::model`] to keep both modules within the file-size
//! standard. This module owns the container-level `#[sluggable(...)]` grammar
//! and the `Model` slug overrides the derive emits (`sluggable`,
//! `slug_options`, `set_slug`, `slug_source_values`).

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::punctuated::Punctuated;
use syn::{DeriveInput, FieldsNamed, Token};

/// Resolved `#[sluggable(...)]` container configuration.
#[derive(Debug, Default)]
pub(crate) struct SluggableConfig {
    /// Whether the container attribute was present at all.
    pub(crate) enabled: bool,
    /// Source columns (`source = "title,subtitle"`).
    pub(crate) source: Vec<String>,
    /// Destination column (`to = "slug"`); defaults to a field named `slug`.
    pub(crate) slug_column: Option<String>,
    /// Word separator (`separator = "-"`).
    pub(crate) separator: Option<char>,
    /// Collision suffixing (`unique = "true"`).
    pub(crate) unique: Option<bool>,
    /// Regeneration on update (`on_update = "true"`).
    pub(crate) on_update: Option<bool>,
    /// Maximum slug length (`max_len = "80"`).
    pub(crate) max_len: Option<usize>,
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

/// Parse the container-level `#[sluggable(...)]` attribute.
///
/// Grammar: bare `#[sluggable]` (derives from a field named `slug`), or a list
/// carrying `source = "a,b"`, `to = "slug"`, `separator = "-"`,
/// `unique = "true"`, `on_update = "true"`, and/or `max_len = "80"`. Unknown
/// keys and non-string literals are spanned errors.
pub(crate) fn parse_sluggable(input: &DeriveInput) -> syn::Result<SluggableConfig> {
    let mut config = SluggableConfig::default();
    for attr in &input.attrs {
        if !attr.path().is_ident("sluggable") {
            continue;
        }
        config.enabled = true;
        match &attr.meta {
            syn::Meta::Path(_) => {}
            syn::Meta::List(list) => {
                let pairs = list.parse_args_with(
                    Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated,
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
                                "#[sluggable] values must be string literals",
                            ))
                        }
                    };
                    match key.as_str() {
                        "source" => config.source = split_columns(&value),
                        "to" => config.slug_column = Some(value),
                        "separator" => config.separator = value.chars().next(),
                        "unique" => config.unique = Some(value == "true"),
                        "on_update" => config.on_update = Some(value == "true"),
                        "max_len" => {
                            config.max_len = Some(value.parse().map_err(|_| {
                                syn::Error::new_spanned(
                                    &pair.value,
                                    "#[sluggable] `max_len` must be an integer",
                                )
                            })?)
                        }
                        other => {
                            return Err(syn::Error::new_spanned(
                                &pair,
                                format!(
                                    "#[sluggable] does not support `{other}`; expected \
                                     `source`, `to`, `separator`, `unique`, `on_update`, or \
                                     `max_len`"
                                ),
                            ))
                        }
                    }
                }
            }
            syn::Meta::NameValue(_) => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[sluggable] takes a list, e.g. #[sluggable(source = \"title\")]",
                ))
            }
        }
    }
    Ok(config)
}

/// Resolve the slug field: the field named by `to`, else a field named `slug`.
///
/// A `#[sluggable]` model without either is a spanned error — the derive has no
/// field to write the generated slug into.
pub(crate) fn find_slug_field(
    fields: &FieldsNamed,
    config: &SluggableConfig,
) -> syn::Result<syn::Ident> {
    let target = config.slug_column.as_deref().unwrap_or("slug");
    for field in &fields.named {
        if let Some(ident) = &field.ident {
            if ident == target {
                return Ok(ident.clone());
            }
        }
    }
    Err(syn::Error::new_spanned(
        fields,
        format!("#[sluggable] model needs a field named `{target}` or a `to = \"...\"` selector"),
    ))
}

/// Build the `Model` slug overrides for a `#[sluggable(...)]` model.
///
/// Emits `sluggable`, `slug_options`, `set_slug`, and `slug_source_values`.
/// Source fields are read with `.to_string()`; an `Option<_>` source field is
/// unwrapped to its default so a missing value contributes an empty string.
pub(crate) fn build_slug_impl(
    config: &SluggableConfig,
    slug_field: &syn::Ident,
    fields: &FieldsNamed,
) -> TokenStream {
    let source_literals = config.source.iter().map(|column| {
        let ident = syn::Ident::new(column, proc_macro2::Span::call_site());
        let is_option = field_is_option(fields, column);
        let read = if is_option {
            quote! { self.#ident.clone().unwrap_or_default() }
        } else {
            quote! { self.#ident.to_string() }
        };
        quote! { (#column.to_string(), #read) }
    });
    // String-literal tokens for the builder's `from(&[&str])` call.
    let source_names: Vec<&String> = config.source.iter().collect();
    let slug_column = config
        .slug_column
        .clone()
        .unwrap_or_else(|| "slug".to_string());
    let separator = config.separator.unwrap_or('-');
    let unique = config.unique.unwrap_or(true);
    let on_update = config.on_update.unwrap_or(false);
    let max_len = config.max_len.unwrap_or(255);

    quote! {
        /// Whether this model derives a URL-safe slug on write.
        fn sluggable() -> bool {
            true
        }

        /// The slug-generation configuration declared via `#[sluggable(...)]`.
        fn slug_options() -> rustasea_orm::sluggable::SlugOptions {
            rustasea_orm::sluggable::SlugOptions::default()
                .from(&[#(#source_names),*])
                .to(#slug_column)
                .separator(#separator)
                .unique(#unique)
                .on_update(#on_update)
                .max_len(#max_len)
        }

        /// Store a generated slug on the model's slug field.
        fn set_slug(&mut self, slug: &str) {
            self.#slug_field = slug.to_string();
        }

        /// The current values of the configured slug source columns.
        fn slug_source_values(&self) -> Vec<(String, String)> {
            vec![#(#source_literals),*]
        }
    }
}

/// Whether `column` names a field whose declared type contains `Option`.
fn field_is_option(fields: &FieldsNamed, column: &str) -> bool {
    fields
        .named
        .iter()
        .find(|field| field.ident.as_ref().is_some_and(|ident| ident == column))
        .map(|field| field.ty.to_token_stream().to_string().contains("Option"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::DeriveInput;

    /// Parse a struct snippet into a `DeriveInput`.
    fn parse(source: &str) -> DeriveInput {
        syn::parse_str(source).expect("valid struct syntax")
    }

    /// The named-field set of a parsed struct, for `find_slug_field`.
    fn fields_of(input: &DeriveInput) -> &FieldsNamed {
        match &input.data {
            syn::Data::Struct(data) => match &data.fields {
                syn::Fields::Named(named) => named,
                _ => panic!("expected named fields"),
            },
            _ => panic!("expected a struct"),
        }
    }

    /// Bare `#[sluggable]` opts in with all-optional overrides unset.
    #[test]
    fn parse_bare_sluggable_enables_defaults() {
        let input = parse("#[sluggable] struct Post { slug: String }");
        let config = parse_sluggable(&input).expect("bare attribute parses");
        assert!(config.enabled);
        assert!(config.source.is_empty());
        assert_eq!(config.slug_column, None);
        assert_eq!(config.separator, None);
        assert_eq!(config.unique, None);
        assert_eq!(config.on_update, None);
        assert_eq!(config.max_len, None);
    }

    /// The full list form populates every override.
    #[test]
    fn parse_full_sluggable_reads_every_key() {
        let input = parse(
            r#"#[sluggable(source = "title, subtitle", to = "permalink",
                separator = "_", unique = "false", on_update = "true", max_len = "80")]
               struct Post { permalink: String }"#,
        );
        let config = parse_sluggable(&input).expect("list attribute parses");
        assert!(config.enabled);
        assert_eq!(config.source, vec!["title", "subtitle"]);
        assert_eq!(config.slug_column.as_deref(), Some("permalink"));
        assert_eq!(config.separator, Some('_'));
        assert_eq!(config.unique, Some(false));
        assert_eq!(config.on_update, Some(true));
        assert_eq!(config.max_len, Some(80));
    }

    /// An unknown key is a spanned parse error naming the supported keys.
    #[test]
    fn parse_unknown_key_is_error() {
        let input = parse(r#"#[sluggable(bogus = "x")] struct Post { slug: String }"#);
        let error = parse_sluggable(&input).expect_err("unknown key must fail");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    /// A non-string literal value is rejected.
    #[test]
    fn parse_non_string_value_is_error() {
        let input = parse("#[sluggable(source = 3)] struct Post { slug: String }");
        let error = parse_sluggable(&input).expect_err("non-string value must fail");
        assert!(error.to_string().contains("string literals"), "{error}");
    }

    /// `max_len` must parse as an integer.
    #[test]
    fn parse_bad_max_len_is_error() {
        let input = parse(r#"#[sluggable(max_len = "big")] struct Post { slug: String }"#);
        let error = parse_sluggable(&input).expect_err("non-integer max_len must fail");
        assert!(error.to_string().contains("integer"), "{error}");
    }

    /// Without `to`, the field literally named `slug` is targeted.
    #[test]
    fn find_slug_field_defaults_to_slug() {
        let input =
            parse("#[sluggable(source = \"name\")] struct Post { name: String, slug: String }");
        let config = parse_sluggable(&input).expect("parses");
        let field = find_slug_field(fields_of(&input), &config).expect("slug field found");
        assert_eq!(field.to_string(), "slug");
    }

    /// `to = "..."` redirects the target field.
    #[test]
    fn find_slug_field_honors_to_selector() {
        let input = parse(
            r#"#[sluggable(to = "permalink")] struct Post { slug: String, permalink: String }"#,
        );
        let config = parse_sluggable(&input).expect("parses");
        let field = find_slug_field(fields_of(&input), &config).expect("target field found");
        assert_eq!(field.to_string(), "permalink");
    }

    /// A model with no `slug` field and no `to` is a spanned error.
    #[test]
    fn find_slug_field_missing_is_error() {
        let input = parse("#[sluggable(source = \"name\")] struct Post { name: String }");
        let config = parse_sluggable(&input).expect("parses");
        let error =
            find_slug_field(fields_of(&input), &config).expect_err("missing slug field must fail");
        assert!(error.to_string().contains("slug"), "{error}");
    }
}
