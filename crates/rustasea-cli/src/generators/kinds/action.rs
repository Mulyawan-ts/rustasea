//! `make:action` template — app/actions/<snake>.rs.
//!
//! An action is one unit of work with a typed input/output/error and optional
//! `validate`/`authorize` hooks. The same action is driven from HTTP
//! (`ActionController`), the queue (`ActionJob`), the CLI (`ActionCommand`), and
//! events (`ActionListener`) without duplicating the domain logic.

use std::path::Path;

use crate::error::CliResult;
use crate::generator::Generated;
use crate::generators::kinds::{slug, write_scaffold};
use crate::generators::MakeOptions;

/// Render and write the action file.
pub fn scaffold(root: &Path, opts: &MakeOptions) -> CliResult<Generated> {
    let rel = format!("app/actions/{}.rs", slug(&opts.name));
    write_scaffold(root, rel, source(&opts.name), opts.force)
}

/// The rustfmt-clean action source for `name`.
fn source(name: &str) -> String {
    format!(
        r#"//! Action scaffold — {name}.
//!
//! One unit of work invoked from HTTP, the queue, the CLI, or an event.
//! Implement `handle` with the domain logic; override `validate`/`authorize`
//! to add hooks. Register the action with `ActionController`/`ActionJob`/
//! `ActionCommand`/`ActionListener` as needed.

use rustasea::action::{{async_trait, Action, ActionError}};

/// Input accepted by the {name} action.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct {name}Input {{
    /// TODO: replace with the fields this action needs.
    pub id: i64,
}}

/// Output produced by the {name} action.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct {name}Output {{
    /// TODO: replace with the fields this action returns.
    pub ok: bool,
}}

/// The {name} action.
pub struct {name};

#[async_trait]
impl Action for {name} {{
    type Input = {name}Input;
    type Output = {name}Output;
    type Error = ActionError;

    /// Execute the action body.
    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error> {{
        let _ = input;
        todo!("implement the action")
    }}
}}
"#,
        name = name,
    )
}
