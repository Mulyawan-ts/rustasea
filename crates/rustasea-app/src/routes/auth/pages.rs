//! Static auth page markup (split out of [`super`] for the 500-line cap).
//!
//! These are the placeholder login/registration forms. They live here so the
//! auth route table in [`super`] stays under the file-size limit; the markup is
//! intentionally dependency-free (no template directory ships with the app).

/// Login page markup (placeholder form).
pub(super) const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Log in</title></head>
<body><main><h1>Log in</h1>
<form method="post" action="/login">
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Log in</button>
</form>
<p><a href="/register">Create an account</a></p>
</main></body></html>"#;

/// Registration page markup (placeholder form).
pub(super) const REGISTER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Register</title></head>
<body><main><h1>Register</h1>
<form method="post" action="/register">
<label>Name <input type="text" name="name" required></label>
<label>Email <input type="email" name="email" required></label>
<label>Password <input type="password" name="password" required></label>
<button type="submit">Register</button>
</form>
<p><a href="/login">Already registered?</a></p>
</main></body></html>"#;
