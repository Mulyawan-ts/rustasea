//! Controller-shape integration tests (TASK-092, Hypervel parity).
//!
//! Adopts the Hypervel 0.4 skeleton convention into the *generated* app: the
//! tree ships an `app/http/controllers/controller.rs` base controller (the
//! `app/Http/Controllers/Controller.php` analogue) and every scaffolded
//! controller is a struct implementing the framework `Controller` trait instead
//! of a bare free-function module.
//!
//! These are fast, string-level guards; `tests/generated_compiles.rs` is the
//! authoritative oracle that proves the emitted `impl Controller` + associated
//! function calls actually type-check against the workspace crates.

use rustasea_scaffold::{RenderedFile, Scaffold, StarterKitVariant};

/// The generated controller modules that must adopt the struct + trait shape.
const CONTROLLER_MODULES: &[&str] = &[
    "app/http/controllers/auth_controller.rs",
    "app/http/controllers/dashboard_controller.rs",
    "app/http/controllers/settings/profile_controller.rs",
    "app/http/controllers/settings/password_controller.rs",
    "app/http/controllers/settings/security_controller.rs",
];

/// The base controller is emitted and re-exports the framework trait.
#[test]
fn base_controller_re_exports_the_framework_trait() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let base = find(&files, "app/http/controllers/controller.rs", variant);
        assert!(
            base.contains("pub use rustasea::http::Controller;"),
            "base controller must re-export the framework trait ({variant})"
        );
    }
}

/// Every scaffolded controller is a struct implementing the base trait.
#[test]
fn every_controller_implements_the_base_trait() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        for module in CONTROLLER_MODULES {
            let body = find(&files, module, variant);
            assert!(
                body.contains("pub struct"),
                "{module} must declare a controller struct ({variant})"
            );
            assert!(
                body.contains("impl Controller for"),
                "{module} must `impl Controller for` the base trait ({variant})"
            );
            assert!(
                body.contains("use crate::app::http::controllers::controller::Controller;"),
                "{module} must import the base trait through the app path ({variant})"
            );
            assert!(
                !body.contains("\npub async fn"),
                "{module} must expose actions as associated functions, not free fns ({variant})"
            );
        }
    }
}

/// Look up a rendered file's contents, panicking with context when absent.
fn find<'a>(files: &'a [RenderedFile], path: &str, variant: StarterKitVariant) -> &'a str {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing {path} ({variant})"))
        .contents
        .as_str()
}
