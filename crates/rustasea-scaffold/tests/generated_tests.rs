//! Integration tests for the generated test suites (TST-003).
//!
//! Verifies that the scaffolder's `tests/` tree mirrors
//! `laravel/livewire-starter-kit`: the Fortify feature gate, the session-guard
//! lifecycle suite, the route-table smoke suite, and the profile-rules unit
//! suite — plus the `Cargo.toml` wiring that makes `tests/<suite>/mod.rs`
//! crate roots actually run under cargo.

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// Render every file for `variant` without touching the filesystem.
fn render(variant: StarterKitVariant) -> Vec<rustasea_scaffold::RenderedFile> {
    Scaffold::new("my-app", variant).render().expect("render")
}

/// The generated test suites are wired as explicit `[[test]]` targets.
///
/// Cargo only auto-discovers `tests/*.rs` and `tests/*/main.rs`, so a bare
/// `tests/feature/mod.rs` (the Laravel-style directory root) would be silently
/// ignored. Every variant must therefore declare both suite targets and add the
/// `tower` dev-dependency the route smoke test needs.
#[test]
fn generated_manifest_wires_the_test_targets() {
    for variant in StarterKitVariant::ALL {
        let files = render(variant);
        let manifest = files
            .iter()
            .find(|file| file.path == "Cargo.toml")
            .expect("Cargo.toml present")
            .contents
            .as_str();
        assert!(
            manifest.contains("[[test]]\nname = \"feature\"\npath = \"tests/feature/mod.rs\""),
            "manifest must wire the feature test target ({variant})"
        );
        assert!(
            manifest.contains("[[test]]\nname = \"unit\"\npath = \"tests/unit/mod.rs\""),
            "manifest must wire the unit test target ({variant})"
        );
        assert!(
            manifest.contains("tower = { version = \"0.5\", features = [\"util\"] }"),
            "manifest must add the tower dev-dependency ({variant})"
        );
    }
}

/// The generated `tests/` tree carries the kit-parity suites.
#[test]
fn generated_feature_tests_mirror_the_kit() {
    for variant in StarterKitVariant::ALL {
        let files = render(variant);
        let find = |path: &str| {
            files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("missing {path} ({variant})"))
                .contents
                .as_str()
        };

        // Feature crate root: fortify gate + both suite modules.
        let feature_mod = find("tests/feature/mod.rs");
        assert!(feature_mod.contains("pub mod auth_test;"));
        assert!(feature_mod.contains("pub mod routes_test;"));
        assert!(feature_mod.contains("pub fn fortify_feature_enabled("));
        assert!(feature_mod.contains("rustasea::config::ConfigLoader"));
        assert!(feature_mod.contains("macro_rules! skip_unless_fortify_has"));

        // Auth suite: validation + real SessionGuard lifecycle, no `todo!()` refs.
        let auth_test = find("tests/feature/auth_test.rs");
        assert!(auth_test.contains("password_validation_rules"));
        assert!(auth_test.contains("SessionGuard"));
        assert!(auth_test.contains("MemoryUserRegistry"));
        assert!(auth_test.contains("skip_unless_fortify_has!(\"registration\")"));
        assert!(!auth_test.contains("todo!"));

        // Routes smoke suite: real router dispatch, no source-text assertions.
        let routes_test = find("tests/feature/routes_test.rs");
        assert!(routes_test.contains("routes::router("));
        assert!(routes_test.contains("StatusCode::METHOD_NOT_ALLOWED"));
        assert!(routes_test.contains("StatusCode::NOT_FOUND"));
        assert!(!routes_test.contains("todo!"));

        // Unit suite: profile validation rules.
        let actions_test = find("tests/unit/actions_test.rs");
        assert!(actions_test.contains("profile_validation_rules"));
        assert!(actions_test.contains("validate_name"));
    }
}
