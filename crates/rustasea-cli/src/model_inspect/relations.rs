//! Relation-token scanning for `show:model` (GAP-029, FR-109).
//!
//! The `relations()` body is read as a raw token stream rather than a typed
//! AST: the model file may reference relation constructors through a local
//! `use` alias, so this scanner matches the `Relation::<method>("name", ...)`
//! shape textually. It is best-effort by design, and only the first string
//! literal of each call (the relation name) is read.

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use super::RelationInfo;

/// Recursively scan a token stream for `Relation::<method>("name", ...)` calls.
pub(super) fn scan_tokens(stream: TokenStream, out: &mut Vec<RelationInfo>) {
    let mut iter = stream.into_iter().peekable();
    while let Some(tree) = iter.next() {
        match tree {
            TokenTree::Ident(ident) if ident == "Relation" => {
                let colon1 = iter.next();
                let colon2 = iter.next();
                let method = iter.next();
                let group = iter.next();
                if let (
                    Some(TokenTree::Punct(p1)),
                    Some(TokenTree::Punct(p2)),
                    Some(TokenTree::Ident(name)),
                    Some(TokenTree::Group(args)),
                ) = (colon1, colon2, method, group)
                {
                    if p1.as_char() == ':'
                        && p2.as_char() == ':'
                        && args.delimiter() == Delimiter::Parenthesis
                    {
                        if let Some(kind) = relation_kind(&name.to_string()) {
                            if let Some(relation) = first_string_literal(args.stream()) {
                                out.push(RelationInfo {
                                    name: relation,
                                    kind: kind.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            TokenTree::Group(group) => scan_tokens(group.stream(), out),
            _ => {}
        }
    }
}

/// The first string literal in a token stream (a relation's name argument).
fn first_string_literal(stream: TokenStream) -> Option<String> {
    match stream.into_iter().next() {
        Some(TokenTree::Literal(lit)) => syn::parse_str::<syn::LitStr>(&lit.to_string())
            .ok()
            .map(|s| s.value()),
        _ => None,
    }
}

/// Map a `Relation::<method>` constructor to its contract kind.
///
/// `*_json` and `*_composite` variants collapse onto the base kind so the
/// contract enum stays `HasMany`/`BelongsTo`/`HasOne`/`BelongsToMany`.
fn relation_kind(method: &str) -> Option<&'static str> {
    match method {
        "has_many" | "has_many_json" | "has_many_composite" => Some("HasMany"),
        "belongs_to" | "belongs_to_json" | "belongs_to_composite" => Some("BelongsTo"),
        "has_one" | "has_one_json" | "has_one_composite" => Some("HasOne"),
        "many_to_many"
        | "many_to_many_composite"
        | "belongs_to_many"
        | "belongs_to_many_json"
        | "belongs_to_many_composite" => Some("BelongsToMany"),
        _ => None,
    }
}
