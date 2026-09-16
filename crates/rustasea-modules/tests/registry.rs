//! Integration tests for the module registry: deterministic ordering, manifest
//! filtering, and real HTTP reachability of module routes.

use std::sync::{Arc, Mutex};

use rustasea_foundation::{Application, ServiceProvider};
use rustasea_modules::{Module, ModuleManifest, ModuleRegistry};
use rustasea_orm::Migration;
use rustasea_router::Router;
use tower::ServiceExt;

/// Shared call log proving provider lifecycle order.
type CallLog = Arc<Mutex<Vec<String>>>;

/// A provider that records its `register`/`boot` invocations.
struct RecordingProvider {
    label: &'static str,
    log: CallLog,
}

impl ServiceProvider for RecordingProvider {
    fn name(&self) -> &'static str {
        self.label
    }

    fn register(&self, _app: &mut Application) {
        self.log
            .lock()
            .expect("log")
            .push(format!("{}.register", self.label));
    }

    fn boot(&self, _app: &Application) {
        self.log
            .lock()
            .expect("log")
            .push(format!("{}.boot", self.label));
    }
}

/// A migration that records its name and emits a recognisable body.
struct TestMigration {
    name: String,
    log: CallLog,
}

impl Migration for TestMigration {
    fn name(&self) -> &str {
        &self.name
    }

    fn up(&self) -> rustasea_orm::Result<String> {
        self.log
            .lock()
            .expect("log")
            .push(format!("{}.up", self.name));
        Ok(format!("SELECT '{}';", self.name))
    }
}

/// A module contributing one route, one provider, and one migration.
struct TestModule {
    name: &'static str,
    route: &'static str,
    log: CallLog,
}

impl TestModule {
    /// Build a module named `name` mounting `route`.
    fn new(name: &'static str, route: &'static str, log: &CallLog) -> Self {
        Self {
            name,
            route,
            log: Arc::clone(log),
        }
    }
}

impl Module for TestModule {
    fn name(&self) -> &str {
        self.name
    }

    fn routes(&self, router: &mut Router) {
        router.get(self.route);
    }

    fn providers(&self) -> Vec<Box<dyn ServiceProvider>> {
        vec![Box::new(RecordingProvider {
            label: self.name,
            log: Arc::clone(&self.log),
        })]
    }

    fn migrations(&self) -> Vec<Box<dyn Migration>> {
        vec![Box::new(TestMigration {
            name: format!("{}_0001_create_table", self.name),
            log: Arc::clone(&self.log),
        })]
    }
}

/// Build a registry with `beta` registered before `alpha` (reverse order).
fn reverse_registry(log: &CallLog) -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    registry
        .register(Arc::new(TestModule::new("beta", "/beta", log)))
        .expect("register beta");
    registry
        .register(Arc::new(TestModule::new("alpha", "/alpha", log)))
        .expect("register alpha");
    registry
}

/// Registration and mounting are alphabetical regardless of insertion order.
#[test]
fn registry_registers_routes_providers_and_migrations_in_order() {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let registry = reverse_registry(&log);

    assert_eq!(registry.names(), vec!["alpha", "beta"]);
    assert_eq!(registry.len(), 2);
    assert!(!registry.is_empty());

    let mut router = Router::new();
    registry.routes(&mut router);
    let paths: Vec<String> = router
        .get_routes()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    assert_eq!(paths, vec!["/alpha", "/beta"]);

    let providers = registry.providers();
    let provider_names: Vec<&str> = providers.iter().map(|provider| provider.name()).collect();
    assert_eq!(provider_names, vec!["alpha", "beta"]);

    assert_eq!(
        registry.migrator().names(),
        vec!["alpha_0001_create_table", "beta_0001_create_table"]
    );
}

/// The providers installed into the application boot DAG keep module order.
#[test]
fn install_boots_providers_in_module_order() {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let registry = reverse_registry(&log);

    let mut app = Application::new();
    registry.install(&mut app);
    app.boot().expect("boot application");

    assert_eq!(
        *log.lock().expect("log"),
        vec!["alpha.register", "beta.register", "alpha.boot", "beta.boot"]
    );
}

/// A disabled module contributes no routes, providers, or migrations.
#[test]
fn disabled_module_is_excluded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest_path = dir.path().join("modules.toml");
    std::fs::write(&manifest_path, "[modules]\ndisabled = [\"beta\"]\n").expect("seed manifest");
    let manifest = ModuleManifest::load(&manifest_path).expect("load manifest");

    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut registry = reverse_registry(&log);
    registry.apply_manifest(&manifest);

    assert_eq!(registry.enabled_names(), vec!["alpha"]);
    assert_eq!(registry.disabled_names(), vec!["beta"]);
    assert!(!registry.get("beta").expect("beta").is_enabled());

    let mut router = Router::new();
    registry.routes(&mut router);
    let paths: Vec<String> = router
        .get_routes()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    assert_eq!(paths, vec!["/alpha"]);
    assert_eq!(registry.providers().len(), 1);
    assert_eq!(registry.migrator().names(), vec!["alpha_0001_create_table"]);

    // In-memory toggling must not resurrect the disabled module incorrectly.
    registry.enable("beta").expect("enable beta");
    assert_eq!(registry.enabled_names(), vec!["alpha", "beta"]);
}

/// Unknown names are rejected by `enable`/`disable`.
#[test]
fn toggling_unknown_module_errors() {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut registry = reverse_registry(&log);
    let error = registry.disable("ghost").expect_err("unknown module");
    assert!(matches!(
        error,
        rustasea_modules::ModuleError::Unknown { name } if name == "ghost"
    ));
}

/// Re-registering the same module name is rejected.
#[test]
fn duplicate_module_name_is_rejected() {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut registry = reverse_registry(&log);
    let error = registry
        .register(Arc::new(TestModule::new("alpha", "/alpha-2", &log)))
        .expect_err("duplicate module");
    assert!(matches!(
        error,
        rustasea_modules::ModuleError::Duplicate { name } if name == "alpha"
    ));
    assert_eq!(registry.len(), 2);
}

/// An enabled module's route answers over real Axum dispatch; a disabled
/// module's route is absent (404).
#[tokio::test]
async fn module_routes_are_reachable_only_when_enabled() {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let mut registry = reverse_registry(&log);
    registry.disable("beta").expect("disable beta");

    let mut router = Router::new();
    registry.routes(&mut router);
    let app = router.into_axum_router_owned();

    let enabled = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/alpha")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("dispatch /alpha");
    assert_eq!(enabled.status(), axum::http::StatusCode::OK);

    let disabled = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/beta")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("dispatch /beta");
    assert_eq!(disabled.status(), axum::http::StatusCode::NOT_FOUND);
}
