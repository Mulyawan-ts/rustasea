//! Message formatting — `:placeholder` interpolation and `|`-separated
//! pluralization selection (Laravel `MessageSelector` parity).

/// A single `(name, value)` interpolation pair.
///
/// Call sites pass these as a slice, e.g.
/// `&[("count", "5"), ("name", "Ada")]`, matching the
/// `__("auth.failed", &[("count", "5")])` ergonomics.
pub type Param<'a> = (&'a str, &'a str);

/// Replace `:name` placeholders in `message` with the supplied pairs.
///
/// Three casings are honoured per Laravel parity:
///
/// - `:name` → the value verbatim.
/// - `:Name` → the value with its first character upper-cased.
/// - `:NAME` → the value upper-cased.
///
/// Replacement is **prefix-safe**: parameters are applied longest-key-first and
/// a `:placeholder` only matches when the character immediately following its
/// name is not an identifier character (`[A-Za-z0-9_]`). Thus `:count` will not
/// corrupt `:country`, even when both are supplied — the longer key wins and the
/// boundary check protects the shorter one.
///
/// Unknown placeholders are left untouched.
pub fn interpolate(message: &str, params: &[Param<'_>]) -> String {
    // Longest keys first so a shorter key cannot pre-empt a longer key that
    // shares its prefix (e.g. `:count` vs `:country`).
    let mut ordered: Vec<Param<'_>> = params.to_vec();
    ordered.sort_by_key(|param| std::cmp::Reverse(param.0.len()));

    let mut out = message.to_string();
    for (name, value) in ordered {
        if name.is_empty() {
            continue;
        }
        out = replace_token(&out, &format!(":{name}"), value);
        out = replace_token(&out, &format!(":{}", capitalize(name)), &capitalize(value));
        out = replace_token(
            &out,
            &format!(":{}", name.to_uppercase()),
            &value.to_uppercase(),
        );
    }
    out
}

/// Replace every `token` occurrence in `haystack` whose immediately following
/// character is not an identifier character.
///
/// This boundary check keeps `:count` from matching inside `:country`, where the
/// next character (`r`) is alphanumeric.
fn replace_token(haystack: &str, token: &str, value: &str) -> String {
    if token.len() <= 1 {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(pos) = rest.find(token) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + token.len()..];
        if after.chars().next().is_none_or(|c| !is_ident_char(c)) {
            out.push_str(value);
        } else {
            // Not a token boundary: keep the text literally and keep scanning.
            out.push_str(token);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// True when `c` may continue an identifier (`[A-Za-z0-9_]`).
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Upper-case the first character of `value`, leaving the remainder unchanged.
fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Select the correct plural form from a `|`-separated `line` for `count`.
///
/// Supported forms:
///
/// - `singular|plural` — the classic two-form case.
/// - Explicit count forms `{0}`, `{1}`, … matched exactly.
/// - Ranges with independent boundaries: `[a,b]` (inclusive), `]a,b[`
///   (exclusive), and the half-open mixes `]a,b]` / `[a,b[`.
///
/// When no explicit form matches, the standard rule applies: `count == 1`
/// selects the first non-explicit form, anything else the second (falling back
/// to the first when only one non-explicit form exists). A `line` without a
/// `|` is returned unchanged.
pub fn choose_form(line: &str, count: i64) -> String {
    let parts: Vec<&str> = line.split('|').map(str::trim).collect();
    if parts.len() == 1 {
        return parts[0].to_string();
    }

    let mut standard: Vec<&str> = Vec::new();
    let mut explicit: Vec<(Form, &str)> = Vec::new();
    for part in &parts {
        match parse_form(part) {
            Some((form, message)) => explicit.push((form, message)),
            None => standard.push(part),
        }
    }

    for (form, message) in &explicit {
        if form.matches(count) {
            return (*message).to_string();
        }
    }

    let index = if count == 1 { 0 } else { 1 };
    let chosen = standard.get(index).or_else(|| standard.first());
    chosen.map_or_else(|| line.to_string(), |s| (*s).to_string())
}

/// An explicit plural form attached to a range or exact count.
#[derive(Debug, Clone, Copy)]
enum Form {
    /// Exact match, e.g. `{0}`.
    Exact(i64),
    /// Inclusive/exclusive range, e.g. `[1,3]` or `]3,5[`.
    Range {
        /// Lower bound.
        low: i64,
        /// Whether `low` itself matches.
        low_inclusive: bool,
        /// Upper bound; `None` means unbounded (`[4,*]`).
        high: Option<i64>,
        /// Whether `high` itself matches.
        high_inclusive: bool,
    },
}

impl Form {
    /// Return true when `count` satisfies this form.
    fn matches(&self, count: i64) -> bool {
        match *self {
            Form::Exact(n) => count == n,
            Form::Range {
                low,
                low_inclusive,
                high,
                high_inclusive,
            } => {
                let above_low = if low_inclusive {
                    count >= low
                } else {
                    count > low
                };
                let below_high = match high {
                    None => true,
                    Some(h) if high_inclusive => count <= h,
                    Some(h) => count < h,
                };
                above_low && below_high
            }
        }
    }
}

/// Parse the explicit prefix of a plural segment (`{0}`, `[1,3]`, `]3,5[`).
///
/// Returns the parsed [`Form`] and the remaining message text (with the prefix
/// and any separating space stripped). Returns `None` when the segment carries
/// no explicit prefix, marking it a "standard" form.
///
/// Interval boundaries are independent, matching Symfony/Laravel half-open
/// intervals: the lower bracket sets inclusivity of the low bound (`[` =
/// inclusive, `]` = exclusive) and the closing bracket — either `]` or `[` —
/// sets inclusivity of the high bound (`]` = inclusive, `[` = exclusive).
/// Malformed intervals (no comma or no closing bracket) return `None`, so they
/// fall back to the standard singular|plural split.
fn parse_form(part: &str) -> Option<(Form, &str)> {
    let part = part.trim();
    let first = part.chars().next()?;

    if first == '{' {
        let end = part.find('}')?;
        let body = &part[1..end];
        let rest = part[end + 1..].trim_start();
        return Some((Form::Exact(body.trim().parse::<i64>().ok()?), rest));
    }

    if first != '[' && first != ']' {
        return None;
    }

    // The low bound runs from after the opening bracket to the comma; the high
    // bound runs to whichever bracket (`]` or `[`) appears first after it.
    let comma = part.find(',')?;
    let tail = &part[comma + 1..];
    let close_rel = tail.find([']', '['])?;
    let closing = tail.as_bytes()[close_rel] as char;
    let rest = part[comma + 1 + close_rel + 1..].trim_start();

    let low = part[1..comma].trim();
    let high = tail[..close_rel].trim();

    Some((
        Form::Range {
            low: parse_bound(low)?,
            low_inclusive: first == '[',
            high: parse_high(high)?,
            high_inclusive: closing == ']',
        },
        rest,
    ))
}

/// Parse a lower bound, which is always a concrete integer.
fn parse_bound(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

/// Parse an upper bound, where `*` denotes "unbounded".
fn parse_high(value: &str) -> Option<Option<i64>> {
    if value == "*" {
        Some(None)
    } else {
        value.parse::<i64>().ok().map(Some)
    }
}
