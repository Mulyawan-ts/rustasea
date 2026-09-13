//! Livewire variant resources — askama views enhanced with HTMX.
//!
//! The livewire kit reuses the blade askama views and overrides both base
//! layouts to load HTMX; fragment partials drive targeted DOM swaps and
//! `rustasea-broadcast` pushes realtime updates (ADR-0002 decision 5).

use super::TemplateFile;

/// `resources/views` layout paths that livewire replaces with HTMX variants.
///
/// The blade shell and the nav-free auth shell are both overridden so every
/// page — including the auth pages — carries `hx-boost` and the HTMX script.
const OVERRIDDEN_LAYOUTS: &[&str] = &[
    "resources/views/layouts/app.html",
    "resources/views/layouts/auth.html",
];

/// Livewire `resources/views` templates.
pub fn entries() -> Vec<TemplateFile> {
    // Reuse the shared askama views (including the `resources/css/app.css`
    // design-token entry), replacing only the two base layouts.
    let mut files: Vec<TemplateFile> = super::blade::entries()
        .into_iter()
        .filter(|(path, _)| !OVERRIDDEN_LAYOUTS.contains(path))
        .collect();
    files.push(("resources/views/layouts/app.html", LAYOUT));
    files.push(("resources/views/layouts/auth.html", AUTH_LAYOUT));
    files.extend([
        ("resources/views/partials/counter.html", COUNTER),
        ("resources/views/partials/login-form.html", LOGIN_FORM),
        ("resources/views/partials/profile-form.html", PROFILE_FORM),
    ]);
    files
}

const LAYOUT: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <title>@@app_pascal@@</title>
  {% include "partials/head.html" %}
  <script src="https://unpkg.com/htmx.org@2" defer></script>
</head>
<body hx-boost="true">
  {% import "components/user-menu.html" as user_menu %}
  <nav>
    <a href="/">@@app_pascal@@</a>
    {% if let Some(current) = user %}
      <a href="/dashboard">Dashboard</a>
      <a href="/settings/profile">Settings</a>
      {{ user_menu::user_menu(name=current.name) }}
    {% else %}
      <a href="/login">Log in</a>
      <a href="/register">Register</a>
    {% endif %}
  </nav>

  {% include "partials/flash.html" %}

  <main id="content">
    {% block content %}{% endblock %}
  </main>
</body>
</html>
"##;

/// Nav-free HTMX auth shell (mirrors `layouts/auth.html` with `hx-boost`).
const AUTH_LAYOUT: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <title>@@app_pascal@@</title>
  {% include "partials/head.html" %}
  <script src="https://unpkg.com/htmx.org@2" defer></script>
</head>
<body hx-boost="true">
  {% include "partials/flash.html" %}

  <main id="content">
    {% block content %}{% endblock %}
  </main>
</body>
</html>
"##;

const COUNTER: &str = r##"<div id="counter" hx-target="this" hx-swap="outerHTML">
  <button hx-post="/livewire/counter/actions/increment">Increment</button>
  <span data-count>{{ count }}</span>
</div>
"##;

const LOGIN_FORM: &str = r##"<form hx-post="/login" hx-target="#content" hx-swap="innerHTML">
  <label>Email <input type="email" name="email" required /></label>
  <label>Password <input type="password" name="password" required /></label>
  <button type="submit">Log in</button>
</form>
"##;

const PROFILE_FORM: &str = r##"<form hx-patch="/settings/profile" hx-target="#content" hx-swap="innerHTML">
  {% if let Some(current) = user %}
  <label>Name <input type="text" name="name" value="{{ current.name }}" required /></label>
  <label>Email <input type="email" name="email" value="{{ current.email }}" required /></label>
  {% endif %}
  <button type="submit">Save</button>
</form>
"##;
