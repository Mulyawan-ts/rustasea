//! Slug generation — Unicode-aware, deterministic, and collision-safe.
//!
//! Mirrors the `cviebrock/eloquent-sluggable` contract (ADOPT-016): a model
//! declares one or more source columns and the ORM derives a URL-safe slug on
//! write. The pipeline is [`slugify`] (transliterate → lowercase → keep ASCII
//! alphanumerics → collapse separators) composed with a uniqueness search that
//! appends `-2`, `-3`, … until the candidate is free.
//!
//! [`SlugOptions`] is the per-model configuration the `#[derive(Model)]` macro
//! emits through [`Model::slug_options`](crate::model::Model::slug_options); the
//! write-path hook in `crate::model_ops::slug_hooks` consumes it. A model that
//! never opts in keeps the trait defaults and pays no cost.
//!
//! ```rust,ignore
//! let options = SlugOptions::default()
//!     .from(&["title"])
//!     .to("slug")
//!     .max_len(80);
//! let slug = options.generate(&[("title".into(), "Héllo World".into())]);
//! assert_eq!(slug, "hello-world");
//! ```

/// Transliterate `text` to a URL-safe slug using `separator` between words.
///
/// The steps match eloquent-sluggable's default (`str_slug`): deunicode
/// transliteration to ASCII, lowercase, keep ASCII alphanumerics, replace every
/// other run of characters with the separator, collapse consecutive separators,
/// and trim leading/trailing separators. An input that reduces to nothing
/// yields an empty string (the caller decides whether that is an error).
///
/// # Examples
///
/// ```
/// use rustasea_orm::sluggable::slugify;
/// assert_eq!(slugify("Café au Lait", '-'), "cafe-au-lait");
/// assert_eq!(slugify("  Hello,   World!  ", '-'), "hello-world");
/// assert_eq!(slugify("!!!", '-'), "");
/// ```
pub fn slugify(text: &str, separator: char) -> String {
    let transliterated = deunicode::deunicode(text);
    let lowered = transliterated.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_separator = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push(separator);
            }
            pending_separator = false;
            out.push(ch);
        } else {
            pending_separator = true;
        }
    }
    out
}

/// Per-model slug configuration produced by `#[sluggable(...)]`.
///
/// The defaults match eloquent-sluggable: derive from no columns (the macro
/// always sets [`source`](SlugOptions::source)), write to `slug`, join with `-`,
/// enforce uniqueness, do **not** regenerate on update, and cap at 255 chars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugOptions {
    /// Source columns whose values are concatenated into the slug.
    pub source: Vec<String>,
    /// Column that stores the generated slug.
    pub slug_column: String,
    /// Word separator used in the slug.
    pub separator: char,
    /// Whether a colliding slug is suffixed (`-2`, `-3`, …) to stay unique.
    pub unique: bool,
    /// Whether the slug is regenerated when a source column changes on update.
    pub on_update: bool,
    /// Maximum slug length; longer slugs are truncated at a char boundary.
    pub max_len: usize,
}

impl Default for SlugOptions {
    /// The eloquent-sluggable defaults.
    fn default() -> Self {
        Self {
            source: Vec::new(),
            slug_column: "slug".to_string(),
            separator: '-',
            unique: true,
            on_update: false,
            max_len: 255,
        }
    }
}

impl SlugOptions {
    /// Declare the source columns, in concatenation order.
    ///
    /// An inherent method named `from` is intentional (parity with the
    /// eloquent-sluggable `sluggable()` config array); it is not a `From` impl.
    #[allow(clippy::should_implement_trait)]
    pub fn from(mut self, sources: &[&str]) -> Self {
        self.source = sources.iter().map(|source| (*source).to_string()).collect();
        self
    }

    /// Set the destination slug column.
    pub fn to(mut self, column: &str) -> Self {
        self.slug_column = column.to_string();
        self
    }

    /// Set the word separator.
    pub fn separator(mut self, separator: char) -> Self {
        self.separator = separator;
        self
    }

    /// Enable or disable collision suffixing.
    pub fn unique(mut self, unique: bool) -> Self {
        self.unique = unique;
        self
    }

    /// Enable or disable regeneration on update.
    pub fn on_update(mut self, on_update: bool) -> Self {
        self.on_update = on_update;
        self
    }

    /// Set the maximum slug length.
    pub fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = max_len;
        self
    }

    /// Build a slug from the model's `(column, value)` source pairs.
    ///
    /// Values are selected in [`source`](SlugOptions::source) order (a source
    /// column absent from `values` is skipped) and joined with the separator;
    /// the result is slugified and truncated to [`max_len`](SlugOptions::max_len)
    /// at a char boundary. An empty or fully non-alphanumeric source yields an
    /// empty string — the write-path hook treats that as a typed error rather
    /// than persisting a silent empty slug.
    pub fn generate(&self, values: &[(String, String)]) -> String {
        let joined = self.join_sources(values);
        let slug = slugify(&joined, self.separator);
        self.truncate(slug)
    }

    /// Join the configured source values with the separator (pre-slugify).
    fn join_sources(&self, values: &[(String, String)]) -> String {
        let separator = self.separator.to_string();
        let parts: Vec<&str> = if self.source.is_empty() {
            values.iter().map(|(_, value)| value.as_str()).collect()
        } else {
            self.source
                .iter()
                .filter_map(|column| {
                    values
                        .iter()
                        .find(|(name, _)| name == column)
                        .map(|(_, value)| value.as_str())
                })
                .collect()
        };
        parts.join(&separator)
    }

    /// Truncate `slug` to `max_len` characters, trimming a trailing separator.
    fn truncate(&self, slug: String) -> String {
        if self.max_len == 0 || slug.chars().count() <= self.max_len {
            return slug;
        }
        let truncated: String = slug.chars().take(self.max_len).collect();
        truncated.trim_end_matches(self.separator).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accented Latin text transliterates to plain ASCII.
    #[test]
    fn slugify_transliterates_accents() {
        assert_eq!(slugify("Café", '-'), "cafe");
        assert_eq!(slugify("Héllo Wörld", '-'), "hello-world");
    }

    /// Indonesian text keeps its ASCII letters and drops punctuation.
    #[test]
    fn slugify_handles_indonesian_text() {
        assert_eq!(
            slugify("Belajar Pemrograman Rust!", '-'),
            "belajar-pemrograman-rust"
        );
        assert_eq!(slugify("Nasi Goreng Spesial", '_'), "nasi_goreng_spesial");
    }

    /// Consecutive non-alphanumeric runs collapse to a single separator and
    /// leading/trailing separators are trimmed.
    #[test]
    fn slugify_collapses_and_trims_separators() {
        assert_eq!(slugify("  a -- b  ", '-'), "a-b");
        assert_eq!(slugify("a...b", '-'), "a-b");
        assert_eq!(slugify("!!!", '-'), "");
    }

    /// An empty source yields an empty slug.
    #[test]
    fn generate_empty_source_is_empty() {
        let options = SlugOptions::default().from(&["name"]);
        assert_eq!(options.generate(&[("name".into(), String::new())]), "");
        assert_eq!(options.generate(&[("name".into(), "   ".into())]), "");
    }

    /// Multiple source columns concatenate in declared order.
    #[test]
    fn generate_concatenates_multiple_sources() {
        let options = SlugOptions::default().from(&["first", "last"]);
        let slug = options.generate(&[
            ("last".into(), "Lovelace".into()),
            ("first".into(), "Ada".into()),
        ]);
        assert_eq!(slug, "ada-lovelace");
    }

    /// `max_len` truncates at a char boundary and trims a dangling separator.
    #[test]
    fn generate_truncates_to_max_len() {
        let options = SlugOptions::default().from(&["title"]).max_len(10);
        let slug = options.generate(&[("title".into(), "the quick brown fox".into())]);
        assert_eq!(slug, "the-quick");
        assert!(slug.chars().count() <= 10);

        // A truncation landing on a separator trims it.
        let options = SlugOptions::default().from(&["title"]).max_len(3);
        assert_eq!(options.generate(&[("title".into(), "ab cd".into())]), "ab");
    }

    /// The default configuration matches the documented eloquent-sluggable set.
    #[test]
    fn default_options_are_eloquent_defaults() {
        let options = SlugOptions::default();
        assert_eq!(options.slug_column, "slug");
        assert_eq!(options.separator, '-');
        assert!(options.unique);
        assert!(!options.on_update);
        assert_eq!(options.max_len, 255);
        assert!(options.source.is_empty());
    }
}
