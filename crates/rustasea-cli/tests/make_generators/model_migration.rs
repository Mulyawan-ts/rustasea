//! Model, migration, force-flag, and invalid-name handling for the class-based
//! generators.

use crate::common::temp_root;

use rustasea_cli::generators::{generate, Kind, MakeOptions};

/// `make:model -m` appends a timestamped migration alongside the model.
#[test]
fn model_with_migration_writes_two_files() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "Post".to_string(),
        force: false,
        resource: false,
        with_migration: true,
        browser: false,
    };
    let files = generate(Kind::Model, &root, &opts).expect("model + migration scaffold");
    assert_eq!(files.len(), 2);

    let model = root.join("app/models/post.rs");
    let migration_rel = files[1].path.clone();
    let migration = root.join(&migration_rel);
    assert!(
        migration.is_file(),
        "migration not written at {migration_rel}"
    );
    assert!(
        migration_rel.contains("_create_posts_table.rs"),
        "unexpected migration path: {migration_rel}"
    );
    let source = std::fs::read_to_string(&migration).expect("read migration");
    assert!(source.contains("CreatePostTable"));

    assert!(model.is_file());
    let _ = std::fs::remove_dir_all(&root);
}

/// `make:migration` accepts a snake_case name and prefixes a timestamp.
#[test]
fn migration_generator_writes_timestamped_file() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "create_users_table".to_string(),
        force: false,
        resource: false,
        with_migration: false,
        browser: false,
    };
    let files = generate(Kind::Migration, &root, &opts).expect("migration scaffold");
    assert_eq!(files.len(), 1);

    let rel = files[0].path.clone();
    let target = root.join(&rel);
    assert!(target.is_file(), "migration not written at {rel}");
    let file_name = target.file_name().expect("file name").to_string_lossy();
    assert!(
        file_name.starts_with("20"),
        "expected timestamp prefix, got {file_name}"
    );

    let source = std::fs::read_to_string(&target).expect("read migration");
    assert!(source.contains("CreateUsersTable"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A second run without `--force` fails; with `--force` it overwrites.
#[test]
fn existing_scaffold_respects_force_flag() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "Post".to_string(),
        force: false,
        resource: false,
        with_migration: false,
        browser: false,
    };
    generate(Kind::Model, &root, &opts).expect("first scaffold");

    let again = generate(Kind::Model, &root, &opts);
    assert!(
        again.is_err(),
        "second scaffold without --force must report AlreadyExists"
    );

    let forced = generate(
        Kind::Model,
        &root,
        &MakeOptions {
            name: "Post".to_string(),
            force: true,
            resource: false,
            with_migration: false,
            browser: false,
        },
    );
    assert!(forced.is_ok(), "scaffold with --force must overwrite");
    let _ = std::fs::remove_dir_all(&root);
}

/// Invalid class names are rejected before any file is written.
#[test]
fn invalid_class_name_is_rejected() {
    let root = temp_root();
    let opts = MakeOptions {
        name: "post_controller".to_string(), // snake_case where PascalCase required
        force: false,
        resource: false,
        with_migration: false,
        browser: false,
    };
    assert!(generate(Kind::Controller, &root, &opts).is_err());
    assert!(!root
        .join("app/http/controllers/post_controller.rs")
        .exists());
    let _ = std::fs::remove_dir_all(&root);
}
