//! Generated package manifest and RustaSea application manifest.
//!
//! The `Cargo.toml` template is the one file whose *dependency features* differ
//! per variant, so each kit gets its own manifest string. Everything else in
//! the tree is byte-identical across variants.

use crate::variant::StarterKitVariant;

use super::TemplateFile;

/// Manifest templates for `variant`.
pub fn entries(variant: StarterKitVariant) -> Vec<TemplateFile> {
    vec![
        ("Cargo.toml", cargo_toml(variant)),
        ("rustasea.toml", RUSTASEA_TOML),
    ]
}

/// Select the variant-specific `Cargo.toml` template.
fn cargo_toml(variant: StarterKitVariant) -> &'static str {
    match variant {
        StarterKitVariant::Blade => BLADE_CARGO,
        StarterKitVariant::React => REACT_CARGO,
        StarterKitVariant::Vue => VUE_CARGO,
        StarterKitVariant::Livewire => LIVEWIRE_CARGO,
    }
}

const BLADE_CARGO: &str = r##"[package]
name = "@@app_name@@"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
description = "@@app_pascal@@ — a RustaSea blade starter kit"

[lib]
name = "@@app_snake@@"
path = "lib.rs"

[[bin]]
name = "@@app_name@@"
path = "main.rs"

# `tests/feature/mod.rs` and `tests/unit/mod.rs` are the suite crate roots;
# cargo does not auto-discover `tests/<dir>/mod.rs`, so both targets are
# declared explicitly.
[[test]]
name = "feature"
path = "tests/feature/mod.rs"

[[test]]
name = "unit"
path = "tests/unit/mod.rs"

[dependencies]
rustasea = { version = "0.1", features = ["view", "action"] }
rustasea-view = "0.1"
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# The generated session guard is generic over any `tower-sessions` store, and
# `app/actions/auth/attempt_to_authenticate.rs` names the `SessionStore` trait
# directly; the app therefore declares the dependency itself.
tower-sessions = "0.15"

[features]
default = []

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt"] }
# `tower::ServiceExt::oneshot` drives the route-table smoke tests.
tower = { version = "0.5", features = ["util"] }
"##;

const REACT_CARGO: &str = r##"[package]
name = "@@app_name@@"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
description = "@@app_pascal@@ — a RustaSea react (Dioxus + Inertia) starter kit"

[lib]
name = "@@app_snake@@"
path = "lib.rs"

[[bin]]
name = "@@app_name@@"
path = "main.rs"

# `tests/feature/mod.rs` and `tests/unit/mod.rs` are the suite crate roots;
# cargo does not auto-discover `tests/<dir>/mod.rs`, so both targets are
# declared explicitly.
[[test]]
name = "feature"
path = "tests/feature/mod.rs"

[[test]]
name = "unit"
path = "tests/unit/mod.rs"

[dependencies]
rustasea = { version = "0.1", features = ["inertia", "wasm-dioxus", "action"] }
rustasea-inertia = "0.1"
rustasea-inertia-adapters = { version = "0.1", features = ["react"] }
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# The generated session guard is generic over any `tower-sessions` store, and
# `app/actions/auth/attempt_to_authenticate.rs` names the `SessionStore` trait
# directly; the app therefore declares the dependency itself.
tower-sessions = "0.15"

[features]
default = []

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt"] }
# `tower::ServiceExt::oneshot` drives the route-table smoke tests.
tower = { version = "0.5", features = ["util"] }
"##;

const VUE_CARGO: &str = r##"[package]
name = "@@app_name@@"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
description = "@@app_pascal@@ — a RustaSea vue (Leptos + Inertia) starter kit"

[lib]
name = "@@app_snake@@"
path = "lib.rs"

[[bin]]
name = "@@app_name@@"
path = "main.rs"

# `tests/feature/mod.rs` and `tests/unit/mod.rs` are the suite crate roots;
# cargo does not auto-discover `tests/<dir>/mod.rs`, so both targets are
# declared explicitly.
[[test]]
name = "feature"
path = "tests/feature/mod.rs"

[[test]]
name = "unit"
path = "tests/unit/mod.rs"

[dependencies]
rustasea = { version = "0.1", features = ["inertia", "wasm-leptos", "action"] }
rustasea-inertia = "0.1"
rustasea-inertia-adapters = { version = "0.1", features = ["vue"] }
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# The generated session guard is generic over any `tower-sessions` store, and
# `app/actions/auth/attempt_to_authenticate.rs` names the `SessionStore` trait
# directly; the app therefore declares the dependency itself.
tower-sessions = "0.15"

[features]
default = []

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt"] }
# `tower::ServiceExt::oneshot` drives the route-table smoke tests.
tower = { version = "0.5", features = ["util"] }
"##;

const LIVEWIRE_CARGO: &str = r##"[package]
name = "@@app_name@@"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
description = "@@app_pascal@@ — a RustaSea livewire (askama + HTMX) starter kit"

[lib]
name = "@@app_snake@@"
path = "lib.rs"

[[bin]]
name = "@@app_name@@"
path = "main.rs"

# `tests/feature/mod.rs` and `tests/unit/mod.rs` are the suite crate roots;
# cargo does not auto-discover `tests/<dir>/mod.rs`, so both targets are
# declared explicitly.
[[test]]
name = "feature"
path = "tests/feature/mod.rs"

[[test]]
name = "unit"
path = "tests/unit/mod.rs"

[dependencies]
# `broadcast` is re-exported by `rustasea` unconditionally, so there is no
# `broadcast` cargo feature to enable (requesting one fails dependency
# resolution); the livewire kit only needs the `view` feature.
rustasea = { version = "0.1", features = ["view", "action"] }
rustasea-view = "0.1"
rustasea-livewire = "0.1"
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# The generated session guard is generic over any `tower-sessions` store, and
# `app/actions/auth/attempt_to_authenticate.rs` names the `SessionStore` trait
# directly; the app therefore declares the dependency itself.
tower-sessions = "0.15"

[features]
default = []

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt"] }
# `tower::ServiceExt::oneshot` drives the route-table smoke tests.
tower = { version = "0.5", features = ["util"] }
"##;

const RUSTASEA_TOML: &str = r##"# Application manifest consumed by RustaSea tooling.
# Generated by `cargo rustasea new`; safe to edit.
[app]
name = "@@app_name@@"
variant = "@@variant@@"
msrv = "1.88"
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_manifest_carries_variant_features() {
        let blade = cargo_toml(StarterKitVariant::Blade);
        assert!(blade.contains("features = [\"view\", \"action\"]"));
        let react = cargo_toml(StarterKitVariant::React);
        assert!(react.contains("wasm-dioxus"));
        assert!(react.contains("features = [\"inertia\", \"wasm-dioxus\", \"action\"]"));
        let vue = cargo_toml(StarterKitVariant::Vue);
        assert!(vue.contains("wasm-leptos"));
        assert!(vue.contains("features = [\"inertia\", \"wasm-leptos\", \"action\"]"));
        let livewire = cargo_toml(StarterKitVariant::Livewire);
        assert!(livewire.contains("features = [\"view\", \"action\"]"));
        assert!(!livewire.contains("\"view\", \"broadcast\""));
    }

    #[test]
    fn every_manifest_has_the_shared_tail() {
        for variant in StarterKitVariant::ALL {
            let manifest = cargo_toml(variant);
            assert_eq!(manifest.matches("serde_json = \"1\"").count(), 1);
            assert!(manifest.contains("[dev-dependencies]"));
        }
    }
}
