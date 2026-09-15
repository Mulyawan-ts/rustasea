//! `#[cacheable(...)]` parsing and cache-impl emission for `#[derive(Model)]`.
//!
//! Split out of [`crate::model`] to keep the derive within the file-size
//! standard. This module owns the container-level `#[cacheable]` grammar and the
//! `Model` overrides the derive emits (`cacheable`, `cache_ttl`) for ADOPT-019.
//!
//! The attribute uses the list form for options — `#[cacheable(ttl = "300")]` —
//! matching the sibling `#[sluggable(...)]` grammar: rustc requires a name-value
//! attribute's value to be a literal, and the list form keeps the value a valid
//! string literal the macro can read. A bare `#[cacheable]` caches forever.

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{DeriveInput, Token};

/// Resolved `#[cacheable(...)]` container configuration.
#[derive(Debug, Default)]
pub(crate) struct CacheableConfig {
    /// Whether the container attribute was present at all.
    pub(crate) enabled: bool,
    /// The cache TTL in seconds (`None` = forever).
    pub(crate) ttl: Option<u64>,
}

/// Parse the container-level `#[cacheable]` / `#[cacheable(ttl = "...")]`.
///
/// Grammar: bare `#[cacheable]` (forever) or a list carrying `ttl = "300"`. A
/// `ttl` of `"0"` is a spanned error (use the bare form for forever); an unknown
/// key, a non-string value, or a non-integer `ttl` is a spanned error. An absent
/// attribute leaves caching disabled.
pub(crate) fn parse_cacheable(input: &DeriveInput) -> syn::Result<CacheableConfig> {
    let mut config = CacheableConfig::default();
    for attr in &input.attrs {
        if !attr.path().is_ident("cacheable") {
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
                                "#[cacheable] values must be string literals",
                            ))
                        }
                    };
                    match key.as_str() {
                        "ttl" => {
                            let secs: u64 = value.parse().map_err(|_| {
                                syn::Error::new_spanned(
                                    &pair.value,
                                    "#[cacheable] `ttl` must be an integer number of seconds",
                                )
                            })?;
                            if secs == 0 {
                                return Err(syn::Error::new_spanned(
                                    &pair.value,
                                    "#[cacheable] `ttl` must be greater than zero; use bare \
                                     `#[cacheable]` to cache forever",
                                ));
                            }
                            config.ttl = Some(secs);
                        }
                        other => {
                            return Err(syn::Error::new_spanned(
                                &pair,
                                format!("#[cacheable] does not support `{other}`; expected `ttl`"),
                            ))
                        }
                    }
                }
            }
            syn::Meta::NameValue(name_value) => {
                return Err(syn::Error::new_spanned(
                    name_value,
                    "expected #[cacheable] or #[cacheable(ttl = \"300\")]",
                ));
            }
        }
    }
    Ok(config)
}

/// Build the `Model` cache overrides for a `#[cacheable]` model.
///
/// Emits `cacheable` returning `true` and `cache_ttl` returning
/// `Some(Duration::from_secs(n))` for a declared TTL, or `None` for forever.
pub(crate) fn build_cacheable_impl(config: &CacheableConfig) -> TokenStream {
    let ttl = match config.ttl {
        Some(secs) => quote! { Some(std::time::Duration::from_secs(#secs)) },
        None => quote! { None },
    };
    quote! {
        /// Whether query/model result caching is enabled for this model.
        fn cacheable() -> bool {
            true
        }

        /// The default TTL for this model's cached queries (`None` = forever).
        fn cache_ttl() -> Option<std::time::Duration> {
            #ttl
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a struct snippet into a `DeriveInput`.
    fn parse(source: &str) -> DeriveInput {
        syn::parse_str(source).expect("valid struct syntax")
    }

    /// A bare attribute enables caching with no TTL (forever).
    #[test]
    fn parse_bare_is_forever() {
        let input = parse("#[cacheable] struct User;");
        let config = parse_cacheable(&input).expect("bare attribute parses");
        assert!(config.enabled);
        assert_eq!(config.ttl, None);
    }

    /// A `ttl` list value is read as seconds.
    #[test]
    fn parse_ttl_is_read() {
        let input = parse(r#"#[cacheable(ttl = "300")] struct User;"#);
        let config = parse_cacheable(&input).expect("ttl attribute parses");
        assert!(config.enabled);
        assert_eq!(config.ttl, Some(300));
    }

    /// An absent attribute leaves caching disabled.
    #[test]
    fn parse_absent_is_disabled() {
        let input = parse("struct User;");
        let config = parse_cacheable(&input).expect("absent attribute parses");
        assert!(!config.enabled);
        assert_eq!(config.ttl, None);
    }

    /// A zero TTL is a spanned error.
    #[test]
    fn parse_zero_ttl_is_error() {
        let input = parse(r#"#[cacheable(ttl = "0")] struct User;"#);
        let error = parse_cacheable(&input).expect_err("zero ttl must fail");
        assert!(error.to_string().contains("greater than zero"), "{error}");
    }

    /// An unknown key is a spanned error.
    #[test]
    fn parse_unknown_key_is_error() {
        let input = parse(r#"#[cacheable(forever = "true")] struct User;"#);
        let error = parse_cacheable(&input).expect_err("unknown key must fail");
        assert!(error.to_string().contains("does not support"), "{error}");
    }

    /// A non-integer TTL is a spanned error.
    #[test]
    fn parse_non_integer_ttl_is_error() {
        let input = parse(r#"#[cacheable(ttl = "soon")] struct User;"#);
        let error = parse_cacheable(&input).expect_err("non-integer ttl must fail");
        assert!(error.to_string().contains("integer"), "{error}");
    }

    /// The emitted overrides report `true` and the TTL.
    #[test]
    fn build_impl_emits_overrides() {
        let config = CacheableConfig {
            enabled: true,
            ttl: Some(60),
        };
        let tokens = build_cacheable_impl(&config).to_string();
        assert!(tokens.contains("cacheable"), "{tokens}");
        assert!(tokens.contains("cache_ttl"), "{tokens}");
        assert!(tokens.contains("from_secs"), "{tokens}");
    }

    /// The emitted TTL is `None` for a forever model.
    #[test]
    fn build_impl_forever_emits_none() {
        let config = CacheableConfig {
            enabled: true,
            ttl: None,
        };
        let tokens = build_cacheable_impl(&config).to_string();
        assert!(tokens.contains("None"), "{tokens}");
        assert!(!tokens.contains("from_secs"), "{tokens}");
    }
}
