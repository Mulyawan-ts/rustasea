//! `make:seeder` template — database/seeders/<snake>.rs.

use std::path::Path;

use crate::error::CliResult;
use crate::generator::Generated;
use crate::generators::kinds::{slug, write_scaffold};
use crate::generators::MakeOptions;

/// Render and write the database seeder file.
pub fn scaffold(root: &Path, opts: &MakeOptions) -> CliResult<Generated> {
    let rel = format!("database/seeders/{}.rs", slug(&opts.name));
    let source = format!(
        r#"//! Database seeder scaffold — {name}.
//!
//! `run` (the trait default) executes `sql` against the live pool; keep the
//! statements idempotent so re-running never duplicates rows
//! (`INSERT … ON CONFLICT DO NOTHING`).
//!
//! For realistic fake data, enable the umbrella crate's `faker` feature
//! (`rustasea = {{ version = "0.1", features = ["faker"] }}`) and drive a
//! seeded [`Faker`](rustasea::testing::faker::Faker) so a seed reproduces the
//! same rows every run:
//!
//! ```ignore
//! use rustasea::testing::faker::Faker;
//!
//! let mut faker = Faker::from_config(42, "en_US");
//! let email = faker.unique_email().expect("unique email");
//! let name = faker.name();
//! ```

use rustasea::orm::migration::Seeder;
use rustasea::orm::Result;

/// Seeds baseline rows for the {kind} domain.
pub struct {name};

impl Seeder for {name} {{
    /// Seeder name reported by `migrate --seed`.
    fn name(&self) -> &str {{
        "{name}"
    }}

    /// Idempotent SQL statements executed against the database.
    fn sql(&self) -> Result<String> {{
        Ok("-- {name}: add idempotent INSERT statements here".to_string())
    }}
}}
"#,
        kind = slug(&opts.name),
        name = opts.name,
    );
    write_scaffold(root, rel, source, opts.force)
}
