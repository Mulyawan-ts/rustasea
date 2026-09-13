//! RustaSea testing harness (FS-M5-04, FR-507..509).
//!
//! A `TestCase` implementor provisions isolated stores for one test binary;
//! factory sequences reset between tests so parallel suites never observe a
//! leaked `Str` counter; paginators render `bootstrap-3` views.
//!
//! Feature `containers` (default) wires the Postgres/Redis container helpers
//! (docker-backed) with a 30s health timeout; disabling it keeps the crate
//! compiling with a thin dependency tree for pure logic tests.
//!
//! Feature `auth` adds reusable session-guard helpers (`login_as`,
//! `assert_authenticated`, `assert_guest`) and the Fortify feature gate
//! (`fortify_feature_enabled`); feature `http` adds [`request_json`], a
//! tower-oneshot JSON client for `axum::Router` feature tests.

#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "containers")]
pub mod containers;
pub mod error;
pub mod factory;
#[cfg(feature = "postgres")]
pub mod fixtures;
#[cfg(feature = "http")]
pub mod http;
pub mod migration;
pub mod paginator;
pub mod test_case;

#[cfg(feature = "auth")]
pub use auth::{
    assert_authenticated, assert_guest, fortify_feature_enabled, login_as, FortifyFeature,
};
#[cfg(feature = "containers")]
pub use containers::{
    postgres_container, redis_container, teardown_all, ContainerError, ContainerHandle,
};
pub use error::{Result, TestError};
pub use factory::{
    factory_registry, register_sequence, reset_factory_sequences, str_factory, StrFactory,
};
#[cfg(feature = "postgres")]
pub use fixtures::{FixtureError, PostgresTestDb};
#[cfg(feature = "http")]
pub use http::request_json;
pub use migration::{migrate_once, MigrateHarness};
pub use paginator::{bootstrap_3, paginator_view};
pub use test_case::{TestCase, TestConfig};
