//! Example domain routes - the `/examples/posts` JSON CRUD surface.
//!
//! These routes bind the scaffold-style [`PostController`] actions registered in
//! `crate::app::http::controllers`, demonstrating the real controller-dispatch
//! DSL (`table.get_action(path, Handler::method).named("name")`). They are
//! ungated: the example is a plain JSON API with no authentication.
//!
//! [`PostController`]: crate::app::http::controllers::PostController

use rustasea::router::Router as RouteTable;

use crate::app::http::controllers::PostController;

/// Register the example domain routes onto `table`.
pub fn register(table: &mut RouteTable) {
    table
        .get_action("/examples/posts", PostController::index)
        .named("examples.posts.index");
    table
        .get_action("/examples/posts/{id}", PostController::show)
        .named("examples.posts.show");
    table
        .post_action("/examples/posts", PostController::store)
        .named("examples.posts.store");
    table
        .put_action("/examples/posts/{id}", PostController::update)
        .named("examples.posts.update");
    table
        .patch_action("/examples/posts/{id}", PostController::update)
        .named("examples.posts.update.patch");
    table
        .delete_action("/examples/posts/{id}", PostController::destroy)
        .named("examples.posts.destroy");
}
