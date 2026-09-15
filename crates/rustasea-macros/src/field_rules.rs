//! Field-level `#[validate(...)]` rule collection for `#[validate]`.
//!
//! Split out of [`crate`] to keep the crate root within the file-size standard.
//! Parses the per-field rule grammar and returns a copy of the struct fields
//! with the consumed `#[validate]` attributes stripped, so rustc never sees an
//! attribute macro left on a field.

/// Collect per-field rule declarations and return a field-clean copy.
///
/// Supported field grammar, both Laravel-pipeline and single-rule forms:
///
/// ```rust,ignore
/// #[validate("required|min:3")]  // pipe-separated rule list
/// #[validate(contains_strict = "admin")] // single name-value rule
/// ```
///
/// On success returns `(rules, fields_without_validate_attrs)`. Any
/// `#[validate]`-shaped field attribute that does not parse — including a
/// bare `#[validate]` with no rules — is a hard compile error: a validator
/// with no rules silently passes every payload.
pub(crate) fn field_rule_specs(
    fields: &syn::Fields,
) -> Result<(Vec<(String, String)>, syn::Fields), syn::Error> {
    let mut emitted = fields.clone();
    let mut specs: Vec<(String, String)> = Vec::new();

    for field in emitted.iter_mut() {
        let field_name = match &field.ident {
            Some(ident) => ident.to_string(),
            None => {
                return Err(syn::Error::new_spanned(
                    field,
                    "#[validate] supports only named struct fields",
                ));
            }
        };
        let mut rules: Vec<String> = Vec::new();
        let mut has_validate_attr = false;
        for attr in &field.attrs {
            if !attr.path().is_ident("validate") {
                continue;
            }
            has_validate_attr = true;
            match &attr.meta {
                syn::Meta::List(list) => {
                    // A single literal string carries a pipe-separated list.
                    match list.parse_args::<syn::LitStr>() {
                        Ok(lit) => {
                            let spec = lit.value();
                            if spec.trim().is_empty() {
                                return Err(syn::Error::new_spanned(
                                    list,
                                    "field #[validate(\"...\")] rule list must not be empty",
                                ));
                            }
                            rules.push(spec);
                        }
                        Err(_) => {
                            // Fall back to name-value form: rule = value.
                            let name_values =
                                list
                                    .parse_args_with(
                                        syn::punctuated::Punctuated::<
                                            syn::MetaNameValue,
                                            syn::Token![,],
                                        >::parse_terminated,
                                    )
                                    .map_err(|e| {
                                        syn::Error::new_spanned(
                                            list,
                                            format!(
                                                "field #[validate] rule must be a string \
                                             (\"required|min:3\") or name=value \
                                             (contains_strict=\"admin\"); {e}"
                                            ),
                                        )
                                    })?;
                            for nv in name_values {
                                let value = match &nv.value {
                                    syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
                                        syn::Lit::Str(s) => s.value(),
                                        _ => {
                                            return Err(syn::Error::new_spanned(
                                                &nv.value,
                                                "field #[validate] rule values must be strings",
                                            ));
                                        }
                                    },
                                    _ => {
                                        return Err(syn::Error::new_spanned(
                                            &nv.value,
                                            "field #[validate] rule values must be strings",
                                        ));
                                    }
                                };
                                let rule_name = nv
                                    .path
                                    .segments
                                    .iter()
                                    .map(|s| s.ident.to_string())
                                    .collect::<Vec<_>>()
                                    .join("::");
                                rules.push(format!("{rule_name}:{value}"));
                            }
                        }
                    }
                }
                syn::Meta::Path(_) => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        format!(
                            "field `{field_name}` has bare #[validate] with no rules; \
                             declare rules as #[validate(\"rule|rule\")]"
                        ),
                    ));
                }
                syn::Meta::NameValue(_) => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        format!(
                            "field `{field_name}` uses unsupported #[validate = \"...\"]; \
                             declare rules as #[validate(\"rule|rule\")]"
                        ),
                    ));
                }
            }
        }
        if has_validate_attr {
            // Consumed — remove from the emitted struct so rustc never sees
            // an attribute macro on a field.
            field.attrs.retain(|a| !a.path().is_ident("validate"));
        }
        if !rules.is_empty() {
            specs.push((field_name, rules.join("|")));
        }
    }

    if specs.is_empty() {
        return Err(syn::Error::new_spanned(
            fields,
            "#[validate] requires at least one field-level rule declaration; \
             a validator with no rules would silently pass every payload",
        ));
    }
    Ok((specs, emitted))
}
