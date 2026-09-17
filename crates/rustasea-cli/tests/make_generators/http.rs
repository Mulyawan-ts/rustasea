//! HTTP generators (middleware, request) - framework wiring and name
//! validation.

use crate::common::temp_root;

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// `make:middleware` / `make:request` reject a non-PascalCase name with the
/// typed `GenerationFailed` error before touching the filesystem.
#[test]
fn http_generators_reject_invalid_names() {
    for (kind, rel) in [
        (Kind::Middleware, "app/http/middleware/ensure_token.rs"),
        (Kind::Request, "app/http/requests/store_post.rs"),
    ] {
        let root = temp_root();
        let opts = MakeOptions {
            name: "ensure_token".to_string(),
            force: false,
            resource: false,
            with_migration: false,
            browser: false,
        };
        let err = generate(kind, &root, &opts).expect_err("invalid name must fail");
        assert!(
            matches!(err, rustasea_cli::CliError::GenerationFailed { .. }),
            "{:?} must surface a typed GenerationFailed, got {err:?}",
            kind.command()
        );
        assert!(
            !root.join(rel).exists(),
            "{:?} must not write on an invalid name",
            kind.command()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Generated HTTP skeletons reference the framework types they compile against:
/// axum's function-middleware surface and the validation `Validatable` trait.
#[test]
fn http_generators_emit_framework_wired_sources() {
    let root = temp_root();

    let middleware = generate(
        Kind::Middleware,
        &root,
        &MakeOptions {
            name: "EnsureTokenIsValid".to_string(),
            force: false,
            resource: false,
            with_migration: false,
            browser: false,
        },
    )
    .expect("middleware scaffold");
    let source =
        std::fs::read_to_string(root.join(&middleware[0].path)).expect("read middleware source");
    assert!(
        source.contains("use axum::extract::Request;"),
        "middleware must use axum's request extractor: {source}"
    );
    assert!(
        source.contains("use axum::middleware::Next;"),
        "middleware must use axum's Next: {source}"
    );
    assert!(
        source.contains("next.run(request).await"),
        "middleware must delegate to the wrapped handler: {source}"
    );

    let request = generate(
        Kind::Request,
        &root,
        &MakeOptions {
            name: "StorePostRequest".to_string(),
            force: false,
            resource: false,
            with_migration: false,
            browser: false,
        },
    )
    .expect("request scaffold");
    let source = std::fs::read_to_string(root.join(&request[0].path)).expect("read request source");
    assert!(
        source.contains("use rustasea::validation::{ErrorBag, FormRequest, Rules, Validatable};"),
        "request must import the validation surface: {source}"
    );
    assert!(
        source.contains("impl Validatable for StorePostRequest"),
        "request must implement Validatable: {source}"
    );
    assert!(
        source.contains("fn validate(&self) -> Result<(), ErrorBag>"),
        "request must declare the validation hook: {source}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
