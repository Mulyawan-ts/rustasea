//! Class-based `make:*` generators (controller, middleware, request, model,
//! provider, command, job, event, listener, observer, test, seeder, agent,
//! tool, action) each emit a real scaffold file.

use crate::common::temp_root;

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// One scaffold expectation: kind, PascalCase name, output rel path, and a
/// marker the source must contain.
struct Case {
    kind: Kind,
    name: &'static str,
    rel: &'static str,
    marker: &'static str,
    resource: bool,
}

/// The full matrix of class-based generators and their expected output.
fn class_cases() -> Vec<Case> {
    vec![
        Case {
            kind: Kind::Controller,
            name: "PostController",
            rel: "app/http/controllers/post_controller.rs",
            marker: "PostController",
            resource: false,
        },
        Case {
            kind: Kind::Controller,
            name: "PostController",
            rel: "app/http/controllers/post_controller.rs",
            marker: "destroy",
            resource: true,
        },
        Case {
            kind: Kind::Middleware,
            name: "EnsureTokenIsValid",
            rel: "app/http/middleware/ensure_token_is_valid.rs",
            marker: "EnsureTokenIsValid",
            resource: false,
        },
        Case {
            kind: Kind::Request,
            name: "StorePostRequest",
            rel: "app/http/requests/store_post_request.rs",
            marker: "StorePostRequest",
            resource: false,
        },
        Case {
            kind: Kind::Model,
            name: "Post",
            rel: "app/models/post.rs",
            marker: "struct Post",
            resource: false,
        },
        Case {
            kind: Kind::Provider,
            name: "AnalyticsProvider",
            rel: "app/providers/analytics_provider.rs",
            marker: "AnalyticsProvider",
            resource: false,
        },
        Case {
            kind: Kind::Command,
            name: "GenerateReport",
            rel: "app/console/commands/generate_report.rs",
            marker: "GenerateReport",
            resource: false,
        },
        Case {
            kind: Kind::Job,
            name: "SendEmail",
            rel: "app/jobs/send_email.rs",
            marker: "SendEmail",
            resource: false,
        },
        Case {
            kind: Kind::Event,
            name: "OrderPlaced",
            rel: "app/events/order_placed.rs",
            marker: "OrderPlaced",
            resource: false,
        },
        Case {
            kind: Kind::Listener,
            name: "SendOrderConfirmation",
            rel: "app/listeners/send_order_confirmation.rs",
            marker: "SendOrderConfirmation",
            resource: false,
        },
        Case {
            kind: Kind::Observer,
            name: "PostObserver",
            rel: "app/observers/post_observer.rs",
            marker: "PostObserver",
            resource: false,
        },
        Case {
            kind: Kind::Test,
            name: "UserTest",
            rel: "tests/feature/user_test.rs",
            marker: "TestCase",
            resource: false,
        },
        Case {
            kind: Kind::Seeder,
            name: "DatabaseSeeder",
            rel: "database/seeders/database_seeder.rs",
            marker: "DatabaseSeeder",
            resource: false,
        },
        Case {
            kind: Kind::Agent,
            name: "SupportAgent",
            rel: "app/ai/agents/support_agent.rs",
            marker: "SupportAgent",
            resource: false,
        },
        Case {
            kind: Kind::Tool,
            name: "SearchDocs",
            rel: "app/ai/tools/search_docs.rs",
            marker: "SearchDocs",
            resource: false,
        },
        Case {
            kind: Kind::Action,
            name: "PublishPost",
            rel: "app/actions/publish_post.rs",
            marker: "impl Action for",
            resource: false,
        },
    ]
}

/// Every class-based `make:*` generator emits a real scaffold file.
#[test]
fn class_generators_write_real_sources() {
    for case in class_cases() {
        let root = temp_root();
        let opts = MakeOptions {
            name: case.name.to_string(),
            force: false,
            resource: case.resource,
            with_migration: false,
            browser: false,
        };
        let files = generate(case.kind, &root, &opts).expect("scaffold succeeds");
        let target = root.join(case.rel);
        assert!(
            target.is_file(),
            "{:?} was not written at {}",
            case.kind.command(),
            case.rel
        );

        let source = std::fs::read_to_string(&target).expect("read scaffold");
        assert!(
            source.contains(case.marker),
            "{:?} source lacks `{}`: {}",
            case.kind.command(),
            case.marker,
            source
        );
        if case.kind == Kind::Controller {
            assert!(
                source.contains("use rustasea::http::{Controller, JsonResponse};"),
                "controller source must import the base Controller trait: {source}"
            );
            assert!(
                source.contains(&format!("impl Controller for {} {{}}", case.name)),
                "controller source must implement the base Controller trait: {source}"
            );
        }
        assert!(
            source.len() > 120,
            "{:?} source looks like a stub ({} bytes)",
            case.kind.command(),
            source.len()
        );
        assert_eq!(
            files.len(),
            1,
            "{:?} should write exactly one file",
            case.kind.command()
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
