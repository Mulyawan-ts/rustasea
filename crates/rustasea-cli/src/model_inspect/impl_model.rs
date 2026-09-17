//! Hand-written `impl Model` extraction for `show:model` (GAP-029, FR-109).
//!
//! The `make:model` generator and the app scaffold emit models as a plain
//! struct plus a hand-written `impl Model for X` block rather than the
//! `#[derive(Model)]` macro (see `crates/rustasea-cli/src/generators/kinds/model.rs`
//! and `crates/rustasea-scaffold/src/templates/app_domain.rs`). This module
//! reads the metadata those blocks declare (`type_name`, `table_name`,
//! `uses_soft_deletes`, `uses_timestamps`) so the inspector reflects real
//! generated applications, not only macro-derived models.

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::{File, ImplItem, Item, Type};

use super::type_ends_with;

/// Hand-written overrides read from an `impl Model for X` block.
///
/// Each field is `None` when the model does not declare the corresponding
/// method, letting the caller fall back to the derive-style inference.
#[derive(Debug, Default)]
pub(super) struct ImplMeta {
    /// Literal returned by `fn type_name()`.
    pub type_name: Option<String>,
    /// Literal returned by `fn table_name()`.
    pub table_name: Option<String>,
    /// Literal returned by `fn uses_soft_deletes()`.
    pub uses_soft_deletes: Option<bool>,
    /// Literal returned by `fn uses_timestamps()`.
    pub uses_timestamps: Option<bool>,
}

/// The struct targeted by the first `impl Model for X` block, if any.
///
/// Used to pick the model struct when the file also declares helper structs
/// before it.
pub(super) fn model_impl_target(file: &File) -> Option<String> {
    for item in &file.items {
        let Item::Impl(imp) = item else {
            continue;
        };
        if !impls_model(&imp.trait_) {
            continue;
        }
        if let Type::Path(path) = &*imp.self_ty {
            if let Some(segment) = path.path.segments.last() {
                return Some(segment.ident.to_string());
            }
        }
    }
    None
}

/// Read the hand-written metadata for `model` from its `impl Model` block.
pub(super) fn parse_impl_model(file: &File, model: &str) -> ImplMeta {
    let mut meta = ImplMeta::default();
    for item in &file.items {
        let Item::Impl(imp) = item else {
            continue;
        };
        if !impls_model(&imp.trait_) || !type_ends_with(&imp.self_ty, model) {
            continue;
        }
        for impl_item in &imp.items {
            let ImplItem::Fn(function) = impl_item else {
                continue;
            };
            match function.sig.ident.to_string().as_str() {
                "type_name" if meta.type_name.is_none() => {
                    meta.type_name = first_string_literal(function.block.to_token_stream());
                }
                "table_name" if meta.table_name.is_none() => {
                    meta.table_name = first_string_literal(function.block.to_token_stream());
                }
                "uses_soft_deletes" if meta.uses_soft_deletes.is_none() => {
                    meta.uses_soft_deletes = first_bool_literal(function.block.to_token_stream());
                }
                "uses_timestamps" if meta.uses_timestamps.is_none() => {
                    meta.uses_timestamps = first_bool_literal(function.block.to_token_stream());
                }
                _ => {}
            }
        }
        break;
    }
    meta
}

/// Whether an `impl` trait reference names the ORM `Model` contract.
fn impls_model(trait_ref: &Option<(Option<syn::Token![!]>, syn::Path, syn::Token![for])>) -> bool {
    trait_ref.as_ref().is_some_and(|(_, path, _)| {
        path.segments
            .last()
            .is_some_and(|segment| segment.ident == "Model")
    })
}

/// The first string literal anywhere in a token stream (a `fn table_name` body).
fn first_string_literal(stream: TokenStream) -> Option<String> {
    for tree in stream {
        match tree {
            TokenTree::Literal(literal) => {
                if let Ok(value) = syn::parse_str::<syn::LitStr>(&literal.to_string()) {
                    return Some(value.value());
                }
            }
            TokenTree::Group(group) => {
                if let Some(value) = first_string_literal(group.stream()) {
                    return Some(value);
                }
            }
            _ => {}
        }
    }
    None
}

/// The first boolean literal anywhere in a token stream (a `fn uses_*` body).
fn first_bool_literal(stream: TokenStream) -> Option<bool> {
    for tree in stream {
        match tree {
            TokenTree::Ident(ident) if ident == "true" => return Some(true),
            TokenTree::Ident(ident) if ident == "false" => return Some(false),
            TokenTree::Group(group) => {
                if let Some(value) = first_bool_literal(group.stream()) {
                    return Some(value);
                }
            }
            _ => {}
        }
    }
    None
}
