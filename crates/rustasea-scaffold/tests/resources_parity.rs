//! Resources-parity integration tests (SK-001).
//!
//! Adopts the relevant `laravel/livewire-starter-kit` `resources/` conventions
//! into the *generated* blade/livewire app templates: a nav-free auth layout
//! family, a shared head partial, macro components, a nested settings section
//! layout, the confirm-password + verify-email pages, and a plain-CSS
//! design-token entry.
//!
//! These tests also carry the regression guards for the layout refactor:
//!
//! - **Template integrity**: every `{% extends %}` / `{% include %}` /
//!   `{% import %}` target in the emitted `resources/views/**.html` tree must
//!   resolve to another emitted file (no dangling reference).
//! - **Negative**: no generated `auth/*.html` page may extend the app shell
//!   (the nav-chrome bug this task fixes).

use std::collections::{BTreeSet, HashSet};

use rustasea_scaffold::{RenderedFile, Scaffold, StarterKitVariant};

/// Render every file for `variant` without touching the filesystem.
fn render(variant: StarterKitVariant) -> Vec<RenderedFile> {
    Scaffold::new("demo-app", variant).render().expect("render")
}

/// Look up a rendered file's contents, panicking with context when absent.
fn contents<'a>(files: &'a [RenderedFile], path: &str, variant: &str) -> &'a str {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing {path} ({variant})"))
        .contents
        .as_str()
}

/// Extract the quoted target of every `{% extends "…" %}`, `{% include "…" %}`
/// and `{% import "…" as … %}` tag in `source`.
///
/// The askama `include`/`import`/`extends` directives all take a string-literal
/// path relative to the `resources/views` template root.
fn template_references(source: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for keyword in ["extends", "include", "import"] {
        let marker = format!("{{% {keyword} \"");
        let mut rest = source;
        while let Some(start) = rest.find(&marker) {
            let after = &rest[start + marker.len()..];
            if let Some(end) = after.find('"') {
                targets.push(after[..end].to_string());
            }
            rest = &after[after.find('"').map(|i| i + 1).unwrap_or(after.len())..];
        }
    }
    targets
}

/// Assert every view reference resolves to an emitted file; return the count of
/// distinct resolved targets.
fn assert_references_resolve(files: &[RenderedFile], variant: &str) -> usize {
    let emitted: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    let mut reference_count = 0usize;

    for file in files {
        if !(file.path.starts_with("resources/views/") && file.path.ends_with(".html")) {
            continue;
        }
        for target in template_references(&file.contents) {
            reference_count += 1;
            let resolved_path = format!("resources/views/{target}");
            assert!(
                emitted.contains(resolved_path.as_str()),
                "{variant}: {} references `{target}`, which is not an emitted file",
                file.path
            );
            resolved.insert(resolved_path);
        }
    }

    assert!(
        reference_count > 0,
        "{variant}: expected the view tree to contain template references"
    );
    resolved.len()
}

/// Paths the blade variant must emit for the auth layout family.
const BLADE_LAYOUT_FAMILY: &[&str] = &[
    "resources/views/layouts/app.html",
    "resources/views/layouts/auth.html",
    "resources/views/partials/head.html",
    "resources/views/partials/flash.html",
    "resources/views/components/auth-header.html",
    "resources/views/components/user-menu.html",
    "resources/views/dashboard.html",
    "resources/views/auth/login.html",
    "resources/views/auth/register.html",
    "resources/views/auth/forgot-password.html",
    "resources/views/auth/reset-password.html",
    "resources/views/auth/confirm-password.html",
    "resources/views/auth/verify-email.html",
    "resources/views/settings/layout.html",
    "resources/views/settings/profile.html",
    "resources/views/settings/password.html",
    "resources/css/app.css",
];

/// Blade emits the full auth layout family.
#[test]
fn blade_emits_the_auth_layout_family() {
    let files = render(StarterKitVariant::Blade);
    for path in BLADE_LAYOUT_FAMILY {
        assert!(
            files.iter().any(|file| file.path == *path),
            "blade must emit {path}"
        );
    }
}

/// The nav-free auth shell includes the shared head partial and no nav chrome.
#[test]
fn auth_layout_is_nav_free_and_includes_head() {
    for variant in [StarterKitVariant::Blade, StarterKitVariant::Livewire] {
        let files = render(variant);
        let auth = contents(
            &files,
            "resources/views/layouts/auth.html",
            variant.as_str(),
        );
        assert!(
            auth.contains("{% include \"partials/head.html\" %}"),
            "auth layout must include the shared head partial ({variant})"
        );
        assert!(
            auth.contains("{% block content %}{% endblock %}"),
            "auth layout must expose a content block ({variant})"
        );
        assert!(
            !auth.contains("<nav>"),
            "auth layout must not render nav chrome ({variant})"
        );
    }
}

/// The shared head partial carries the meta/favicon/stylesheet conventions and
/// deliberately holds no `<title>` (askama `include` takes no parameters).
#[test]
fn head_partial_is_the_single_head_convention() {
    let files = render(StarterKitVariant::Blade);
    let head = contents(&files, "resources/views/partials/head.html", "blade");
    for needle in [
        "<meta charset=\"utf-8\" />",
        "name=\"viewport\"",
        "rel=\"icon\"",
        "rel=\"stylesheet\"",
        "href=\"/css/app.css\"",
    ] {
        assert!(head.contains(needle), "head partial missing {needle}");
    }
    assert!(
        !head.contains("<title>"),
        "head partial must not own the title (each layout keeps its own)"
    );
}

/// Components are `{% macro %}` files (no `@props`/slots/attribute-merge).
#[test]
fn components_are_macro_files() {
    let files = render(StarterKitVariant::Blade);
    let auth_header = contents(
        &files,
        "resources/views/components/auth-header.html",
        "blade",
    );
    assert!(auth_header.contains("{% macro auth_header(title, description)"));
    assert!(auth_header.contains("endmacro %}"));
    assert!(!auth_header.contains("@props"));

    let user_menu = contents(&files, "resources/views/components/user-menu.html", "blade");
    assert!(user_menu.contains("{% macro user_menu(name)"));
    assert!(user_menu.contains("endmacro %}"));
    assert!(user_menu.contains("action=\"/logout\""));
}

/// Settings screens extend the nested settings layout, never the app shell.
#[test]
fn settings_screens_extend_the_settings_layout() {
    for variant in [StarterKitVariant::Blade, StarterKitVariant::Livewire] {
        let files = render(variant);
        let settings_layout = contents(
            &files,
            "resources/views/settings/layout.html",
            variant.as_str(),
        );
        assert!(
            settings_layout.contains("{% block settings %}{% endblock %}"),
            "settings layout must expose a settings block ({variant})"
        );
        assert!(
            settings_layout.contains("{% extends \"layouts/app.html\" %}"),
            "settings layout must extend the app shell ({variant})"
        );

        for screen in [
            "resources/views/settings/profile.html",
            "resources/views/settings/password.html",
        ] {
            let body = contents(&files, screen, variant.as_str());
            assert!(
                body.contains("{% extends \"settings/layout.html\" %}"),
                "{screen} must extend the settings layout ({variant})"
            );
            assert!(
                !body.contains("{% extends \"layouts/app.html\" %}"),
                "{screen} must not extend the app shell directly ({variant})"
            );
        }
    }
}

/// The two missing auth pages are emitted and point at real route paths.
#[test]
fn confirm_password_and_verify_email_pages_are_emitted() {
    for variant in [StarterKitVariant::Blade, StarterKitVariant::Livewire] {
        let files = render(variant);

        let confirm = contents(
            &files,
            "resources/views/auth/confirm-password.html",
            variant.as_str(),
        );
        assert!(
            confirm.contains("{% extends \"layouts/auth.html\" %}"),
            "confirm-password must extend the auth layout ({variant})"
        );
        assert!(
            confirm.contains("action=\"/confirm-password\""),
            "confirm-password must post to the real route ({variant})"
        );

        let verify = contents(
            &files,
            "resources/views/auth/verify-email.html",
            variant.as_str(),
        );
        assert!(
            verify.contains("{% extends \"layouts/auth.html\" %}"),
            "verify-email must extend the auth layout ({variant})"
        );
        // The resend action has no implemented handler yet; it stays a clearly
        // marked stub pointing at the kit's path.
        assert!(verify.contains("STUB"));
        assert!(verify.contains("action=\"/email/verification-notification\""));
    }
}

/// Regression guard (negative): no generated `auth/*.html` page extends the app
/// shell — that is the nav-chrome bug this task fixes.
#[test]
fn auth_pages_do_not_extend_the_app_layout() {
    for variant in [StarterKitVariant::Blade, StarterKitVariant::Livewire] {
        let files = render(variant);
        let auth_pages: Vec<_> = files
            .iter()
            .filter(|file| {
                file.path.starts_with("resources/views/auth/") && file.path.ends_with(".html")
            })
            .collect();
        assert!(!auth_pages.is_empty(), "no auth pages emitted ({variant})");

        for page in auth_pages {
            assert!(
                page.contents
                    .contains("{% extends \"layouts/auth.html\" %}"),
                "{} must extend the auth layout ({variant})",
                page.path
            );
            assert!(
                !page.contents.contains("layouts/app.html"),
                "{} must not extend the app layout ({variant})",
                page.path
            );
        }
    }
}

/// The livewire override applies the HTMX shell to BOTH layouts and reuses the
/// blade css entry, so auth pages are HTMX-enhanced too.
#[test]
fn livewire_overrides_both_layouts_and_keeps_the_css_entry() {
    let files = render(StarterKitVariant::Livewire);

    for layout in [
        "resources/views/layouts/app.html",
        "resources/views/layouts/auth.html",
    ] {
        let body = contents(&files, layout, "livewire");
        assert!(
            body.contains("hx-boost=\"true\""),
            "{layout} must carry hx-boost"
        );
        assert!(
            body.contains("htmx.org@2"),
            "{layout} must load the HTMX script"
        );
        assert!(
            body.contains("{% include \"partials/head.html\" %}"),
            "{layout} must include the shared head partial"
        );
    }
    assert!(
        !contents(&files, "resources/views/layouts/auth.html", "livewire").contains("<nav>"),
        "livewire auth layout must stay nav-free"
    );

    // The css entry is shared verbatim from the blade variant.
    assert!(files
        .iter()
        .any(|file| file.path == "resources/css/app.css" && file.contents.contains(":root")));

    // The livewire-only fragment partials remain.
    for path in [
        "resources/views/partials/counter.html",
        "resources/views/partials/login-form.html",
        "resources/views/partials/profile-form.html",
    ] {
        assert!(
            files.iter().any(|file| file.path == path),
            "livewire must still emit {path}"
        );
    }
}

/// The css entry is plain CSS: design tokens, a dark override, a base layer.
#[test]
fn css_entry_is_plain_design_tokens() {
    let files = render(StarterKitVariant::Blade);
    let css = contents(&files, "resources/css/app.css", "blade");
    assert!(css.contains(":root"));
    assert!(css.contains("--color-primary"));
    assert!(css.contains("[data-theme=\"dark\"]"));
    assert!(css.contains("@media (prefers-color-scheme: dark)"));
    for forbidden in ["@apply", "@tailwind", "@import", "theme("] {
        assert!(
            !css.contains(forbidden),
            "css entry must not contain `{forbidden}`"
        );
    }
}

/// Template-integrity proof: every `extends`/`include`/`import` target in the
/// emitted `resources/views/**.html` tree resolves to another emitted file.
///
/// Reports the distinct resolved-path count for each server-rendered variant.
#[test]
fn emitted_view_references_all_resolve() {
    for variant in [StarterKitVariant::Blade, StarterKitVariant::Livewire] {
        let files = render(variant);
        let resolved = assert_references_resolve(&files, variant.as_str());
        assert!(
            resolved >= 7,
            "{variant}: expected at least 7 distinct resolved template paths, got {resolved}"
        );
        eprintln!("{variant}: {resolved} distinct template references resolved");
    }
}
