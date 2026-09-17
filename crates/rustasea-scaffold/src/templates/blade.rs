//! Blade variant resources — server-rendered askama views.
//!
//! Templates live under `resources/views` and are compiled into the binary by
//! askama (`askama.toml` points the engine at that directory).
//!
//! The layout family mirrors `laravel/livewire-starter-kit`:
//!
//! - `layouts/app.html` — authenticated shell with the app nav chrome.
//! - `layouts/auth.html` — nav-free auth shell (login/register/confirm).
//! - `partials/head.html` — the shared `<head>` remainder (each layout keeps its
//!   own `<title>` because askama `include` takes no parameters).
//! - `components/*.html` — reusable `{% macro %}` files (askama has no
//!   `@props`/slots/attribute-merge; callers pass keyword arguments).
//! - `settings/layout.html` — nested settings section layout with its own
//!   sub-nav, extending the app shell.

use super::TemplateFile;

/// Blade `resources/views` templates.
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("resources/views/layouts/app.html", LAYOUT),
        ("resources/views/layouts/auth.html", AUTH_LAYOUT),
        ("resources/views/partials/head.html", HEAD),
        ("resources/views/partials/flash.html", FLASH),
        ("resources/views/components/auth-header.html", AUTH_HEADER),
        ("resources/views/components/user-menu.html", USER_MENU),
        ("resources/views/dashboard.html", DASHBOARD),
        ("resources/views/auth/login.html", LOGIN),
        ("resources/views/auth/register.html", REGISTER),
        ("resources/views/auth/forgot-password.html", FORGOT_PASSWORD),
        ("resources/views/auth/reset-password.html", RESET_PASSWORD),
        (
            "resources/views/auth/confirm-password.html",
            CONFIRM_PASSWORD,
        ),
        ("resources/views/auth/verify-email.html", VERIFY_EMAIL),
        ("resources/views/settings/layout.html", SETTINGS_LAYOUT),
        ("resources/views/settings/profile.html", SETTINGS_PROFILE),
        ("resources/views/settings/password.html", SETTINGS_PASSWORD),
        ("resources/views/settings/security.html", SETTINGS_SECURITY),
        ("resources/css/app.css", APP_CSS),
    ]
}

/// Shared `<head>` remainder included by every layout.
///
/// The `<title>` stays in each layout because askama's `include` takes no
/// parameters, so a partial cannot receive the per-page title.
const HEAD: &str = r##"<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<link rel="icon" href="/favicon.ico" />
<link rel="stylesheet" href="/css/app.css" />
"##;

/// Authenticated app shell: nav chrome, shared head, and the content block.
const LAYOUT: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <title>@@app_pascal@@</title>
  {% include "partials/head.html" %}
</head>
<body>
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

  <main>
    {% block content %}{% endblock %}
  </main>
</body>
</html>
"##;

/// Nav-free auth shell. Auth pages extend this so they never render the
/// authenticated nav chrome (the bug this layout family fixes).
const AUTH_LAYOUT: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <title>@@app_pascal@@</title>
  {% include "partials/head.html" %}
</head>
<body>
  {% include "partials/flash.html" %}

  <main>
    {% block content %}{% endblock %}
  </main>
</body>
</html>
"##;

/// Auth page heading component (title + description).
const AUTH_HEADER: &str = r##"{% macro auth_header(title, description) -%}
<header>
  <h1>{{ title }}</h1>
  <p>{{ description }}</p>
</header>
{%- endmacro %}
"##;

/// Current-user component: name plus a logout form.
const USER_MENU: &str = r##"{% macro user_menu(name) -%}
<div>
  <span>{{ name }}</span>
  <form method="post" action="/logout">
    <button type="submit">Log out</button>
  </form>
</div>
{%- endmacro %}
"##;

const DASHBOARD: &str = r##"{% extends "layouts/app.html" %}
{% block content %}
<h1>Dashboard</h1>
{% if let Some(current) = user %}
<p>Welcome back, {{ current.name }}.</p>
{% endif %}
{% endblock %}
"##;

const LOGIN: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Log in", description="Welcome back. Enter your credentials to continue.") }}
<form method="post" action="/login">
  <label>Email <input type="email" name="email" required /></label>
  <label>Password <input type="password" name="password" required /></label>
  <button type="submit">Log in</button>
</form>
<p><a href="/forgot-password">Forgot your password?</a></p>
{% endblock %}
"##;

const REGISTER: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Register", description="Create an account to get started.") }}
<form method="post" action="/register">
  <label>Name <input type="text" name="name" required /></label>
  <label>Email <input type="email" name="email" required /></label>
  <label>Password <input type="password" name="password" required /></label>
  <label>Confirm <input type="password" name="password_confirmation" required /></label>
  <button type="submit">Create account</button>
</form>
{% endblock %}
"##;

const FORGOT_PASSWORD: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Forgot password", description="Enter your email and we will send a reset link.") }}
<form method="post" action="/forgot-password">
  <label>Email <input type="email" name="email" required /></label>
  <button type="submit">Email reset link</button>
</form>
{% endblock %}
"##;

const RESET_PASSWORD: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Reset password", description="Choose a new password for your account.") }}
<form method="post" action="/reset-password">
  <input type="hidden" name="token" value="{{ token }}" />
  <label>Email <input type="email" name="email" required /></label>
  <label>New password <input type="password" name="password" required /></label>
  <label>Confirm <input type="password" name="password_confirmation" required /></label>
  <button type="submit">Reset password</button>
</form>
{% endblock %}
"##;

const CONFIRM_PASSWORD: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Confirm password", description="This is a secure area. Please confirm your password before continuing.") }}
<form method="post" action="/confirm-password">
  <label>Password <input type="password" name="password" required /></label>
  <button type="submit">Confirm</button>
</form>
{% endblock %}
"##;

const VERIFY_EMAIL: &str = r##"{% extends "layouts/auth.html" %}
{% import "components/auth-header.html" as auth_header %}
{% block content %}
{{ auth_header::auth_header(title="Verify email", description="Check your inbox for the verification link we sent you.") }}

{# STUB: `POST /email/verification-notification` is not yet in the generated
   route table (email verification infrastructure has not landed); this form
   is a clearly-marked stub that mirrors the kit's resend action. #}
<form method="post" action="/email/verification-notification">
  <button type="submit">Resend verification email</button>
</form>

<form method="post" action="/logout">
  <button type="submit">Log out</button>
</form>
{% endblock %}
"##;

/// Nested settings section layout: overrides the app content block with a
/// settings sub-nav and exposes a `settings` block for the child screens.
const SETTINGS_LAYOUT: &str = r##"{% extends "layouts/app.html" %}
{% block content %}
<section>
  <h1>Settings</h1>
  <nav>
    <a href="/settings/profile">Profile</a>
    <a href="/settings/password">Password</a>
    <a href="/settings/security">Security</a>
  </nav>

  {% block settings %}{% endblock %}
</section>
{% endblock %}
"##;

const SETTINGS_PROFILE: &str = r##"{% extends "settings/layout.html" %}
{% block settings %}
<h2>Profile</h2>
<form method="post" action="/settings/profile">
  <input type="hidden" name="_method" value="PATCH" />
  {% if let Some(current) = user %}
  <label>Name <input type="text" name="name" value="{{ current.name }}" required /></label>
  <label>Email <input type="email" name="email" value="{{ current.email }}" required /></label>
  {% endif %}
  <button type="submit">Save</button>
</form>
{% endblock %}
"##;

const SETTINGS_PASSWORD: &str = r##"{% extends "settings/layout.html" %}
{% block settings %}
<h2>Password</h2>
<form method="post" action="/settings/password">
  <input type="hidden" name="_method" value="PUT" />
  <label>Current password <input type="password" name="current_password" required /></label>
  <label>New password <input type="password" name="password" required /></label>
  <label>Confirm <input type="password" name="password_confirmation" required /></label>
  <button type="submit">Update password</button>
</form>
{% endblock %}
"##;

/// Security settings screen: two-factor authentication and passkey management.
///
/// Both sections post to the kit's real management endpoints — 2FA to
/// `/user/two-factor-authentication` (enable/disable) and passkeys to
/// `/user/passkeys` (register). The recovery-code screen is linked rather than
/// inlined so the one-time codes are only shown on their dedicated page.
const SETTINGS_SECURITY: &str = r##"{% extends "settings/layout.html" %}
{% block settings %}
<h2>Security</h2>

<section>
  <h3>Two-Factor Authentication</h3>
  <p>Add an extra layer of security to your account using a TOTP authenticator app.</p>
  <form method="post" action="/user/two-factor-authentication">
    <button type="submit">Enable two-factor authentication</button>
  </form>
  <form method="post" action="/user/two-factor-authentication">
    <input type="hidden" name="_method" value="DELETE" />
    <button type="submit">Disable two-factor authentication</button>
  </form>
  <p><a href="/user/two-factor-recovery-codes">View recovery codes</a></p>
</section>

<section>
  <h3>Passkeys</h3>
  <p>Sign in without a password using a passkey stored on your device.</p>
  <form method="post" action="/user/passkeys">
    <button type="submit">Register a passkey</button>
  </form>
</section>
{% endblock %}
"##;

const FLASH: &str = r##"{% if let Some(message) = flash %}
<div role="status">{{ message }}</div>
{% endif %}
"##;

/// Plain-CSS design-token entry linked from `partials/head.html`.
///
/// Deliberately dependency-free: custom properties, a dark override, and a tiny
/// base layer. No CSS framework directives, no preprocessor, no build step.
const APP_CSS: &str = r##"/* @@app_pascal@@ — design tokens and base layer.
 * Plain CSS only: no CSS framework directives, no preprocessor, no build step.
 * Tweak the custom properties below to re-theme the whole starter kit. */
:root {
  --color-bg: #ffffff;
  --color-surface: #f8fafc;
  --color-text: #0f172a;
  --color-muted: #64748b;
  --color-border: #e2e8f0;
  --color-primary: #4f46e5;
  --color-primary-hover: #4338ca;
  --color-on-primary: #ffffff;
  --color-danger: #dc2626;

  --font-sans: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  --font-mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;

  --radius-sm: 0.25rem;
  --radius-md: 0.5rem;
  --radius-lg: 1rem;

  --space-1: 0.25rem;
  --space-2: 0.5rem;
  --space-3: 0.75rem;
  --space-4: 1rem;
  --space-6: 1.5rem;
  --space-8: 2rem;

  --shadow-sm: 0 1px 2px rgb(15 23 42 / 0.08);
  --shadow-md: 0 4px 12px rgb(15 23 42 / 0.10);
}

/* Dark theme: opt in explicitly, and follow the OS preference by default. */
[data-theme="dark"] {
  --color-bg: #0b1120;
  --color-surface: #111827;
  --color-text: #e2e8f0;
  --color-muted: #94a3b8;
  --color-border: #1f2937;
  --color-primary: #818cf8;
  --color-primary-hover: #a5b4fc;
  --color-on-primary: #0b1120;
}

@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --color-bg: #0b1120;
    --color-surface: #111827;
    --color-text: #e2e8f0;
    --color-muted: #94a3b8;
    --color-border: #1f2937;
    --color-primary: #818cf8;
    --color-primary-hover: #a5b4fc;
    --color-on-primary: #0b1120;
  }
}

*,
*::before,
*::after {
  box-sizing: border-box;
}

body {
  margin: 0;
  font-family: var(--font-sans);
  background: var(--color-bg);
  color: var(--color-text);
  line-height: 1.5;
}

a {
  color: var(--color-primary);
}

a:hover {
  color: var(--color-primary-hover);
}

nav {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-4);
  align-items: center;
  padding: var(--space-4);
  border-bottom: 1px solid var(--color-border);
  background: var(--color-surface);
}

main {
  display: block;
  max-width: 40rem;
  margin: 0 auto;
  padding: var(--space-8) var(--space-4);
}

form {
  display: grid;
  gap: var(--space-4);
  max-width: 28rem;
}

label {
  display: grid;
  gap: var(--space-1);
  font-size: 0.875rem;
  color: var(--color-muted);
}

input,
button {
  font: inherit;
}

input {
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-bg);
  color: var(--color-text);
}

button {
  padding: var(--space-2) var(--space-4);
  border: 0;
  border-radius: var(--radius-md);
  background: var(--color-primary);
  color: var(--color-on-primary);
  cursor: pointer;
}

button:hover {
  background: var(--color-primary-hover);
}
"##;
