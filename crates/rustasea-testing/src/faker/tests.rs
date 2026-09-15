//! Behavioural tests for the [`Faker`] facade.
//!
//! Covers the ADOPT-012 acceptance checklist: seeded determinism, locale
//! divergence, config-driven locale resolution with fail-open fallback, and
//! unique-value helpers that never repeat (and error on exhaustion).

use super::*;
use chrono::{TimeZone, Utc};

/// Collect `count` values produced by `draw` into a vector.
fn collect<T>(count: usize, mut draw: impl FnMut() -> T) -> Vec<T> {
    (0..count).map(|_| draw()).collect()
}

/// The same seed reproduces an identical interleaved sequence.
#[test]
fn same_seed_reproduces_sequence() {
    let mut a = Faker::new(2024);
    let mut b = Faker::new(2024);
    let seq = |f: &mut Faker| {
        collect(50, || {
            let name = f.name();
            let email = f.email();
            let city = f.city();
            format!("{name}|{email}|{city}")
        })
    };
    assert_eq!(seq(&mut a), seq(&mut b));
}

/// `Faker::seeded` is the documented alias for `Faker::new`.
#[test]
fn seeded_alias_matches_new() {
    let mut a = Faker::new(11);
    let mut b = Faker::seeded(11);
    assert_eq!(collect(20, || a.word()), collect(20, || b.word()));
}

/// Different seeds diverge.
#[test]
fn different_seeds_diverge() {
    let mut a = Faker::new(1);
    let mut b = Faker::new(2);
    assert_ne!(collect(20, || a.name()), collect(20, || b.name()));
}

/// Re-seeding restarts the stream and a new seed changes it.
#[test]
fn reseed_restarts_and_changes_sequence() {
    let mut faker = Faker::new(5);
    let first = collect(10, || faker.name());

    faker.reseed(5);
    assert_eq!(first, collect(10, || faker.name()), "same seed replays");

    faker.reseed(6);
    assert_ne!(first, collect(10, || faker.name()), "new seed diverges");
}

/// A locale switch changes the generated data.
#[test]
fn locale_switch_changes_output() {
    let mut en = Faker::new(77);
    let mut de = Faker::new(77).with_locale(Locale::DeDe);
    let en_names = collect(10, || en.first_name());
    let de_names = collect(10, || de.first_name());
    assert_ne!(en_names, de_names, "de_DE vs en first names must differ");
}

/// Every locale produces non-empty data without panicking.
#[test]
fn every_locale_generates_values() {
    let locales = [
        Locale::En,
        Locale::FrFr,
        Locale::ZhCn,
        Locale::ZhTw,
        Locale::ArSa,
        Locale::JaJp,
        Locale::PtBr,
        Locale::PtPt,
        Locale::DeDe,
        Locale::ItIt,
        Locale::CyGb,
        Locale::NlNl,
        Locale::TrTr,
        Locale::FaIr,
    ];
    for locale in locales {
        let mut faker = Faker::new(3).with_locale(locale);
        assert!(!faker.name().is_empty(), "{locale:?} name");
        assert!(!faker.city().is_empty(), "{locale:?} city");
        assert!(!faker.company_name().is_empty(), "{locale:?} company");
    }
}

/// `from_config` resolves known codes and falls back for unknown ones.
#[test]
fn from_config_resolves_and_falls_back() {
    assert_eq!(Faker::from_config(1, "en_US").locale(), Locale::En);
    assert_eq!(Faker::from_config(1, "de_DE").locale(), Locale::DeDe);
    assert_eq!(Faker::from_config(1, "fr").locale(), Locale::FrFr);
    assert_eq!(Faker::from_config(1, "unsupported").locale(), Locale::En);
    assert_eq!(Faker::from_config(1, "").locale(), Locale::En);
}

/// A `unique_*` helper never repeats across 100 draws.
#[test]
fn unique_email_never_repeats() {
    let mut faker = Faker::new(42);
    let emails: Vec<String> = (0..100)
        .map(|_| faker.unique_email().expect("unique email"))
        .collect();
    let distinct: std::collections::HashSet<&String> = emails.iter().collect();
    assert_eq!(distinct.len(), emails.len());
}

/// The `unique_*` helpers share the retry machinery for other fields.
#[test]
fn unique_name_and_username_never_repeat() {
    let mut faker = Faker::new(7);
    let names: std::collections::HashSet<String> = (0..50)
        .map(|_| faker.unique_name().expect("name"))
        .collect();
    let usernames: std::collections::HashSet<String> = (0..50)
        .map(|_| faker.unique_username().expect("username"))
        .collect();
    assert_eq!(names.len(), 50);
    assert_eq!(usernames.len(), 50);
}

/// Exhausting a tiny unique space reports `UniqueExhausted`, not a panic.
#[test]
fn unique_exhaustion_errors() {
    let mut faker = Faker::new(1).with_unique_max_attempts(3);
    // The first constant is novel, so it is accepted...
    let first = faker
        .unique_with("constant", |_| "always-the-same".to_string())
        .expect("first draw is new");
    assert_eq!(first, "always-the-same");
    // ...but every later draw collides, exhausting the budget.
    let error = faker
        .unique_with("constant", |_| "always-the-same".to_string())
        .expect_err("exhaustion must error");
    match error {
        FakerError::UniqueExhausted { field, attempts } => {
            assert_eq!(field, "constant");
            assert_eq!(attempts, 3);
        }
    }
    assert_eq!(error.code(), "faker.unique_exhausted");
}

/// `reset_unique` lets a previously seen value be produced again.
#[test]
fn reset_unique_allows_repeats() {
    let mut faker = Faker::new(1).with_unique_max_attempts(1);
    let first = faker
        .unique_with("fixed", |_| "same".to_string())
        .expect("first draw is new");
    assert!(faker.unique_with("fixed", |_| "same".to_string()).is_err());
    faker.reset_unique();
    let again = faker
        .unique_with("fixed", |_| "same".to_string())
        .expect("reset clears the seen set");
    assert_eq!(first, again);
}

/// `date_between` stays inside the requested half-open range.
#[test]
fn date_between_stays_in_range() {
    let start = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2020, 3, 1, 0, 0, 0).unwrap();
    let mut faker = Faker::new(9);
    for _ in 0..50 {
        let value = faker.date_between(start, end);
        assert!(value >= start && value < end, "{value} out of range");
    }
}

/// `boolean_with_ratio` honours the ratio extremes.
#[test]
fn boolean_with_ratio_extremes() {
    let mut faker = Faker::new(4);
    for _ in 0..64 {
        assert!(faker.boolean_with_ratio(1.0));
        assert!(!faker.boolean_with_ratio(0.0));
    }
}

/// `random_element` samples the slice and yields `None` when empty.
#[test]
fn random_element_samples_slice() {
    let mut faker = Faker::new(6);
    let items = ["alpha", "beta", "gamma"];
    for _ in 0..20 {
        let picked = faker.random_element(&items).expect("non-empty slice");
        assert!(items.contains(&picked));
    }
    assert_eq!(faker.random_element::<u8>(&[]), None);
}

/// `numerify` and `lexify` replace their placeholder characters.
#[test]
fn numerify_and_lexify_formats() {
    let mut faker = Faker::new(8);
    let numeric = faker.numerify("###-####");
    assert_eq!(numeric.len(), 8);
    assert!(numeric.chars().enumerate().all(|(i, ch)| {
        if i == 3 {
            ch == '-'
        } else {
            ch.is_ascii_digit()
        }
    }));

    let alphabetic = faker.lexify("??-??");
    assert_eq!(alphabetic.len(), 5);
    assert!(alphabetic.chars().enumerate().all(|(i, ch)| {
        if i == 2 {
            ch == '-'
        } else {
            ch.is_ascii_lowercase()
        }
    }));
}

/// `uuid` produces distinct values and is seed-reproducible.
#[test]
fn uuid_is_distinct_and_reproducible() {
    let mut a = Faker::new(123);
    let mut b = Faker::new(123);
    let first = a.uuid();
    assert_ne!(first, a.uuid());
    assert_eq!(first, b.uuid());
}

/// Text helpers honour the requested count.
#[test]
fn text_helpers_honour_count() {
    let mut faker = Faker::new(21);
    assert_eq!(faker.words(5).len(), 5);
    assert_eq!(faker.sentence(6).split_whitespace().count(), 6);
    assert_eq!(faker.paragraph(3).lines().count(), 3);
}
