//! Faker locales and their resolution from `AppConfig.faker_locale` codes.
//!
//! `fake` ships one marker type per locale (each a unit struct implementing
//! [`fake::locales::Data`]); [`Locale`] is the small, `Copy` enum the [`Faker`]
//! facade matches on to select the marker. The mapping mirrors the locale
//! codes `fakerphp/faker` accepts: a base language (`de`, `fr`, …) resolves to
//! a sensible default region, while an explicit `lang_REGION` code pins the
//! exact dataset.
//!
//! [`Faker`]: super::Faker

/// Fallback locale code used when a configured locale is unsupported.
pub const DEFAULT_LOCALE_CODE: &str = "en";

/// A supported faker locale (one per `fake` locale dataset).
///
/// The variants are named after their canonical BCP-47-style codes:
/// `En` (`en_US`), `FrFr` (`fr_FR`), `ZhCn` (`zh_CN`), `ZhTw` (`zh_TW`),
/// `ArSa` (`ar_SA`), `JaJp` (`ja_JP`), `PtBr` (`pt_BR`), `PtPt` (`pt_PT`),
/// `DeDe` (`de_DE`), `ItIt` (`it_IT`), `CyGb` (`cy_GB`), `NlNl` (`nl_NL`),
/// `TrTr` (`tr_TR`), and `FaIr` (`fa_IR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    /// `en_US` — the default (English/United States).
    En,
    /// `fr_FR` — French/France.
    FrFr,
    /// `zh_CN` — Chinese/China (Simplified).
    ZhCn,
    /// `zh_TW` — Chinese/Taiwan (Traditional).
    ZhTw,
    /// `ar_SA` — Arabic/Saudi Arabia.
    ArSa,
    /// `ja_JP` — Japanese/Japan.
    JaJp,
    /// `pt_BR` — Portuguese/Brazil (the default for a bare `pt`).
    PtBr,
    /// `pt_PT` — Portuguese/Portugal.
    PtPt,
    /// `de_DE` — German/Germany.
    DeDe,
    /// `it_IT` — Italian/Italy.
    ItIt,
    /// `cy_GB` — Welsh/United Kingdom.
    CyGb,
    /// `nl_NL` — Dutch/Netherlands.
    NlNl,
    /// `tr_TR` — Turkish/Turkey.
    TrTr,
    /// `fa_IR` — Persian/Iran.
    FaIr,
}

impl Locale {
    /// The canonical `lang_REGION` code for this locale.
    pub fn as_str(&self) -> &'static str {
        match self {
            Locale::En => "en_US",
            Locale::FrFr => "fr_FR",
            Locale::ZhCn => "zh_CN",
            Locale::ZhTw => "zh_TW",
            Locale::ArSa => "ar_SA",
            Locale::JaJp => "ja_JP",
            Locale::PtBr => "pt_BR",
            Locale::PtPt => "pt_PT",
            Locale::DeDe => "de_DE",
            Locale::ItIt => "it_IT",
            Locale::CyGb => "cy_GB",
            Locale::NlNl => "nl_NL",
            Locale::TrTr => "tr_TR",
            Locale::FaIr => "fa_IR",
        }
    }

    /// Resolve a locale from a configured code, or `None` when unsupported.
    ///
    /// Accepts the same spellings `fakerphp/faker` does: case-insensitive with
    /// either `_` or `-` as the region separator (`en`, `en_US`, `en-US`,
    /// `en_us`). A bare language code maps to a default region: `pt` → Brazil,
    /// `zh` → Simplified, and every other language to its primary dataset.
    /// Strictness is preserved by returning `None`; use
    /// [`resolve_faker_locale`] for a fail-open resolution.
    pub fn from_code(code: &str) -> Option<Locale> {
        let normalized = code.trim().to_ascii_lowercase().replace('-', "_");
        let locale = match normalized.as_str() {
            "en" | "en_us" => Locale::En,
            "fr" | "fr_fr" => Locale::FrFr,
            "zh" | "zh_cn" => Locale::ZhCn,
            "zh_tw" => Locale::ZhTw,
            "ar" | "ar_sa" => Locale::ArSa,
            "ja" | "ja_jp" => Locale::JaJp,
            "pt" | "pt_br" => Locale::PtBr,
            "pt_pt" => Locale::PtPt,
            "de" | "de_de" => Locale::DeDe,
            "it" | "it_it" => Locale::ItIt,
            "cy" | "cy_gb" => Locale::CyGb,
            "nl" | "nl_nl" => Locale::NlNl,
            "tr" | "tr_tr" => Locale::TrTr,
            "fa" | "fa_ir" => Locale::FaIr,
            _ => return None,
        };
        Some(locale)
    }
}

/// Resolve a configured locale code, falling back to English (fail-open).
///
/// Mirrors the resilience contract of `AppConfig.faker_locale`: an unsupported
/// value must never panic a test run, so it resolves to [`Locale::En`] and
/// emits a `tracing::warn!` naming the offending code. Callers that need
/// strictness should use [`Locale::from_code`] directly.
pub fn resolve_faker_locale(code: &str) -> Locale {
    Locale::from_code(code).unwrap_or_else(|| {
        tracing::warn!(
            locale = code,
            fallback = DEFAULT_LOCALE_CODE,
            "unsupported faker locale; falling back"
        );
        Locale::En
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every canonical `lang_REGION` code round-trips to its variant.
    #[test]
    fn canonical_codes_parse() {
        let cases = [
            ("en_US", Locale::En),
            ("fr_FR", Locale::FrFr),
            ("zh_CN", Locale::ZhCn),
            ("zh_TW", Locale::ZhTw),
            ("ar_SA", Locale::ArSa),
            ("ja_JP", Locale::JaJp),
            ("pt_BR", Locale::PtBr),
            ("pt_PT", Locale::PtPt),
            ("de_DE", Locale::DeDe),
            ("it_IT", Locale::ItIt),
            ("cy_GB", Locale::CyGb),
            ("nl_NL", Locale::NlNl),
            ("tr_TR", Locale::TrTr),
            ("fa_IR", Locale::FaIr),
        ];
        for (code, expected) in cases {
            assert_eq!(Locale::from_code(code), Some(expected), "code {code}");
            assert_eq!(expected.as_str(), code);
        }
    }

    /// Separator and case variants of the same code all resolve identically.
    #[test]
    fn aliases_and_normalization_parse() {
        for code in ["en", "EN", "en_us", "en-US", " en_us "] {
            assert_eq!(Locale::from_code(code), Some(Locale::En), "code {code:?}");
        }
        assert_eq!(Locale::from_code("fr"), Some(Locale::FrFr));
        assert_eq!(Locale::from_code("pt"), Some(Locale::PtBr));
        assert_eq!(Locale::from_code("zh"), Some(Locale::ZhCn));
        assert_eq!(Locale::from_code("de"), Some(Locale::DeDe));
        assert_eq!(Locale::from_code("ja"), Some(Locale::JaJp));
        assert_eq!(Locale::from_code("ar"), Some(Locale::ArSa));
        assert_eq!(Locale::from_code("it"), Some(Locale::ItIt));
        assert_eq!(Locale::from_code("nl"), Some(Locale::NlNl));
        assert_eq!(Locale::from_code("tr"), Some(Locale::TrTr));
        assert_eq!(Locale::from_code("fa"), Some(Locale::FaIr));
        assert_eq!(Locale::from_code("cy"), Some(Locale::CyGb));
    }

    /// Unknown codes are rejected by the strict parser.
    #[test]
    fn unknown_codes_are_none() {
        for code in ["", "xx", "xx_YY", "klingon", "en_GB"] {
            assert_eq!(Locale::from_code(code), None, "code {code:?}");
        }
    }

    /// The fail-open resolver always yields English for unknown codes.
    #[test]
    fn resolve_falls_back_to_english() {
        assert_eq!(resolve_faker_locale("de_DE"), Locale::DeDe);
        assert_eq!(resolve_faker_locale("nope"), Locale::En);
        assert_eq!(resolve_faker_locale(""), Locale::En);
    }
}
