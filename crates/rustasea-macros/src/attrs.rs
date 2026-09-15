//! Shared parsing/emission helpers for the M5 declarative attribute bundle
//! (FS-M5-03, FR-506).
//!
//! Procedural attribute macros must be defined in the crate root, so every
//! `#[proc_macro_attribute]` entry point lives in `lib.rs`; this private
//! module holds the parsers and the source-preserving emitters they delegate
//! to. Each expansion re-emits the annotated item unchanged and appends a
//! doc-hidden helper const (`__RUSTASEA_*`) that the runtime discovers
//! reflectively — the consts are inert until a driver reads them.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{parse_macro_input, Item, ItemFn};

/// Extract the ident from an annotated struct/enum/union.
///
/// Returns `None` when the attribute is applied to an item without a type
/// name (fn, const, impl, mod), which all of these attributes reject.
pub(crate) fn item_ident(input: &Item) -> Option<syn::Ident> {
    match input {
        Item::Struct(s) => Some(s.ident.clone()),
        Item::Enum(e) => Some(e.ident.clone()),
        Item::Union(u) => Some(u.ident.clone()),
        _ => None,
    }
}

/// Build the fallback ident used when the target item type is unsupported.
///
/// All emitters still re-emit the item unchanged (source-preserving), so the
/// worst case is a misleading helper const name — never a code change.
pub(crate) fn fallback_ident() -> syn::Ident {
    syn::Ident::new("Type", proc_macro2::Span::call_site())
}

/// Parse `#[route]` metadata: method + path name-value pairs.
///
/// Grammar: `#[route(method = "GET", path = "/users")]` — `method` defaults
/// to `"GET"` when omitted so `#[route(path = "/users")]` still compiles.
/// Both values must be string literals; anything else is a compile error
/// pointing at the offending argument.
pub(crate) fn parse_route(
    tokens: proc_macro2::TokenStream,
) -> Result<(String, String), syn::Error> {
    if tokens.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[route] requires method and path, e.g. #[route(method = \"GET\", path = \"/users\")]",
        ));
    }
    let pairs = syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated
        .parse2(tokens)
        .map_err(|e| {
            syn::Error::new(
                e.span(),
                format!(
                    "#[route] expects `method = \"...\", path = \"...\"` name-value pairs; {e}"
                ),
            )
        })?;
    let mut method: Option<String> = None;
    let mut path: Option<String> = None;
    for pair in pairs {
        let ident = pair
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let lit = match &pair.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => s.value(),
            _ => {
                return Err(syn::Error::new_spanned(
                    &pair.value,
                    "#[route] values must be string literals, e.g. method = \"GET\"",
                ));
            }
        };
        match ident.as_str() {
            "method" => {
                if method.replace(lit.clone()).is_some() {
                    return Err(syn::Error::new_spanned(
                        &pair,
                        "#[route] declares `method` more than once",
                    ));
                }
            }
            "path" => {
                if path.replace(lit.clone()).is_some() {
                    return Err(syn::Error::new_spanned(
                        &pair,
                        "#[route] declares `path` more than once",
                    ));
                }
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &pair,
                    format!("#[route] does not support `{other}`; expected `method` and `path`"),
                ));
            }
        }
    }
    let path = path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[route] requires a `path`, e.g. #[route(method = \"GET\", path = \"/users\")]",
        )
    })?;
    Ok((method.unwrap_or_else(|| "GET".to_string()), path))
}

/// Parse a positive integer literal from the attribute token stream.
///
/// Accepts a bare literal (`#[tries(3)]`). Trailing comma tokens are
/// tolerated so `#[tries(3,)]` does not break the macro.
pub(crate) fn parse_usize_literal(
    tokens: proc_macro2::TokenStream,
    name: &str,
) -> Result<usize, syn::Error> {
    let mut value: Option<usize> = None;
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Literal(lit) => {
                let text = lit.to_string();
                let parsed: usize = text.parse().map_err(|_| {
                    syn::Error::new_spanned(
                        &lit,
                        format!("#[{name}] expects a positive integer literal"),
                    )
                })?;
                if value.replace(parsed).is_some() {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        format!("#[{name}] expects a single integer literal"),
                    ));
                }
            }
            proc_macro2::TokenTree::Punct(p) if p.as_char() == ',' => {
                // tolerate trailing commas
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    format!("#[{name}] expects a single integer literal"),
                ));
            }
        }
    }
    value.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("#[{name}] requires an integer literal, e.g. #[{name}(3)]"),
        )
    })
}

/// Parse a single string literal argument.
///
/// `#[usage("app:send {user}")]` — a lone literal, optionally wrapped in an
/// extra parenthesis group; surrounding quotes are stripped.
pub(crate) fn parse_string_literal(
    tokens: proc_macro2::TokenStream,
    attr: &str,
) -> Result<String, syn::Error> {
    let mut collected: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    if matches!(collected.first(), Some(proc_macro2::TokenTree::Group(_))) {
        if let proc_macro2::TokenTree::Group(group) = collected.remove(0) {
            collected = group.stream().into_iter().collect();
        }
    }
    let text = collected
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join("");
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    if text.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("#[{attr}] requires a non-empty string literal"),
        ));
    }
    Ok(text.to_string())
}

/// Build a `__RUSTASEA_*` helper const identifier.
fn const_ident(key: &str, suffix: Option<&syn::Ident>) -> syn::Ident {
    let suffix = suffix.map(|i| format!("_{i}")).unwrap_or_default();
    syn::Ident::new(
        &format!("__RUSTASEA_{key}{suffix}"),
        proc_macro2::Span::call_site(),
    )
}

/// Parse a comma-separated list of string literals.
///
/// Grammar: `#[middleware("auth:jwt", "throttle:60,1")]` — every argument
/// must be a string literal; a trailing comma is tolerated. Used by the
/// `#[middleware]` handler attribute (FS-M3-06).
pub(crate) fn parse_string_list(
    tokens: proc_macro2::TokenStream,
    attr: &str,
) -> Result<Vec<String>, syn::Error> {
    let items = syn::punctuated::Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated
        .parse2(tokens)
        .map_err(|e| {
            syn::Error::new(
                e.span(),
                format!(
                    "#[{attr}] expects a comma-separated list of string literals, e.g. #[{attr}(\"a\", \"b\")]; {e}"
                ),
            )
        })?;
    let values = items.iter().map(syn::LitStr::value).collect::<Vec<_>>();
    if values.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("#[{attr}] requires at least one string literal"),
        ));
    }
    Ok(values)
}

/// Parse `#[authorize]` metadata: ability string plus optional target.
///
/// Grammar: `#[authorize("update")]`, `#[authorize("update", User)]`, or
/// `#[authorize("update", &post)]` — the ability must be a string literal; the
/// optional second argument names the target. A type path (`User`) is recorded
/// verbatim; a reference to the request-bound resource (`&post`) is unwrapped
/// and only its final path segment (`post`) is recorded as the resource id. A
/// trailing comma is tolerated. Returns `(ability, resource_id)` where the
/// resource id is an empty string when the target is omitted (FS-M3-06).
pub(crate) fn parse_authorize(
    tokens: proc_macro2::TokenStream,
) -> Result<(String, String), syn::Error> {
    let args =
        syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated.parse2(tokens)
            .map_err(|e| {
                syn::Error::new(
                    e.span(),
                    format!(
                        "#[authorize] expects an ability string plus an optional target, e.g. #[authorize(\"update\", &post)]; {e}"
                    ),
                )
            })?;
    let mut ability: Option<String> = None;
    let mut target = String::new();
    for (idx, arg) in args.iter().enumerate() {
        if idx == 0 {
            match arg {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => ability = Some(s.value()),
                _ => {
                    return Err(syn::Error::new_spanned(
                        arg,
                        "#[authorize] first argument must be an ability string, e.g. \"update\"",
                    ));
                }
            }
        } else {
            if !target.is_empty() {
                return Err(syn::Error::new_spanned(
                    arg,
                    "#[authorize] accepts at most one ability string and one target",
                ));
            }
            target = resource_id_from_target(arg);
        }
    }
    let ability = ability.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[authorize] requires an ability string, e.g. #[authorize(\"update\")]",
        )
    })?;
    Ok((ability, target))
}

/// Expand the `#[authorize]` attribute into metadata consts.
///
/// Source-preserving: the handler is re-emitted unchanged and one or two
/// doc-hidden consts are appended. The resource form (`#[authorize("update",
/// &post)]`) emits `__RUSTASEA_AUTHORIZE_<Fn>: (&str, &str)` recording the
/// ability and the resource id. The ability-only form (`#[authorize("users.edit")]`)
/// additionally emits `__RUSTASEA_AUTHORIZE_ABILITY_<Fn>: &str` recording the
/// bare ability name, so the router can enforce it through a Gate permission
/// check instead of a record-level resource. The legacy `(ability, "")` tuple
/// is still emitted for the ability-only form, so existing
/// `authorize_meta(__RUSTASEA_AUTHORIZE_<Fn>)` call sites keep compiling
/// (FS-M3-06, TC-M3-02).
pub(crate) fn expand_authorize(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let ident = input.sig.ident.clone();
    let tuple_name = syn::Ident::new(
        &format!("__RUSTASEA_AUTHORIZE_{ident}"),
        proc_macro2::Span::call_site(),
    );
    match parse_authorize(attr.into()) {
        Ok((ability, target)) => {
            if target.is_empty() {
                let ability_name = syn::Ident::new(
                    &format!("__RUSTASEA_AUTHORIZE_ABILITY_{ident}"),
                    proc_macro2::Span::call_site(),
                );
                quote! {
                    #input

                    #[doc(hidden)]
                    #[allow(non_upper_case_globals)]
                    pub const #tuple_name: (&str, &str) = (#ability, "");

                    #[doc(hidden)]
                    #[allow(non_upper_case_globals)]
                    pub const #ability_name: &str = #ability;
                }
                .into()
            } else {
                quote! {
                    #input

                    #[doc(hidden)]
                    #[allow(non_upper_case_globals)]
                    pub const #tuple_name: (&str, &str) = (#ability, #target);
                }
                .into()
            }
        }
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive the recorded resource id from an `#[authorize]` target expression.
///
/// `&post` (and `&&post`) unwrap to `post`; a bare path `User` yields `User`;
/// any other expression falls back to its source tokens so the id stays stable
/// and inspectable.
fn resource_id_from_target(expr: &syn::Expr) -> String {
    let inner = match expr {
        syn::Expr::Reference(reference) => &reference.expr,
        other => other,
    };
    if let syn::Expr::Path(path) = inner {
        if let Some(segment) = path.path.segments.last() {
            return segment.ident.to_string();
        }
    }
    let mut rendered = String::new();
    for token in quote::ToTokens::to_token_stream(inner) {
        rendered.push_str(&token.to_string());
    }
    rendered
}

/// Emit a usize-typed helper const for the annotated item.
///
/// Grammar: `#[tries(3)]`, `#[backoff(10)]`, `#[timeout(30)]` — appends
/// `pub const __RUSTASEA_{KEY}_{Type}: usize` after the unchanged item. The
/// const is `pub` so the queue runtime can bind it to a job's retry policy
/// (`register_job_with_policy`), mirroring the `#[authorize]` metadata surface.
pub(crate) fn usize_attr(
    attr: TokenStream,
    item: TokenStream,
    attribute: &str,
    key: &str,
) -> TokenStream {
    let input = parse_macro_input!(item as Item);
    let ident = item_ident(&input).unwrap_or_else(fallback_ident);
    match parse_usize_literal(attr.into(), attribute) {
        Ok(value) => {
            let const_name = const_ident(key, Some(&ident));
            quote! {
                #input
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                pub const #const_name: usize = #value;
            }
            .into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

/// Emit a bare boolean marker const for the annotated item.
///
/// Grammar: `#[hidden]`, `#[failOnTimeout]`, `#[withoutBroadcasting]`,
/// `#[repairToolCalls]` — appends `const __RUSTASEA_{KEY}: bool = true`.
pub(crate) fn marker_attr(item: TokenStream, key: &str) -> TokenStream {
    let input = parse_macro_input!(item as Item);
    let const_name = const_ident(key, None);
    quote! {
        #input
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #const_name: bool = true;
    }
    .into()
}

/// Emit a `&'static str`-typed helper const for the annotated item.
///
/// Grammar: `#[usage("...")]`, `#[help("...")]`, `#[queue("...")]`,
/// `#[connection("...")]` — appends `const __RUSTASEA_{KEY}_{Type}: &str`.
pub(crate) fn string_attr(
    attr: TokenStream,
    item: TokenStream,
    attribute: &str,
    key: &str,
) -> TokenStream {
    let input = parse_macro_input!(item as Item);
    let ident = item_ident(&input).unwrap_or_else(fallback_ident);
    match parse_string_literal(attr.into(), attribute) {
        Ok(text) => {
            let const_name = const_ident(key, Some(&ident));
            quote! {
                #input
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #const_name: &str = #text;
            }
            .into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}
