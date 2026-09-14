//! Integration-style tests for the i18n crate.
//!
//! These are written but not executed here — the tester owns test execution.

use std::fs;

use crate::{
    choose_form, interpolate, TranslationLoader, Translations, Translator, DEFAULT_LANG_DIR,
};

/// A temporary language directory removed on drop.
struct TempLang {
    root: std::path::PathBuf,
}

impl TempLang {
    /// Create a fresh temp directory with a process-unique name.
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rustasea-i18n-{}-{}", std::process::id(), unique));
        fs::create_dir_all(&root).expect("create temp lang dir");
        Self { root }
    }

    /// Write `contents` to `resources/lang/{locale}/{name}`.
    fn write(&self, locale: &str, name: &str, contents: &str) {
        let dir = self.root.join(locale);
        fs::create_dir_all(&dir).expect("create locale dir");
        fs::write(dir.join(name), contents).expect("write dictionary");
    }

    /// A loader rooted at this temp directory.
    fn loader(&self) -> TranslationLoader {
        TranslationLoader::new(&self.root)
    }
}

impl Drop for TempLang {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn default_lang_dir_is_resources_lang() {
    assert_eq!(DEFAULT_LANG_DIR, "resources/lang");
}

#[test]
fn toml_dictionary_loads_and_resolves() {
    let lang = TempLang::new();
    lang.write(
        "en",
        "auth.toml",
        "failed = \"These credentials do not match our records.\"\n",
    );
    let loader = lang.loader();
    let translations = loader.load_locale("en").expect("load en");
    assert_eq!(
        translations.get("auth.failed"),
        Some("These credentials do not match our records.")
    );
}

#[test]
fn json_dictionary_loads_and_flattens_nested_keys() {
    let lang = TempLang::new();
    lang.write(
        "en",
        "auth.json",
        r#"{ "passwords": { "reset": "We have emailed your reset link." } }"#,
    );
    let loader = lang.loader();
    let translations = loader.load_locale("en").expect("load en");
    assert_eq!(
        translations.get("auth.passwords.reset"),
        Some("We have emailed your reset link.")
    );
}

#[test]
fn missing_locale_directory_is_tolerated() {
    let lang = TempLang::new();
    let loader = lang.loader();
    let translations = loader.load_locale("zz").expect("missing dir tolerated");
    assert!(translations.is_empty());
}

#[test]
fn malformed_toml_is_a_typed_error() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \n");
    let loader = lang.loader();
    let err = loader
        .load_locale("en")
        .expect_err("malformed toml must fail");
    assert_eq!(err.code(), "I18nError::Malformed");
}

#[test]
fn positive_key_resolves_with_interpolated_params() {
    let lang = TempLang::new();
    lang.write(
        "en",
        "auth.toml",
        "failed = \"Attempt :count for :Name.\"\n",
    );
    let translator = Translator::load(&lang.loader(), "en", "en").expect("load translator");
    let line = translator.trans("auth.failed", &[("count", "5"), ("name", "ada")]);
    assert_eq!(line, "Attempt 5 for Ada.");
}

#[test]
fn negative_missing_key_in_both_locales_returns_raw_key() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"nope\"\n");
    lang.write("fr", "auth.toml", "failed = \"non\"\n");
    let translator = Translator::load(&lang.loader(), "en", "fr").expect("load translator");
    assert_eq!(translator.trans("auth.unknown", &[]), "auth.unknown");
}

#[test]
fn fallback_locale_is_consulted_when_primary_misses() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"primary only\"\n");
    lang.write(
        "fr",
        "auth.toml",
        "failed = \"non\"\nthrottle = \"fallback throttle\"\n",
    );
    let translator = Translator::load(&lang.loader(), "fr", "en").expect("load translator");
    assert_eq!(translator.trans("auth.throttle", &[]), "fallback throttle");
    assert_eq!(translator.trans("auth.failed", &[]), "non");
}

#[test]
fn trans_choice_selects_plural_form() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "items = \":count item|:count items\"\n");
    let translator = Translator::load(&lang.loader(), "en", "en").expect("load translator");
    assert_eq!(translator.trans_choice("auth.items", 1, &[]), "1 item");
    assert_eq!(translator.trans_choice("auth.items", 5, &[]), "5 items");
}

#[test]
fn trans_choice_missing_key_returns_raw_key() {
    let lang = TempLang::new();
    let translator = Translator::load(&lang.loader(), "en", "en").expect("load translator");
    assert_eq!(translator.trans_choice("auth.items", 3, &[]), "auth.items");
}

#[test]
fn choose_form_handles_explicit_and_range_forms() {
    assert_eq!(choose_form("{0} none|{1} one|[2,*] many", 0), "none");
    assert_eq!(choose_form("{0} none|{1} one|[2,*] many", 1), "one");
    assert_eq!(choose_form("{0} none|{1} one|[2,*] many", 9), "many");
    assert_eq!(choose_form("]1,5[ few|other", 3), "few");
    assert_eq!(choose_form("single", 2), "single");
}

#[test]
fn interpolate_leaves_unknown_placeholders_untouched() {
    assert_eq!(
        interpolate("Hi :name, :missing", &[("name", "Ada")]),
        "Hi Ada, :missing"
    );
}

#[test]
fn has_reports_resolution_across_locales() {
    let mut primary = Translations::new();
    primary.insert("a", "one");
    let mut fallback = Translations::new();
    fallback.insert("b", "two");
    let translator = Translator::with_translations("en", "fr", primary, fallback);
    assert!(translator.has("a"));
    assert!(translator.has("b"));
    assert!(!translator.has("c"));
    assert_eq!(translator.locale(), "en");
    assert_eq!(translator.fallback_locale(), "fr");
}

#[test]
fn global_helpers_degrade_without_translator() {
    crate::clear_translator();
    assert_eq!(crate::__("auth.failed", &[]), "auth.failed");
    assert_eq!(crate::trans_choice("auth.items", 2, &[]), "auth.items");
}

#[test]
fn traversal_locale_is_rejected_before_io() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"ok\"\n");
    let loader = lang.loader();

    for bad in [
        "../..",
        "../../en",
        "en/../..",
        "/etc",
        "/etc/passwd",
        "en/../../x",
    ] {
        let err = loader
            .load_locale(bad)
            .expect_err("traversal locale must be rejected");
        assert_eq!(err.code(), "I18nError::InvalidLocale", "locale {bad:?}");
    }
}

#[test]
fn empty_dot_and_odd_locales_are_rejected() {
    let lang = TempLang::new();
    let loader = lang.loader();
    for bad in ["", ".", "..", "en us", "en.us", "fr\u{00e9}"] {
        let err = loader
            .load_locale(bad)
            .expect_err("invalid locale must be rejected");
        assert_eq!(err.code(), "I18nError::InvalidLocale", "locale {bad:?}");
    }
}

#[test]
fn hyphen_and_underscore_locales_are_accepted() {
    let lang = TempLang::new();
    lang.write("en-US", "auth.toml", "failed = \"american\"\n");
    lang.write("pt_BR", "auth.toml", "failed = \"brasileiro\"\n");
    let loader = lang.loader();

    assert_eq!(
        loader
            .load_locale("en-US")
            .expect("en-US")
            .get("auth.failed"),
        Some("american")
    );
    assert_eq!(
        loader
            .load_locale("pt_BR")
            .expect("pt_BR")
            .get("auth.failed"),
        Some("brasileiro")
    );
}

#[test]
fn choose_form_supports_half_open_intervals() {
    // `]1, *]` — exclusive low, unbounded high: count >= 2.
    assert_eq!(choose_form("]1, *] many|other", 2), "many");
    assert_eq!(choose_form("]1, *] many|other", 99), "many");
    assert_eq!(choose_form("]1, *] many|other", 1), "other");

    // `]0, 10]` — exclusive low, inclusive high: 1..=10.
    assert_eq!(choose_form("]0, 10] batch|other", 1), "batch");
    assert_eq!(choose_form("]0, 10] batch|other", 10), "batch");
    assert_eq!(choose_form("]0, 10] batch|other", 11), "other");
    assert_eq!(choose_form("]0, 10] batch|other", 0), "other");

    // `[1,10[` — inclusive low, exclusive high: 1..=9.
    assert_eq!(choose_form("[1,10[ batch|other", 1), "batch");
    assert_eq!(choose_form("[1,10[ batch|other", 9), "batch");
    assert_eq!(choose_form("[1,10[ batch|other", 10), "other");
}

#[test]
fn malformed_interval_falls_back_to_standard_split() {
    // Missing comma → not an interval; the raw segment is the standard form.
    assert_eq!(choose_form("[1 10] one|two", 1), "[1 10] one");
    assert_eq!(choose_form("[1 10] one|two", 5), "two");
    // Missing closing bracket → standard split again.
    assert_eq!(choose_form("[1,10 one|two", 1), "[1,10 one");
    assert_eq!(choose_form("[1,10 one|two", 5), "two");
    // Non-numeric bound → standard split.
    assert_eq!(choose_form("[x,y] one|two", 2), "two");
    // No bracket at all is untouched.
    assert_eq!(choose_form("one|two", 1), "one");
}

#[test]
fn set_locale_plus_reload_switches_resolution() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"english\"\n");
    lang.write("fr", "auth.toml", "failed = \"french\"\n");
    let loader = lang.loader();
    let mut translator = Translator::load(&loader, "en", "en").expect("load translator");
    assert_eq!(translator.locale(), "en");
    assert_eq!(translator.trans("auth.failed", &[]), "english");

    translator.set_locale("fr");
    // Staged but not yet committed: `locale()` still reflects the loaded dict.
    assert_eq!(translator.locale(), "en");
    assert_eq!(translator.requested_locale(), "fr");

    translator.reload(&loader).expect("reload fr");
    assert_eq!(translator.locale(), "fr");
    assert_eq!(translator.trans("auth.failed", &[]), "french");
}

#[test]
fn reload_with_missing_locale_keeps_previous_dictionaries() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"english\"\n");
    let loader = lang.loader();
    let mut translator = Translator::load(&loader, "en", "en").expect("load translator");

    translator.set_locale("de");
    let err = translator
        .reload(&loader)
        .expect_err("missing locale dir must fail");
    assert_eq!(err.code(), "I18nError::MissingLocale");
    // Previous dictionaries and committed locale are untouched.
    assert_eq!(translator.locale(), "en");
    assert_eq!(translator.trans("auth.failed", &[]), "english");
}

#[test]
fn reload_with_invalid_locale_is_typed_and_non_destructive() {
    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"english\"\n");
    let loader = lang.loader();
    let mut translator = Translator::load(&loader, "en", "en").expect("load translator");

    translator.set_locale("../..");
    let err = translator
        .reload(&loader)
        .expect_err("traversal locale must fail reload");
    assert_eq!(err.code(), "I18nError::InvalidLocale");
    assert_eq!(translator.locale(), "en");
    assert_eq!(translator.trans("auth.failed", &[]), "english");
}

#[test]
fn interpolation_is_prefix_safe_for_shared_prefixes() {
    // `:count` must not corrupt `:country` even when both are supplied.
    assert_eq!(
        interpolate(
            ":country has :count",
            &[("count", "5"), ("country", "France")]
        ),
        "France has 5"
    );
    // Order of params must not matter.
    assert_eq!(
        interpolate(
            ":country has :count",
            &[("country", "France"), ("count", "5")]
        ),
        "France has 5"
    );
    // `:count` still interpolates alone.
    assert_eq!(interpolate(":count items", &[("count", "5")]), "5 items");
}

#[test]
fn interpolation_handles_adjacent_placeholders() {
    assert_eq!(interpolate(":a:b", &[("a", "1"), ("b", "2")]), "12");
    // Capitalized and upper-case casings remain prefix-safe too.
    assert_eq!(
        interpolate(":Country / :COUNTRY", &[("count", "9"), ("country", "ch")]),
        "Ch / CH"
    );
}
