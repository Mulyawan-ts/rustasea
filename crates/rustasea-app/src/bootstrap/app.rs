//! Application bootstrap — `Application::configure` for the runnable app.
//!
//! Mirrors Laravel's `bootstrap/app.php` (README "Proposed Directory
//! Structure"): configures the foundation container, registers the providers
//! from [`crate::bootstrap::providers`], registers the console command
//! surface, and boots the register→boot DAG before the HTTP kernel takes
//! over. `src/main.rs` calls [`configure`].

use rustasea::foundation::BootError;
use rustasea::Application;

use crate::bootstrap::{commands, providers};

/// Build and boot the application.
///
/// Returns [`BootError`] when the config loader cannot be built or the provider
/// graph contains a cycle or an unresolved dependency, so a misconfigured boot
/// never starts the server. [`Application::boot`] mounts the layered
/// [`ConfigLoader`](rustasea::ConfigLoader) (from `config/*.toml` + environment)
/// into the container before providers register, so they resolve configuration
/// from `app.container` instead of loading it themselves. Registering the
/// default command surface also registers the framework's queue migrations.
pub fn configure() -> Result<Application, BootError> {
    let mut app = Application::configure(|_| {});
    for provider in providers::providers() {
        app.provider(provider);
    }
    commands::register_default();
    // Publish the live route table so `route:list` introspects the real routes.
    // `rustasea-cli` is framework-generic and cannot depend on this app crate,
    // so the app pushes its table into the CLI's process-wide route registry at
    // boot; the command reads it back (see `rustasea::cli::routes`).
    rustasea::cli::routes::set_route_source(|| crate::routes::table().get_routes());
    app.boot()?;
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Booting publishes the live route table `route:list` reads back.
    #[test]
    fn configure_publishes_route_table() {
        configure().expect("app boots");

        let routes = rustasea::cli::routes::routes();
        let paths: Vec<&str> = routes.iter().map(|route| route.path.as_str()).collect();
        for expected in ["/login", "/dashboard", "/settings/profile"] {
            assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
        }
    }
}
