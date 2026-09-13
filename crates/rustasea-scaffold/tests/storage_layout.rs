//! Storage-layout integration tests (STG-002).
//!
//! The generated `storage/` tree mirrors the laravel/livewire-starter-kit
//! convention: each runtime directory carries its own self-contained
//! `.gitignore`, and the generated root `.gitignore` holds no central storage
//! rules. Every disk/link target declared by `config/storage.toml` therefore
//! has a real, correctly-ignored parent directory.

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// The five per-directory `.gitignore` files every variant must emit.
const STORAGE_GITIGNORES: &[&str] = &[
    "storage/app/.gitignore",
    "storage/app/public/.gitignore",
    "storage/logs/.gitignore",
    "storage/framework/.gitignore",
    "storage/archive/.gitignore",
];

#[test]
fn every_variant_emits_kit_style_storage_gitignores() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");

        for path in STORAGE_GITIGNORES {
            assert!(
                files.iter().any(|file| file.path == *path),
                "missing storage gitignore {path} ({variant})"
            );
        }

        // No `.gitkeep` markers remain anywhere under `storage/`.
        assert!(
            !files
                .iter()
                .any(|file| file.path.starts_with("storage/") && file.path.ends_with(".gitkeep")),
            "storage must not emit .gitkeep files ({variant})"
        );

        // The generated root `.gitignore` keeps only the non-storage rules.
        let root_ignore = files
            .iter()
            .find(|file| file.path == ".gitignore")
            .expect(".gitignore present")
            .contents
            .as_str();
        assert!(root_ignore.contains("/target"));
        assert!(root_ignore.contains("/.env"));
        assert!(root_ignore.contains("/database/*.sqlite"));
        assert!(
            !root_ignore.contains("/storage/"),
            "root .gitignore must not carry storage rules ({variant}):\n{root_ignore}"
        );

        // Spot-check the load-bearing per-directory contents.
        let find = |path: &str| {
            files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("missing {path} ({variant})"))
                .contents
                .as_str()
        };
        assert!(find("storage/app/.gitignore").contains("!public/"));
        assert!(find("storage/app/public/.gitignore").contains("!.gitignore"));
        assert!(find("storage/logs/.gitignore").contains("!.gitignore"));
        assert!(find("storage/archive/.gitignore").contains("!.gitignore"));
        // `framework/` uses the same self-contained pattern as the other dirs:
        // ignore all runtime state while keeping `.gitignore` itself trackable.
        assert!(find("storage/framework/.gitignore").contains("MAINTENANCE_MARKER"));
        assert!(find("storage/framework/.gitignore").contains("*"));
        assert!(find("storage/framework/.gitignore").contains("!.gitignore"));
    }
}

/// The generated tree actually writes the files to disk, not just the render.
#[test]
fn generated_tree_writes_storage_gitignores() {
    let root = std::env::temp_dir().join(format!(
        "rustasea-scaffold-storage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create temp dir");

    Scaffold::new("demo-app", StarterKitVariant::Blade)
        .generate(&root)
        .expect("generate");

    for path in STORAGE_GITIGNORES {
        assert!(root.join(path).is_file(), "missing on disk: {path}");
    }
    assert!(!root.join("storage/app/.gitkeep").exists());
    assert!(!root.join("storage/logs/.gitkeep").exists());

    let root_ignore = std::fs::read_to_string(root.join(".gitignore")).expect("read .gitignore");
    assert!(!root_ignore.contains("/storage/"));

    std::fs::remove_dir_all(&root).ok();
}
