//! Tinker source — publishes application introspection to the `tinker` REPL.
//!
//! `rustasea-cli` is framework-generic and must not depend on this app crate,
//! so the app pushes an [`AppTinkerSource`] into the CLI's process-wide tinker
//! registry at boot (mirroring the route-source publication in
//! [`crate::bootstrap::app`]). The REPL reads it back to inspect configuration
//! and the container.
//!
//! The source captures what is introspectable without sharing the live
//! [`Container`](rustasea::foundation::Container) — which is not `Clone` and
//! stays owned by the booted [`Application`](rustasea::Application): the
//! boot-time [`ConfigLoader`], the resolved environment, and a snapshot of the
//! container's binding keys with a best-effort summary for each.

use std::collections::BTreeMap;
use std::sync::Arc;

use rustasea::cli::TinkerSource;
use rustasea::config::ConfigLoader;
use rustasea::foundation::Container;

/// Application-backed [`TinkerSource`] over a booted application.
///
/// Built by [`AppTinkerSource::from_booted`] once the app has booted, so the
/// container bindings and config loader are fully populated.
pub struct AppTinkerSource {
    /// Boot-time config loader (the single source of truth for configuration).
    loader: Arc<ConfigLoader>,
    /// Resolved environment name.
    environment: String,
    /// Container binding keys, sorted for deterministic output.
    keys: Vec<String>,
    /// Best-effort per-key summary (binding kind / resolved value).
    entries: BTreeMap<String, String>,
}

impl AppTinkerSource {
    /// Capture a source snapshot from a booted [`Container`].
    ///
    /// `loader` is the boot-time config loader; `environment` the resolved
    /// environment name. The container's keys are snapshotted and summarised:
    /// well-known bindings get a typed summary, unknown ones are reported as
    /// bound (the container exposes no generic value formatting).
    pub fn from_booted(
        loader: Arc<ConfigLoader>,
        environment: String,
        container: &Container,
    ) -> Self {
        let mut keys: Vec<String> = container.keys();
        keys.sort();
        let mut entries = BTreeMap::new();
        for key in &keys {
            entries.insert(key.clone(), summarise(key, container));
        }
        Self {
            loader,
            environment,
            keys,
            entries,
        }
    }
}

impl TinkerSource for AppTinkerSource {
    /// Resolve a configuration key as a JSON-rendered value.
    fn config(&self, key: &str) -> Option<String> {
        let value = self.loader.get_key::<serde_json::Value>(key).ok()?;
        serde_json::to_string(&value).ok()
    }

    /// List the snapshotted container keys.
    fn container_keys(&self) -> Vec<String> {
        self.keys.clone()
    }

    /// Summarise one container entry from the boot-time snapshot.
    fn container_entry(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    /// The resolved application environment.
    fn environment(&self) -> Option<String> {
        Some(self.environment.clone())
    }
}

/// Summarise one container binding from the values the app can downcast.
///
/// The container exposes typed `get::<T>` lookups but no generic value
/// formatting, so only the well-known framework bindings are described in
/// detail; every other bound key reports as `bound`.
fn summarise(key: &str, container: &Container) -> String {
    use rustasea::auth::{SessionGuard, UserProvider};

    match key {
        "config.loader" => "Arc<ConfigLoader>".to_string(),
        "app.environment" => container
            .get::<String>("app.environment")
            .map(|env| format!("String(\"{env}\")"))
            .unwrap_or_else(|| "String".to_string()),
        "app.password_policy" => "PasswordPolicy".to_string(),
        "app.prohibits_destructive_commands" => container
            .get::<bool>("app.prohibits_destructive_commands")
            .map(|flag| format!("bool({flag})"))
            .unwrap_or_else(|| "bool".to_string()),
        "auth.session_guard" => match container.get::<Arc<SessionGuard>>(key) {
            Some(_) => "Arc<SessionGuard>".to_string(),
            None => "Arc<SessionGuard> (unresolved)".to_string(),
        },
        "auth.user_provider" => match container.get::<Arc<dyn UserProvider>>(key) {
            Some(_) => "Arc<dyn UserProvider>".to_string(),
            None => "Arc<dyn UserProvider> (unresolved)".to_string(),
        },
        _ => "bound".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustasea::foundation::CONFIG_LOADER_KEY;

    /// Build a container with a config loader + environment binding.
    fn container() -> (Arc<ConfigLoader>, Container) {
        let loader = Arc::new(ConfigLoader::load_from(&["config/app"]).expect("load config"));
        let mut container = Container::new();
        container.instance(CONFIG_LOADER_KEY, Arc::clone(&loader));
        container.instance("app.environment", "local".to_string());
        (loader, container)
    }

    /// Config lookup renders the value as JSON.
    #[test]
    fn config_renders_json() {
        let (loader, container) = container();
        let source = AppTinkerSource::from_booted(loader, "local".to_string(), &container);
        // `app_env` is always resolvable (defaulted) — it must render as a JSON
        // string, never panic.
        let rendered = source.config("app_env").expect("app_env resolves");
        assert!(
            rendered.starts_with('"'),
            "expected JSON string: {rendered}"
        );
    }

    /// Container keys and summaries reflect the boot-time bindings.
    #[test]
    fn container_keys_and_entries() {
        let (loader, container) = container();
        let source = AppTinkerSource::from_booted(loader, "local".to_string(), &container);

        assert!(source
            .container_keys()
            .contains(&CONFIG_LOADER_KEY.to_string()));
        assert_eq!(
            source.container_entry(CONFIG_LOADER_KEY).as_deref(),
            Some("Arc<ConfigLoader>")
        );
        assert_eq!(
            source.container_entry("app.environment").as_deref(),
            Some("String(\"local\")")
        );
        assert!(source.container_entry("does.not.exist").is_none());
        assert_eq!(source.environment().as_deref(), Some("local"));
    }
}
