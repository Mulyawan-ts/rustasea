//! Template rendering hook tests for `rustasea-mail` (AUTH-018).
//!
//! Gated behind the `templates` feature. The positive case proves a rendered
//! body carries the signed URL from the context; the negative case proves a
//! missing template is a typed error, never an empty body.
//!
//! Template roots are written under `CARGO_TARGET_TMPDIR` (a directory inside
//! `target/`), not `/tmp` — `/tmp` is a small tmpfs in this environment.

#![cfg(feature = "templates")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustasea_mail::{
    Mail, MailAddress, MailError, Mailable, MinijinjaEngine, TemplateMailable, ViewEngine,
    ViewError,
};
use serde::Serialize;

/// A serializable context mirroring the verify-email template contract.
#[derive(Serialize)]
struct VerifyContext {
    /// The absolute signed verification URL rendered into the body.
    link: String,
}

/// A unique scratch views directory under `target/`.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch views dir");
    dir
}

/// A layout + page pair that reproduces the shipped `mail/` template shape.
fn write_mail_templates(root: &Path) {
    std::fs::write(
        root.join("layout.html"),
        "<!DOCTYPE html><body>{% block content %}{% endblock %}</body>",
    )
    .expect("write layout");
    std::fs::write(
        root.join("verify-email.html"),
        "{% extends \"layout.html\" %}{% block content %}\
         <a href=\"{{ link | safe }}\">Verify email address</a>\
         {% endblock %}",
    )
    .expect("write verify-email template");
}

/// Positive: rendering the verify-email template produces a body containing the
/// signed URL passed in the context.
#[test]
fn renders_verify_email_template_with_signed_link() {
    let root = scratch_dir("mail_views_positive");
    write_mail_templates(&root);
    let engine: Arc<dyn ViewEngine> = Arc::new(MinijinjaEngine::new(&root));

    let link = "https://example.test/email/verify/user-a/deadbeef?expires=1&signature=abc";
    let mailable = TemplateMailable::new(
        engine,
        "verify-email.html",
        VerifyContext {
            link: link.to_string(),
        },
    )
    .expect("serialize context")
    .subject("Verify your email address")
    .to(MailAddress::from_email("ada@example.com"));

    let body = mailable
        .render_html()
        .expect("render verify-email template");
    assert!(
        body.contains(link),
        "rendered body must carry the signed URL: {body}"
    );
    assert!(body.contains("Verify email address"), "got {body}");

    // The transport-ready message carries the same rendered body.
    let message = mailable.try_build().expect("build message");
    assert_eq!(message.html.as_deref(), Some(body.as_str()));
    assert_eq!(message.to[0].email, "ada@example.com");
}

/// Negative: a missing template is a typed error, not an empty body.
#[test]
fn missing_template_is_a_typed_error() {
    let root = scratch_dir("mail_views_negative");
    let engine: Arc<dyn ViewEngine> = Arc::new(MinijinjaEngine::new(&root));

    let mailable = TemplateMailable::new(
        engine,
        "does-not-exist.html",
        VerifyContext {
            link: "https://example.test/verify".to_string(),
        },
    )
    .expect("serialize context");

    let error = mailable
        .render_html()
        .expect_err("a missing template must fail, never render empty");
    assert!(
        matches!(
            &error,
            MailError::Template(ViewError::TemplateNotFound { name }) if name == "does-not-exist.html"
        ),
        "expected a typed TemplateNotFound, got {error:?}"
    );

    // `try_build` surfaces the same typed failure before building a message.
    let build_error = mailable.try_build().expect_err("build must fail too");
    assert!(
        matches!(build_error, MailError::Template(_)),
        "got {build_error:?}"
    );
}

/// The explicit-injection delivery path (ADR-0007) sends a rendered message
/// through a caller-supplied mailer without touching the global registry.
#[tokio::test]
async fn explicit_mailer_injection_delivers_rendered_message() {
    use rustasea_mail::ArrayMailer;

    let root = scratch_dir("mail_views_injection");
    write_mail_templates(&root);
    let engine: Arc<dyn ViewEngine> = Arc::new(MinijinjaEngine::new(&root));

    let mailable = TemplateMailable::new(
        engine,
        "verify-email.html",
        VerifyContext {
            link: "https://example.test/email/verify/user-a/hash".to_string(),
        },
    )
    .expect("serialize context")
    .subject("Verify your email address")
    .to(MailAddress::from_email("ada@example.com"));

    let mailer = Arc::new(ArrayMailer::new());
    let message = mailable.try_build().expect("build message");
    Mail::deliver_with(&(mailer.clone() as Arc<dyn rustasea_mail::Mailer>), message)
        .await
        .expect("deliver via explicit mailer");

    assert_eq!(
        mailer.count(),
        1,
        "the injected mailer must receive the mail"
    );
    let sent = mailer.last().expect("captured message");
    assert!(
        sent.html
            .as_deref()
            .unwrap_or_default()
            .contains("/email/verify/user-a/hash"),
        "the delivered body must carry the signed link"
    );
}

/// A `Mailable` implementation still builds through the infallible contract
/// when no templating is involved (guard against a regression in `build`).
#[test]
fn plain_mailable_still_builds() {
    struct Plain;
    impl Mailable for Plain {
        fn subject(&self) -> String {
            "Plain".into()
        }
        fn to(&self) -> Vec<MailAddress> {
            vec![MailAddress::from_email("ada@example.com")]
        }
        fn html_body(&self) -> String {
            "<p>hi</p>".into()
        }
    }
    let message = Plain.build();
    assert_eq!(message.html.as_deref(), Some("<p>hi</p>"));
}
