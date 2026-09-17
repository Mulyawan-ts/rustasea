//! Sycamore adapter for the `svelte` starter-kit variant (ADR-0002 decision 4).

use sycamore::prelude::*;
use sycamore::web::events::MouseEvent;

use rustasea_inertia_client::NavigationOutcome;

use crate::browser::{fetch_page, hard_navigate};
use crate::router::RouterState;

/// Sycamore context carrying the shared Inertia [`RouterState`].
///
/// [`Signal`] is a `Copy` handle into Sycamore's reactive graph, so the context
/// is cheap to clone and can be read from any descendant scope.
#[derive(Clone, Copy)]
pub struct RouterContext {
    state: Signal<RouterState>,
}

impl RouterContext {
    /// The reactive router state signal.
    pub fn state(&self) -> Signal<RouterState> {
        self.state
    }
}

/// Provide the Inertia router to a Sycamore subtree.
///
/// `initial_page` is the JSON the server embedded in the root document's
/// `data-page` attribute. It is hydrated once, mounting the initial component
/// through the registry installed via [`crate::install_registry`].
#[component(inline_props)]
pub fn RouterProvider(initial_page: String, children: Children) -> View {
    let mut router = RouterState::new();
    let _ = router.hydrate(&initial_page);

    provide_context(RouterContext {
        state: create_signal(router),
    });

    children.call()
}

/// Read the nearest [`RouterContext`].
///
/// Panics when called outside a [`RouterProvider`] subtree, matching Sycamore's
/// `use_context` contract.
pub fn use_router() -> RouterContext {
    use_context::<RouterContext>()
}

/// A Sycamore anchor that performs an Inertia visit instead of a full page load.
///
/// Clicking prevents the browser's default navigation, fetches `to` with the
/// `X-Inertia` headers, and swaps the page in place. A version conflict
/// (`409` + `X-Inertia-Location`) or a non-Inertia response hard-navigates.
#[component(inline_props)]
pub fn Link(to: String, children: Children) -> View {
    let context = use_router();
    let href = to.clone();
    let content = children.call();

    view! {
        a(
            href=href,
            on:click=move |event: MouseEvent| {
                event.prevent_default();
                let to = to.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let snapshot = context.state().get_clone();
                    match fetch_page(&to, &snapshot).await {
                        Ok(NavigationOutcome::Mount(page)) => {
                            context.state().update(|router| router.set_page(page));
                        }
                        Ok(NavigationOutcome::HardNavigate(location)) => hard_navigate(&location),
                        Ok(NavigationOutcome::FullReload) | Err(_) => hard_navigate(&to),
                    }
                });
            }
        ) {
            (content)
        }
    }
}
