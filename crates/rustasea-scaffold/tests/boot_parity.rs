//! Generated-app boot-parity and tooling tests (TASK-096).
//!
//! The scaffolder must wire the generated application's boot so `cargo artisan
//! migrate` sees a real migration registry (framework queue migrations plus the
//! app's own nine migrations), and it must ship the framework's formatting
//! contract (`rustfmt.toml`) plus a minimal CI workflow. These are string
//! assertions over the rendered tree; `tests/generated_compiles.rs` is the
//! authoritative compile gate for the same output.

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// The nine application migrations the generated boot must register, in the
/// order the generated `bootstrap/commands.rs` registers them.
const APP_MIGRATIONS: &[&str] = &[
    "CreateUsers",
    "CreateSessions",
    "CreatePasswordResetTokens",
    "CreateRoles",
    "CreatePermissions",
    "CreateRoleUser",
    "CreatePermissionRole",
    "CreateAuditLog",
    "CreateAuthenticationLog",
];

/// Find a rendered file by path, panicking when it is absent.
fn find<'a>(files: &'a [rustasea_scaffold::RenderedFile], path: &str, variant: &str) -> &'a str {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing {path} ({variant})"))
        .contents
        .as_str()
}

/// The generated `bootstrap/commands.rs` registers the framework command surface
/// and every application migration, and `bootstrap/app.rs` calls it at boot.
#[test]
fn generated_bootstrap_registers_framework_and_app_migrations() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let commands = find(&files, "bootstrap/commands.rs", variant.as_str());

        // Framework command surface (which also registers the queue migrations).
        assert!(
            commands.contains("rustasea::cli::load_default_commands()"),
            "bootstrap/commands.rs must call `rustasea::cli::load_default_commands()` ({variant})"
        );

        // The app's own nine migrations, registered through the ORM registry.
        assert!(
            commands.contains("use rustasea::orm::register_migration;"),
            "bootstrap/commands.rs must import `rustasea::orm::register_migration` ({variant})"
        );
        for migration in APP_MIGRATIONS {
            assert!(
                commands.contains(&format!("register_migration({migration});")),
                "bootstrap/commands.rs must register {migration} ({variant})"
            );
        }

        // The migration registration is guarded so a second boot is a no-op.
        assert!(
            commands.contains("OnceLock"),
            "bootstrap/commands.rs must guard registration against double-boot ({variant})"
        );

        // `configure` must invoke the registration before booting the DAG.
        let app = find(&files, "bootstrap/app.rs", variant.as_str());
        assert!(
            app.contains("commands::register_default();"),
            "bootstrap/app.rs must call `commands::register_default()` ({variant})"
        );
    }
}

/// Every generated migration the boot registers exists as a real module and
/// struct, so the `use crate::database::migrations::…` imports resolve.
#[test]
fn registered_migrations_exist_in_the_generated_tree() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");

        for module in [
            "create_users",
            "create_sessions",
            "create_password_reset_tokens",
            "create_roles",
            "create_permissions",
            "create_role_user",
            "create_permission_role",
            "create_audit_log",
            "create_authentication_log",
        ] {
            let path = format!("database/migrations/{module}.rs");
            assert!(
                files.iter().any(|file| file.path == path),
                "missing migration module {path} ({variant})"
            );
        }

        let migrations_mod = find(&files, "database/migrations/mod.rs", variant.as_str());
        for module in [
            "create_users",
            "create_sessions",
            "create_password_reset_tokens",
            "create_roles",
            "create_permissions",
            "create_role_user",
            "create_permission_role",
            "create_audit_log",
            "create_authentication_log",
        ] {
            assert!(
                migrations_mod.contains(&format!("pub mod {module};")),
                "database/migrations/mod.rs must declare `pub mod {module};` ({variant})"
            );
        }
    }
}

/// The generated `rustfmt.toml` mirrors the framework's formatting contract.
#[test]
fn generated_rustfmt_matches_framework_contract() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let rustfmt = find(&files, "rustfmt.toml", variant.as_str());

        assert!(
            rustfmt.contains("edition = \"2021\""),
            "rustfmt.toml must pin edition 2021 ({variant})"
        );
        assert!(
            rustfmt.contains("max_width = 100"),
            "rustfmt.toml must pin max_width = 100 ({variant})"
        );
        assert!(
            rustfmt.contains("tab_spaces = 4"),
            "rustfmt.toml must pin tab_spaces = 4 ({variant})"
        );
    }
}

/// The generated CI workflow runs fmt, clippy (`-D warnings`), and tests, and
/// stays free of workspace-only tooling the generated app does not carry.
#[test]
fn generated_ci_workflow_is_minimal_and_generic() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let workflow = find(&files, ".github/workflows/ci.yml", variant.as_str());

        assert!(
            workflow.contains("cargo fmt --all -- --check"),
            "ci.yml must run `cargo fmt --all -- --check` ({variant})"
        );
        assert!(
            workflow.contains("cargo clippy --all-targets -- -D warnings"),
            "ci.yml must run clippy with `-D warnings` ({variant})"
        );
        assert!(
            workflow.contains("cargo test"),
            "ci.yml must run `cargo test` ({variant})"
        );
        assert!(
            workflow.contains("dtolnay/rust-toolchain@stable"),
            "ci.yml must install the stable toolchain ({variant})"
        );

        // No workspace-only tooling: the generated app has no `xtask`, `deny.toml`,
        // or audit configuration.
        for forbidden in ["cargo xtask", "cargo deny", "cargo audit"] {
            assert!(
                !workflow.contains(forbidden),
                "ci.yml must not reference `{forbidden}` ({variant})"
            );
        }
    }
}
