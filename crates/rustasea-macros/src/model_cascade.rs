//! `#[cascade_soft_deletes("posts", ...)]` parsing and cascade-impl emission.
//!
//! Split out of [`crate::model`] to keep the derive within the file-size
//! standard. This module owns the container-level `cascade_soft_deletes`
//! attribute grammar and the `Model` overrides the derive emits
//! (`cascade_soft_deletes`, `cascade_relations`).
//!
//! The attribute uses the list form `#[cascade_soft_deletes("posts")]` rather
//! than `#[cascade_soft_deletes = ["posts"]]`: rustc requires a name-value
//! attribute's value to be a literal, and an array literal is rejected with
//! "attribute value must be a literal" before the derive ever runs. The list
//! form carries the same string entries with a valid grammar.
//!
//! Relation names are validated at *runtime* by the cascade engine, not at
//! derive time: [`Model::relations`](rustasea_orm::model::Model::relations) is a
//! hand-written method, so the macro cannot see the declared names. An unknown
//! name surfaces as a typed `OrmError::InvalidState` on the first cascade.

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{DeriveInput, LitStr, Token};

/// Resolved `#[cascade_soft_deletes(...)]` container configuration.
#[derive(Debug, Default)]
pub(crate) struct CascadeConfig {
    /// Whether the container attribute was present at all.
    pub(crate) enabled: bool,
    /// The declared relation names to cascade to.
    pub(crate) relation_names: Vec<String>,
}

/// Parse the container-level `#[cascade_soft_deletes("posts", ...)]`.
///
/// Grammar: a parenthesised list of string literals. An absent attribute leaves
/// cascading disabled; an empty list, a name-value form, or a non-string entry
/// is a spanned error.
pub(crate) fn parse_cascade(input: &DeriveInput) -> syn::Result<CascadeConfig> {
    let mut config = CascadeConfig::default();
    for attr in &input.attrs {
        if !attr.path().is_ident("cascade_soft_deletes") {
            continue;
        }
        config.enabled = true;
        let syn::Meta::List(list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "expected #[cascade_soft_deletes(\"posts\", ...)]",
            ));
        };
        let names = list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
        for name in names {
            config.relation_names.push(name.value());
        }
        if config.relation_names.is_empty() {
            return Err(syn::Error::new_spanned(
                &list.tokens,
                "`cascade_soft_deletes` must declare at least one relation",
            ));
        }
    }
    Ok(config)
}

/// Build the `Model` cascade overrides for a `#[cascade_soft_deletes]` model.
///
/// Emits `cascade_soft_deletes` returning `true` and `cascade_relations` with
/// the declared names as a `&'static [&'static str]`.
pub(crate) fn build_cascade_impl(config: &CascadeConfig) -> TokenStream {
    let names = config.relation_names.iter().map(|name| {
        let name = syn::LitStr::new(name, proc_macro2::Span::call_site());
        quote! { #name }
    });
    quote! {
        /// Whether this model cascades soft deletes to its declared relations.
        fn cascade_soft_deletes() -> bool {
            true
        }

        /// The relation names cascaded with this model's delete/restore.
        fn cascade_relations() -> &'static [&'static str] {
            &[#(#names),*]
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

    /// The full list form records every relation name and enables cascading.
    #[test]
    fn parse_full_cascade_reads_names() {
        let input = parse(r#"#[cascade_soft_deletes("posts", "comments")] struct User;"#);
        let config = parse_cascade(&input).expect("list attribute parses");
        assert!(config.enabled);
        assert_eq!(config.relation_names, vec!["posts", "comments"]);
    }

    /// An absent attribute leaves cascading disabled.
    #[test]
    fn parse_absent_cascade_is_disabled() {
        let input = parse("struct User;");
        let config = parse_cascade(&input).expect("absent attribute parses");
        assert!(!config.enabled);
        assert!(config.relation_names.is_empty());
    }

    /// A single-name list is accepted.
    #[test]
    fn parse_single_name_is_accepted() {
        let input = parse(r#"#[cascade_soft_deletes("posts")] struct User;"#);
        let config = parse_cascade(&input).expect("single-name list parses");
        assert_eq!(config.relation_names, vec!["posts"]);
    }

    /// An empty list is a spanned error.
    #[test]
    fn parse_empty_list_is_error() {
        let input = parse("#[cascade_soft_deletes()] struct User;");
        let error = parse_cascade(&input).expect_err("empty list must fail");
        assert!(error.to_string().contains("at least one"), "{error}");
    }

    /// A non-string entry is a spanned error.
    #[test]
    fn parse_non_string_element_is_error() {
        let input = parse("#[cascade_soft_deletes(3)] struct User;");
        let error = parse_cascade(&input).expect_err("non-string entry must fail");
        assert!(!error.to_string().is_empty(), "{error}");
    }

    /// A name-value form is a spanned error pointing at the list grammar.
    #[test]
    fn parse_name_value_is_error() {
        let input = parse(r#"#[cascade_soft_deletes = "posts"] struct User;"#);
        let error = parse_cascade(&input).expect_err("name-value form must fail");
        assert!(error.to_string().contains("expected"), "{error}");
    }

    /// The emitted override reports `true` and the declared names.
    #[test]
    fn build_impl_emits_override() {
        let config = CascadeConfig {
            enabled: true,
            relation_names: vec!["posts".to_string()],
        };
        let tokens = build_cascade_impl(&config).to_string();
        assert!(tokens.contains("cascade_soft_deletes"), "{tokens}");
        assert!(tokens.contains("cascade_relations"), "{tokens}");
        assert!(tokens.contains("\"posts\""), "{tokens}");
    }
}
