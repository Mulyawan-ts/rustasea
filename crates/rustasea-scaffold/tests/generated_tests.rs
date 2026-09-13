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

/// The generated route tables use the `rustasea::router` DSL (RTE-003).
///
/// Mirrors the kit route conventions: every route is named, `/settings`
/// redirects to the profile screen, the security screen is registered, and the
/// confirm-password screen is exposed. The tables are built with the expressive
/// router (`named` / `redirect` / `middleware`) and compiled with
/// `try_into_axum_router`, so the generated app boots without an
/// `UnknownMiddleware` failure.
#[test]
fn generated_route_tables_use_the_router_dsl() {
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

        // Router table root: DSL build + the middleware ids the tables declare.
        let routes_mod = find("routes/mod.rs");
        assert!(routes_mod.contains("use rustasea::router::Router;"));
        assert!(routes_mod.contains("try_into_axum_router()"));
        for id in ["auth", "verified", "password.confirm"] {
            assert!(
                routes_mod.contains(&format!("register_middleware(\"{id}\"")),
                "routes/mod.rs must register the `{id}` middleware id ({variant})"
            );
        }

        // Web: named home + auth/verified dashboard.
        let web = find("routes/web.rs");
        assert!(web.contains(".named(\"home\")"));
        assert!(web.contains(".named(\"dashboard\")"));
        assert!(web.contains(".middleware(\"auth\")"));
        assert!(web.contains(".middleware(\"verified\")"));

        // Auth: named login/logout/register + confirm-password.
        let auth = find("routes/auth.rs");
        for name in ["login", "logout", "register", "password.confirm"] {
            assert!(
                auth.contains(&format!(".named(\"{name}\")")),
                "routes/auth.rs must name `{name}` ({variant})"
            );
        }
        assert!(auth.contains("/confirm-password"));

        // Settings: redirect + named screens + password.confirm on security.
        let settings = find("routes/settings.rs");
        assert!(settings.contains(".redirect(\"/settings\", \"/settings/profile\")"));
        for name in ["profile.edit", "password.edit", "security.edit"] {
            assert!(
                settings.contains(&format!(".named(\"{name}\")")),
                "routes/settings.rs must name `{name}` ({variant})"
            );
        }
        assert!(settings.contains("/settings/security"));
        assert!(settings.contains(".middleware(\"password.confirm\")"));

        // FIX-RTE-01 regression: the password PUT route must bind to the real
        // controller action, never an unresolved `password_update` identifier.
        assert!(
            settings.contains(".put_action(\"/settings/password\", password_controller::update)"),
            "routes/settings.rs must bind PUT /settings/password to \
             password_controller::update ({variant})"
        );
        assert!(
            !settings.contains("password_update,"),
            "routes/settings.rs must not reference the unresolved `password_update` \
             identifier ({variant})"
        );
    }
}

/// The generated `users` schema carries every auth column the kit defines.
///
/// `laravel/livewire-starter-kit`'s `app/Models/User.php` is the schema
/// contract; the generated migration, `User` model, and `UserFactory` must all
/// agree on the full column set, and the four secret columns must never be
/// serialized (the kit's `#[Hidden([...])]` analogue). This is a fast,
/// string-level companion to the real `cargo check` gate in
/// `tests/generated_compiles.rs`, which proves the added fields actually
/// type-check against `rustasea::orm`.
#[test]
fn generated_user_schema_carries_the_auth_columns() {
    // Columns added by AUTH-003 on top of the original six.
    let columns = [
        "two_factor_secret",
        "two_factor_recovery_codes",
        "two_factor_confirmed_at",
        "remember_token",
    ];
    // Fields the model must hide from serialization (the kit's `Hidden`).
    let hidden = [
        "password",
        "two_factor_secret",
        "two_factor_recovery_codes",
        "remember_token",
    ];

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

        // Migration: every auth column, plus the pre-existing constraints.
        let migration = find("database/migrations/create_users.rs");
        for column in columns {
            assert!(
                migration.contains(column),
                "CreateUsers migration must declare `{column}` ({variant})"
            );
        }
        assert!(
            migration.contains("email VARCHAR(255) NOT NULL UNIQUE"),
            "CreateUsers migration must keep the email UNIQUE constraint ({variant})"
        );
        assert!(
            migration.contains("deleted_at TIMESTAMPTZ NULL"),
            "CreateUsers migration must keep the soft-delete column ({variant})"
        );

        // Model: the four fields, typed as `Option<…>`, with the secrets skipped.
        let model = find("app/models/user.rs");
        for column in columns {
            assert!(
                model.contains(&format!("pub {column}: Option<")),
                "User model must declare `pub {column}: Option<…>` ({variant})"
            );
        }
        for field in hidden {
            assert!(
                model.contains(&format!("#[serde(skip_serializing)]\n    pub {field}")),
                "User model must `#[serde(skip_serializing)]` the `{field}` field ({variant})"
            );
        }

        // Factory: the new fields default to `None` so factory rows match.
        let factory = find("database/factories/user_factory.rs");
        for column in columns {
            assert!(
                factory.contains(&format!("{column}: None,")),
                "UserFactory must default `{column}` to `None` ({variant})"
            );
        }
    }
}
