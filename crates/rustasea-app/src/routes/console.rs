//! Console surface — mirrors the command-registry pattern over HTTP.
//!
//! The app registers the framework's built-in commands at boot
//! (`bootstrap/commands.rs`); this module exposes the same process-wide
//! registry as a read-only HTTP surface so the registered command list is
//! observable. It mirrors the generated scaffold's `routes/console.rs`, which
//! surfaces `bootstrap/commands.rs` rather than defining HTTP routes of its
//! own.

use axum::response::Response;

use rustasea::router::Router as RouteTable;

/// Console index payload — the registered command signatures.
#[derive(serde::Serialize)]
struct ConsoleIndex {
    /// Registered command signatures, in registration order.
    commands: Vec<String>,
}

/// Return the registered console command signatures.
///
/// Reads the process-wide [`rustasea::cli::registry`] the same way the
/// scaffold's `routes/console.rs` surfaces `bootstrap/commands.rs`. A poisoned
/// lock yields an empty list rather than panicking.
pub fn commands() -> Vec<String> {
    match rustasea::cli::registry().read() {
        Ok(registry) => registry
            .list()
            .into_iter()
            .map(|registered| registered.meta.name)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Register the console route table onto `table`.
pub fn register(table: &mut RouteTable) {
    table.get_action("/console", index).named("console.index");
}

/// GET /console — list the registered console commands as JSON.
async fn index() -> Response {
    rustasea::http::JsonResponse::ok(ConsoleIndex {
        commands: commands(),
    })
}
