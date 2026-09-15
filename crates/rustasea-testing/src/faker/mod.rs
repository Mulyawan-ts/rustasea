//! Faker facade — locale-aware, deterministically seedable fake data.
//!
//! `fakerphp/faker` gives Laravel tests and seeders realistic fake values
//! (`fake()->name()`, `fake()->unique()->email()`) with a configurable locale
//! (`AppConfig.faker_locale`). This module is the RustaSea equivalent: a thin
//! [`Faker`] facade over the [`fake`] crate that
//!
//! - draws every value from one seeded `ChaCha12Rng`, so a run is reproducible
//!   from a single `u64` seed (failing tests replay exactly);
//! - selects the locale dataset per call from [`Locale`], resolved from the
//!   configured code via [`resolve_faker_locale`];
//! - tracks emitted values in a [`UniqueRegistry`] for the `unique_*` helpers,
//!   which surface exhaustion as [`FakerError::UniqueExhausted`].
//!
//! ```no_run
//! use rustasea_testing::faker::Faker;
//!
//! let mut faker = Faker::new(42);
//! let name = faker.name();
//! let email = faker.unique_email().expect("unique email");
//! # let _ = (name, email);
//! ```

mod locale;
mod unique;

#[cfg(test)]
mod tests;

pub use locale::{resolve_faker_locale, Locale, DEFAULT_LOCALE_CODE};
pub use unique::{UniqueRegistry, DEFAULT_MAX_ATTEMPTS};

use chrono::{DateTime, Utc};
use fake::rand::rngs::ChaCha12Rng;
use fake::rand::SeedableRng;
use fake::RngExt;

/// Errors produced by the [`Faker`] facade.
#[derive(Debug, thiserror::Error)]
pub enum FakerError {
    /// A `unique_*` helper could not find an unseen value within its budget.
    #[error("faker could not produce a unique `{field}` value within {attempts} attempts")]
    UniqueExhausted {
        /// The logical field name whose unique space was exhausted.
        field: &'static str,
        /// How many draws were attempted before giving up.
        attempts: usize,
    },
}

impl FakerError {
    /// A stable machine-readable error code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            FakerError::UniqueExhausted { .. } => "faker.unique_exhausted",
        }
    }
}

/// Dispatch a locale-parameterised raw faker against the facade's RNG.
///
/// `fake` exposes one raw faker type per field, generic over a locale marker
/// (`Name<EN>`, `Name<DE_DE>`, …). This macro turns the runtime [`Locale`] back
/// into the compile-time marker so every helper stays a single expression.
macro_rules! fake_with_locale {
    ($rng:expr, $locale:expr, $ty:ty, $faker:path $(, $arg:expr)*) => {{
        use fake::Fake;
        match $locale {
            Locale::En => $faker(fake::locales::EN $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::FrFr => $faker(fake::locales::FR_FR $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::ZhCn => $faker(fake::locales::ZH_CN $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::ZhTw => $faker(fake::locales::ZH_TW $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::ArSa => $faker(fake::locales::AR_SA $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::JaJp => $faker(fake::locales::JA_JP $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::PtBr => $faker(fake::locales::PT_BR $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::PtPt => $faker(fake::locales::PT_PT $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::DeDe => $faker(fake::locales::DE_DE $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::ItIt => $faker(fake::locales::IT_IT $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::CyGb => $faker(fake::locales::CY_GB $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::NlNl => $faker(fake::locales::NL_NL $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::TrTr => $faker(fake::locales::TR_TR $(, $arg)*).fake_with_rng::<$ty, _>($rng),
            Locale::FaIr => $faker(fake::locales::FA_IR $(, $arg)*).fake_with_rng::<$ty, _>($rng),
        }
    }};
}

/// A locale-aware, deterministically seedable fake-data generator.
///
/// Every value is drawn from the internal `ChaCha12Rng`; two `Faker`s built
/// with the same seed and locale produce identical sequences. Draw order is
/// significant — calling an extra helper advances the stream — so golden
/// sequences must replay the exact call order.
#[derive(Debug)]
pub struct Faker {
    /// The seeded deterministic RNG every helper draws from.
    rng: ChaCha12Rng,
    /// The locale dataset every helper selects.
    locale: Locale,
    /// Tracks values already emitted by the `unique_*` helpers.
    unique: UniqueRegistry,
}

impl Faker {
    /// Create a deterministic faker seeded with `seed` (locale [`Locale::En`]).
    ///
    /// Seeding through `ChaCha12Rng::seed_from_u64` is stable across runs and
    /// architectures, so the same seed always yields the same sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: ChaCha12Rng::seed_from_u64(seed),
            locale: Locale::En,
            unique: UniqueRegistry::new(),
        }
    }

    /// Alias for [`Faker::new`], matching FakerPHP's `fake()` naming.
    pub fn seeded(seed: u64) -> Self {
        Self::new(seed)
    }

    /// Create a non-deterministic faker seeded from the thread RNG.
    ///
    /// Useful for interactive seeding where reproducibility is not required:
    /// the sequence cannot be replayed, so prefer [`Faker::new`] in tests.
    pub fn random() -> Self {
        let mut thread_rng = fake::rand::rng();
        Self {
            rng: ChaCha12Rng::from_rng(&mut thread_rng),
            locale: Locale::En,
            unique: UniqueRegistry::new(),
        }
    }

    /// Create a faker from a configured seed and `faker_locale` code.
    ///
    /// The locale is resolved fail-open through [`resolve_faker_locale`]: an
    /// unsupported code falls back to English and emits a warning instead of
    /// panicking. Intended to be fed `AppConfig.faker_locale`.
    pub fn from_config(seed: u64, faker_locale: &str) -> Self {
        Self::new(seed).with_locale(resolve_faker_locale(faker_locale))
    }

    /// Return this faker with a different locale dataset (builder style).
    pub fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    /// Re-seed the internal RNG in place, restarting the sequence.
    ///
    /// The unique-value registry is left untouched, so previously emitted
    /// values remain excluded from future `unique_*` draws.
    pub fn reseed(&mut self, seed: u64) {
        self.rng = ChaCha12Rng::seed_from_u64(seed);
    }

    /// Replace the unique-value attempt budget (builder style).
    ///
    /// Lowering the budget makes exhaustion reachable sooner; the default is
    /// [`DEFAULT_MAX_ATTEMPTS`].
    pub fn with_unique_max_attempts(mut self, max_attempts: usize) -> Self {
        self.unique = UniqueRegistry::with_max_attempts(max_attempts);
        self
    }

    /// The locale dataset this faker draws from.
    pub fn locale(&self) -> Locale {
        self.locale
    }

    /// The unique-value registry (attempt budget, recorded fields).
    pub fn unique_registry(&self) -> &UniqueRegistry {
        &self.unique
    }

    /// A full name (`"Jane Doe"`), localised.
    pub fn name(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(&mut self.rng, locale, String, fake::faker::name::raw::Name)
    }

    /// A first name, localised.
    pub fn first_name(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::name::raw::FirstName
        )
    }

    /// A last name, localised.
    pub fn last_name(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::name::raw::LastName
        )
    }

    /// An email on a real free provider (`user@gmail.com`-style).
    pub fn email(&mut self) -> String {
        self.free_email()
    }

    /// An email guaranteed not to resolve (`user@example.com`-style).
    pub fn safe_email(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::internet::raw::SafeEmail
        )
    }

    /// An email on a free provider; explicit alias of [`Faker::email`].
    pub fn free_email(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::internet::raw::FreeEmail
        )
    }

    /// A username handle, localised.
    pub fn username(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::internet::raw::Username
        )
    }

    /// A phone number, localised.
    pub fn phone_number(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::phone_number::raw::PhoneNumber
        )
    }

    /// A company name, localised.
    pub fn company_name(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::company::raw::CompanyName
        )
    }

    /// A job title, localised.
    pub fn job_title(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(&mut self.rng, locale, String, fake::faker::job::raw::Title)
    }

    /// A city name, localised.
    pub fn city(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::address::raw::CityName
        )
    }

    /// A country name, localised.
    pub fn country_name(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::address::raw::CountryName
        )
    }

    /// A street address line (`"4821 Oak Avenue"`), localised.
    pub fn street_address(&mut self) -> String {
        let locale = self.locale;
        let number: String = fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::address::raw::BuildingNumber
        );
        let street: String = fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::address::raw::StreetName
        );
        format!("{number} {street}")
    }

    /// Alias for [`Faker::street_address`].
    pub fn address(&mut self) -> String {
        self.street_address()
    }

    /// A postal code, localised.
    pub fn postcode(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::address::raw::PostCode
        )
    }

    /// A single lorem word.
    pub fn word(&mut self) -> String {
        let locale = self.locale;
        fake_with_locale!(&mut self.rng, locale, String, fake::faker::lorem::raw::Word)
    }

    /// Exactly `count` lorem words.
    pub fn words(&mut self, count: usize) -> Vec<String> {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            Vec<String>,
            fake::faker::lorem::raw::Words,
            count..count + 1
        )
    }

    /// A lorem sentence of exactly `count` words (ending in `.`).
    pub fn sentence(&mut self, count: usize) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::lorem::raw::Sentence,
            count..count + 1
        )
    }

    /// A lorem paragraph of exactly `count` sentences.
    pub fn paragraph(&mut self, count: usize) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::lorem::raw::Paragraph,
            count..count + 1
        )
    }

    /// A boolean that is `true` with probability `ratio` (clamped to `0.0..=1.0`).
    pub fn boolean_with_ratio(&mut self, ratio: f64) -> bool {
        self.rng.random_bool(ratio.clamp(0.0, 1.0))
    }

    /// A timestamp within `[start, end)`, drawn at minute granularity.
    pub fn date_between(&mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> DateTime<Utc> {
        self.date_time_between(start, end)
    }

    /// A timestamp within `[start, end)`, drawn at minute granularity.
    pub fn date_time_between(&mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> DateTime<Utc> {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            DateTime<Utc>,
            fake::faker::chrono::raw::DateTimeBetween,
            start,
            end
        )
    }

    /// A random UUID (v4), drawn deterministically from the seeded RNG.
    pub fn uuid(&mut self) -> uuid::Uuid {
        use fake::Fake;
        fake::uuid::UUIDv4.fake_with_rng::<uuid::Uuid, _>(&mut self.rng)
    }

    /// A random element from `items`, or `None` when the slice is empty.
    pub fn random_element<T: Clone>(&mut self, items: &[T]) -> Option<T> {
        use fake::rand::seq::IndexedRandom;
        items.choose(&mut self.rng).cloned()
    }

    /// Replace each `#` in `format` with a digit and each `^` with `1..=9`.
    pub fn numerify(&mut self, format: &str) -> String {
        let locale = self.locale;
        fake_with_locale!(
            &mut self.rng,
            locale,
            String,
            fake::faker::number::raw::NumberWithFormat,
            format
        )
    }

    /// Replace each `?` in `format` with a random lowercase ASCII letter.
    pub fn lexify(&mut self, format: &str) -> String {
        format
            .chars()
            .map(|ch| {
                if ch == '?' {
                    let offset: u8 = self.rng.random_range(0..26);
                    char::from(b'a' + offset)
                } else {
                    ch
                }
            })
            .collect()
    }

    /// A unique name across this faker's lifetime, or exhaustion error.
    pub fn unique_name(&mut self) -> Result<String, FakerError> {
        self.draw_unique("name", |faker| faker.name())
    }

    /// A unique email across this faker's lifetime, or exhaustion error.
    pub fn unique_email(&mut self) -> Result<String, FakerError> {
        self.draw_unique("email", |faker| faker.email())
    }

    /// A unique username across this faker's lifetime, or exhaustion error.
    pub fn unique_username(&mut self) -> Result<String, FakerError> {
        self.draw_unique("username", |faker| faker.username())
    }

    /// A unique value from an arbitrary `draw` closure, keyed by `field`.
    ///
    /// Generalises the `unique_*` helpers: `draw` receives the faker so it can
    /// call any helper, and `field` namespaces the seen-value set (distinct
    /// fields never collide). Exhaustion surfaces as
    /// [`FakerError::UniqueExhausted`].
    pub fn unique_with(
        &mut self,
        field: &'static str,
        draw: impl FnMut(&mut Faker) -> String,
    ) -> Result<String, FakerError> {
        self.draw_unique(field, draw)
    }

    /// Forget every recorded unique value, keeping the current RNG position.
    pub fn reset_unique(&mut self) {
        self.unique.reset();
    }

    /// Draw values from `draw` until one is unseen for `field`, else fail.
    ///
    /// Each retry draws a fresh value from the same seeded RNG, so a
    /// deterministic seed still reproduces the exact retry sequence.
    fn draw_unique(
        &mut self,
        field: &'static str,
        mut draw: impl FnMut(&mut Faker) -> String,
    ) -> Result<String, FakerError> {
        let attempts = self.unique.max_attempts();
        for _ in 0..attempts {
            let candidate = draw(self);
            if self.unique.accept(field, candidate.clone()) {
                return Ok(candidate);
            }
        }
        Err(FakerError::UniqueExhausted { field, attempts })
    }
}
