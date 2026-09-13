//! RESTful resource registration — the seven Laravel-style actions.
//!
//! Split out of [`crate::router`] so the builder stays within the file-size
//! budget. [`Router::resource`] delegates here; every generated route carries
//! the group name prefix and middleware through the shared
//! [`Router::push_with_name`] path.

use crate::route::ControllerRef;
use crate::router::Router;

impl Router {
    /// Expand a resource into its seven REST routes (8 method rows).
    ///
    /// `index`, `create`, `store`, `show`, `edit`, `update` (both `PUT` and
    /// `PATCH`), and `destroy`. Names are `{name}.{action}` composed with the
    /// group name prefix; the controller ref is attached when non-empty so a
    /// later controller-binding pass can resolve real handlers.
    pub(crate) fn register_resource(&mut self, name: &str, controller: &str) -> &mut Self {
        let resolved = if controller.is_empty() {
            self.pending_controller.clone().unwrap_or_default()
        } else {
            controller.to_string()
        };
        let base = format!("/{name}");
        let item = format!("/{name}/{{id}}");
        let create = format!("/{name}/create");
        let edit = format!("/{name}/{{id}}/edit");

        self.begin_batch();
        self.resource_route("GET", &base, name, "index", &resolved);
        self.resource_route("GET", &create, name, "create", &resolved);
        self.resource_route("POST", &base, name, "store", &resolved);
        self.resource_route("GET", &item, name, "show", &resolved);
        self.resource_route("GET", &edit, name, "edit", &resolved);
        self.resource_route("PUT", &item, name, "update", &resolved);
        self.resource_route("PATCH", &item, name, "update", &resolved);
        self.resource_route("DELETE", &item, name, "destroy", &resolved);
        self
    }

    /// Register a single named resource route with an optional controller.
    fn resource_route(
        &mut self,
        method: &str,
        path: &str,
        resource: &str,
        action: &str,
        controller: &str,
    ) {
        self.push_with_name(method, path, &format!("{resource}.{action}"));
        if let Some(entry) = self.routes.last_mut() {
            entry.controller = (!controller.is_empty()).then(|| ControllerRef {
                name: controller.to_string(),
                action: action.to_string(),
            });
        }
    }
}
