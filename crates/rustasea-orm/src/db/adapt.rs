//! Placeholder adaptation shared by the pool and transaction execution paths.
//!
//! All emitters (`QueryBuilder`, `model_ops`, `m2`) produce `$n` positional
//! placeholders; SQLite and Postgres accept them natively, while MySQL requires
//! `?`. Rewriting here keeps the emitters dialect-agnostic and makes this the
//! single choke point for placeholder adaptation. Only SQL text is rewritten —
//! bind values are never inspected, so a literal `$` inside a bound string is
//! unaffected.

/// Translate `$n` placeholders in `sql` to the given driver's native shape.
///
/// All emitters (`QueryBuilder`, `model_ops`, `m2`) produce `$n` positional
/// placeholders; SQLite and Postgres accept them natively, while MySQL requires
/// `?`. Rewriting here keeps the emitters dialect-agnostic and makes this the
/// single choke point for placeholder adaptation. Only SQL text is rewritten —
/// bind values are never inspected, so a literal `$` inside a bound string is
/// unaffected.
pub(crate) fn adapt_placeholders<'a>(sql: &'a str, dialect: &str) -> std::borrow::Cow<'a, str> {
    #[cfg(feature = "mysql")]
    {
        if dialect == "mysql" {
            return rewrite_dollar_placeholders(sql);
        }
    }
    let _ = dialect;
    std::borrow::Cow::Borrowed(sql)
}

/// Rewrite `$n` references to `?`, renumbering sequentially.
///
/// MySQL's `?` placeholders are positional, not indexed, so non-contiguous `$n`
/// indices (e.g. `$1 … $3`) collapse to a sequential run of `?` in textual
/// order. `$` not followed by one or more digits is left untouched, which
/// preserves dollar signs appearing as literals in the SQL text.
#[cfg(feature = "mysql")]
pub(crate) fn rewrite_dollar_placeholders(sql: &str) -> std::borrow::Cow<'_, str> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            out.push('?');
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        } else {
            // Copy one full UTF-8 scalar; multi-byte bytes are never `$`/digits.
            let ch = sql[index..].chars().next().unwrap_or_default();
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    std::borrow::Cow::Owned(out)
}
