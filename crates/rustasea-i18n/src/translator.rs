//! The [`Translator`] — locale-aware key lookup with fallback and formatting.

use crate::error::Result;
use crate::loader::{TranslationLoader, Translations};
use crate::message::{choose_form, interpolate, Param};

/// Resolves translation keys against an active locale and a fallback locale.
///
/// Lookup order is active locale first, then fallback locale. When a key is
/// missing from **both**, the raw key string is returned (Laravel parity) —
/// missing keys are never errors.
///
/// ```no_run
/// use rustasea_i18n::{TranslationLoader, Translator};
///
/// # fn main() -> Result<(), rustasea_i18n::I18nError> {
/// let loader = TranslationLoader::default();
/// let translator = Translator::load(&loader, "en", "en")?;
/// let line = translator.trans("auth.failed", &[("count", "5")]);
/// # let _ = line;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Translator {
    /// Locale the `primary` dictionary was actually loaded for.
    locale: String,
    /// Locale the `fallback` dictionary was actually loaded for.
    fallback_locale: String,
    /// Active locale requested by [`Translator::set_locale`], not yet loaded.
    desired_locale: String,
    /// Fallback locale requested by [`Translator::set_fallback_locale`].
    desired_fallback_locale: String,
    primary: Translations,
    fallback: Translations,
}

impl Translator {
    /// Create an empty translator for `locale` with `fallback_locale`.
    ///
    /// No dictionaries are loaded; every key resolves to itself until
    /// translations are supplied via [`Translator::with_translations`].
    pub fn new(locale: impl Into<String>, fallback_locale: impl Into<String>) -> Self {
        let locale = locale.into();
        let fallback_locale = fallback_locale.into();
        Self {
            desired_locale: locale.clone(),
            desired_fallback_locale: fallback_locale.clone(),
            locale,
            fallback_locale,
            primary: Translations::new(),
            fallback: Translations::new(),
        }
    }

    /// Build a translator from already-loaded dictionaries.
    pub fn with_translations(
        locale: impl Into<String>,
        fallback_locale: impl Into<String>,
        primary: Translations,
        fallback: Translations,
    ) -> Self {
        let locale = locale.into();
        let fallback_locale = fallback_locale.into();
        Self {
            desired_locale: locale.clone(),
            desired_fallback_locale: fallback_locale.clone(),
            locale,
            fallback_locale,
            primary,
            fallback,
        }
    }

    /// Load both the active and fallback locales through `loader`.
    ///
    /// The active locale is loaded first, then the fallback locale; a missing
    /// locale directory is tolerated and simply contributes no entries.
    pub fn load(
        loader: &TranslationLoader,
        locale: impl Into<String>,
        fallback_locale: impl Into<String>,
    ) -> Result<Self> {
        let locale = locale.into();
        let fallback_locale = fallback_locale.into();
        let primary = loader.load_locale(&locale)?;
        let fallback = loader.load_locale(&fallback_locale)?;
        Ok(Self::with_translations(
            locale,
            fallback_locale,
            primary,
            fallback,
        ))
    }

    /// The active locale — the locale the primary dictionary was loaded for.
    ///
    /// This always agrees with the dictionary actually consulted by
    /// [`Translator::get`]; it changes only when [`Translator::reload`] commits
    /// a staged [`Translator::set_locale`] change. Use
    /// [`Translator::requested_locale`] to read a not-yet-loaded staged value.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// The fallback locale — the locale the fallback dictionary was loaded for.
    ///
    /// Mirrors [`Translator::locale`]: it reflects the committed fallback and
    /// only changes on [`Translator::reload`].
    pub fn fallback_locale(&self) -> &str {
        &self.fallback_locale
    }

    /// The active locale staged by [`Translator::set_locale`], loaded or not.
    ///
    /// Equal to [`Translator::locale`] until a [`Translator::set_locale`] call
    /// stages a change that has not yet been committed by
    /// [`Translator::reload`].
    pub fn requested_locale(&self) -> &str {
        &self.desired_locale
    }

    /// The fallback locale staged by [`Translator::set_fallback_locale`].
    ///
    /// Equal to [`Translator::fallback_locale`] until a
    /// [`Translator::set_fallback_locale`] call stages a pending change.
    pub fn requested_fallback_locale(&self) -> &str {
        &self.desired_fallback_locale
    }

    /// Replace the active locale **string** only.
    ///
    /// This does **not** reload dictionaries: lookups continue to resolve the
    /// previously loaded primary dictionary until [`Translator::reload`] is
    /// called. Use `set_locale` followed by `reload` to switch locales for real;
    /// [`Translator::locale`] alone is not a guarantee that the active
    /// dictionary matches it.
    pub fn set_locale(&mut self, locale: impl Into<String>) {
        self.desired_locale = locale.into();
    }

    /// Replace the fallback locale **string** only.
    ///
    /// Like [`Translator::set_locale`], this does not reload dictionaries; call
    /// [`Translator::reload`] afterwards to load the new fallback dictionary.
    pub fn set_fallback_locale(&mut self, locale: impl Into<String>) {
        self.desired_fallback_locale = locale.into();
    }

    /// Reload both dictionaries for the current locale pair through `loader`.
    ///
    /// Loads the active locale first, then the fallback locale, into temporary
    /// dictionaries and swaps them in **only on success**. If either locale has
    /// no directory under the loader base, a typed
    /// [`I18nError::MissingLocale`](crate::I18nError::MissingLocale) is returned
    /// and the previously loaded dictionaries remain intact — so there is never
    /// a state where [`Translator::locale`] disagrees with the dictionaries in
    /// use.
    ///
    /// Typical switching flow:
    ///
    /// ```no_run
    /// # use rustasea_i18n::{TranslationLoader, Translator};
    /// # fn main() -> Result<(), rustasea_i18n::I18nError> {
    /// let loader = TranslationLoader::default();
    /// let mut translator = Translator::load(&loader, "en", "en")?;
    /// translator.set_locale("fr");
    /// translator.reload(&loader)?; // now resolves French, fallback intact
    /// # Ok(())
    /// # }
    /// ```
    pub fn reload(&mut self, loader: &TranslationLoader) -> Result<()> {
        let primary = loader.load_locale_required(&self.desired_locale)?;
        let fallback = loader.load_locale_required(&self.desired_fallback_locale)?;
        self.primary = primary;
        self.fallback = fallback;
        self.locale.clone_from(&self.desired_locale);
        self.fallback_locale
            .clone_from(&self.desired_fallback_locale);
        Ok(())
    }

    /// Resolve a raw message: active locale, then fallback. `None` when absent
    /// from both.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.primary.get(key).or_else(|| self.fallback.get(key))
    }

    /// True when `key` resolves in the active or fallback locale.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Translate `key`, interpolating `:placeholder` params.
    ///
    /// Returns the raw `key` when it is missing from both locales.
    pub fn trans(&self, key: &str, params: &[Param<'_>]) -> String {
        match self.get(key) {
            Some(message) => interpolate(message, params),
            None => key.to_string(),
        }
    }

    /// Translate `key` selecting the plural form for `count`.
    ///
    /// The resolved line is split on `|` (see [`crate::message::choose_form`]);
    /// `count` is also made available to interpolation as `:count`. Returns the
    /// raw `key` when it is missing from both locales.
    pub fn trans_choice(&self, key: &str, count: i64, params: &[Param<'_>]) -> String {
        let Some(message) = self.get(key) else {
            return key.to_string();
        };
        let line = choose_form(message, count);
        let count_text = count.to_string();
        let mut owned: Vec<Param<'_>> = vec![("count", count_text.as_str())];
        owned.extend_from_slice(params);
        interpolate(&line, &owned)
    }
}
