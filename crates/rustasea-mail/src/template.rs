//! Template rendering hook for mailables (AUTH-018).
//!
//! The [`Mailable`](crate::Mailable) contract only knows how to return an
//! already-built HTML `String`. [`TemplateMailable`] bridges that contract to
//! [`rustasea_view`]: it takes a template name plus any [`Serialize`] context
//! and renders the body through a [`ViewEngine`] (minijinja at runtime).
//!
//! Rendering is **fallible on purpose**: a missing template, an unserializable
//! context, or an engine failure returns a typed [`MailError`] rather than an
//! empty body, so an auth handler can fail closed instead of mailing a blank
//! message. This is why [`TemplateMailable`] does **not** implement
//! [`Mailable`](crate::Mailable) — that trait's `html_body` is infallible and
//! would have to swallow the error. Callers assemble a transport-ready
//! [`MailMessage`] with [`TemplateMailable::try_build`] and deliver it through
//! an explicitly resolved [`Mailer`](crate::Mailer).
//!
//! The whole module is gated behind the `templates` cargo feature, so the base
//! crate never links a template engine unless an application opts in.

use std::sync::Arc;

use serde::Serialize;

use crate::address::MailAddress;
use crate::error::{MailError, Result};
use crate::message::MailMessage;

/// View-engine types a template-backed mailable needs, re-exported so callers
/// depend only on `rustasea-mail`.
pub use rustasea_view::{MinijinjaEngine, ViewEngine, ViewError};

/// A mail body rendered from a `rustasea-view` template.
///
/// Construct one with [`TemplateMailable::new`], then chain [`subject`],
/// [`to`](TemplateMailable::to), and [`text`](TemplateMailable::text) before
/// calling [`try_build`](TemplateMailable::try_build). The context is any
/// [`Serialize`] value; it is converted to JSON once at construction, so the
/// per-render cost is a single template evaluation.
pub struct TemplateMailable {
    /// Engine that resolves and renders the template.
    engine: Arc<dyn ViewEngine>,
    /// Template name, application-relative (for example `mail/verify-email.html`).
    template: String,
    /// Serialized render context.
    context: serde_json::Value,
    /// Subject line, empty until [`subject`](TemplateMailable::subject) is set.
    subject: String,
    /// Primary recipients.
    to: Vec<MailAddress>,
    /// Optional plain-text body.
    text: Option<String>,
}

impl TemplateMailable {
    /// Create a template-backed mailable.
    ///
    /// `context` is serialized immediately; a serialization failure is a typed
    /// [`MailError::Render`]. The template itself is resolved lazily, on
    /// [`render_html`](TemplateMailable::render_html) / [`try_build`].
    pub fn new(
        engine: Arc<dyn ViewEngine>,
        template: impl Into<String>,
        context: impl Serialize,
    ) -> Result<Self> {
        let context = serde_json::to_value(context).map_err(|source| {
            MailError::Render(format!(
                "mail template context could not be serialized: {source}"
            ))
        })?;
        Ok(Self {
            engine,
            template: template.into(),
            context,
            subject: String::new(),
            to: Vec::new(),
            text: None,
        })
    }

    /// Set the subject line.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Append a primary recipient.
    pub fn to(mut self, to: impl Into<MailAddress>) -> Self {
        self.to.push(to.into());
        self
    }

    /// Set the optional plain-text body.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// The template name this mailable renders.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Render the template to an HTML body.
    ///
    /// A missing template is a typed [`MailError::Template`] carrying the
    /// engine's [`ViewError::TemplateNotFound`]; the body is never silently
    /// empty.
    pub fn render_html(&self) -> Result<String> {
        self.engine
            .render_value(&self.template, &self.context)
            .map(|response| response.into_body())
            .map_err(MailError::from)
    }

    /// Render and assemble the transport-ready [`MailMessage`].
    ///
    /// Rendering happens first, so a template failure surfaces as a typed
    /// [`MailError`] before any message is built.
    pub fn try_build(&self) -> Result<MailMessage> {
        let mut message = MailMessage::new(self.subject.clone());
        message.to = self.to.clone();
        message.html = Some(self.render_html()?);
        message.text = self.text.clone();
        Ok(message)
    }
}
