//! Laravel-parity port surface for generated apps (TASK-101).
//!
//! Laravel's `php artisan serve` listens on port 8000 and the framework's own
//! `rustasea-app` binary binds `0.0.0.0:8000`. The scaffold must match that
//! surface end to end: the app on 8000, the advertised `APP_URL` on 8000, the
//! Docker image/compose/healthcheck on 8000, and the generated `main.rs`
//! deriving its bind address from `APP_URL` rather than hard-coding a port.
//!
//! The negative oracle (no rendered file may contain the literal `3000`) is the
//! regression guard: it fails the moment any template drifts back to the old
//! port, including in files this suite does not enumerate by name.

use rustasea_scaffold::{RenderedFile, Scaffold, StarterKitVariant};

/// Look up a rendered file by path, panicking with the variant on absence.
fn find<'a>(files: &'a [RenderedFile], path: &str, variant: StarterKitVariant) -> &'a str {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing {path} ({variant})"))
        .contents
        .as_str()
}

/// Every variant advertises and serves port 8000 across the generated surface.
#[test]
fn every_variant_uses_laravel_parity_port_8000() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");

        let main_rs = find(&files, "main.rs", variant);
        assert!(
            main_rs.contains("0.0.0.0:8000"),
            "main.rs must bind the Laravel-parity default 0.0.0.0:8000 ({variant})"
        );

        let env = find(&files, ".env.example", variant);
        assert!(
            env.contains("APP_URL=http://localhost:8000"),
            ".env.example must advertise APP_URL=http://localhost:8000 ({variant})"
        );

        let dockerfile = find(&files, "Dockerfile", variant);
        assert!(
            dockerfile.contains("EXPOSE 8000"),
            "Dockerfile must EXPOSE 8000 ({variant})"
        );

        let compose = find(&files, "docker-compose.yml", variant);
        assert!(
            compose.contains("\"8000:8000\""),
            "docker-compose.yml must publish 8000:8000 ({variant})"
        );
        assert!(
            compose.contains("http://localhost:8000"),
            "docker-compose.yml must default APP_URL to http://localhost:8000 ({variant})"
        );

        let readme = find(&files, "README.md", variant);
        assert!(
            readme.contains("8000"),
            "README must mention the 8000 port ({variant})"
        );
    }
}

/// The generated `main.rs` derives its bind address from `APP_URL`, so the
/// advertised URL and the listening socket cannot diverge (the pre-existing bug).
#[test]
fn generated_main_rs_derives_bind_from_app_url() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let main_rs = find(&files, "main.rs", variant);

        assert!(
            main_rs.contains("APP_URL"),
            "main.rs must read APP_URL to resolve its bind address ({variant})"
        );
        assert!(
            main_rs.contains("parse_host_port"),
            "main.rs must parse APP_URL host:port via parse_host_port ({variant})"
        );
        assert!(
            main_rs.contains("bind_address"),
            "main.rs must resolve the bind through bind_address ({variant})"
        );
    }
}

/// Negative oracle: no rendered file may contain the retired `3000` literal.
#[test]
fn no_rendered_file_contains_the_retired_port() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        for file in &files {
            assert!(
                !file.contents.contains("3000"),
                "retired port 3000 leaked into {} ({variant})",
                file.path
            );
        }
    }
}
