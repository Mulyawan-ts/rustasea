//! `tinker` — interactive terminal REPL for a booted application.
//!
//! Launches a read-eval-print loop over [`TinkerSession`]: config inspection,
//! container resolution, route/command listing, and a friendly `help`. The
//! line-editing shell is a thin wrapper over `rustyline`; the evaluation core
//! lives in [`crate::tinker`] so it can be tested without a TTY.
//!
//! # Modes
//!
//! - **Interactive** (stdin is a terminal): a `rustyline` editor loop with
//!   history. `Ctrl+C` cancels the current line and keeps the session alive;
//!   `Ctrl+D` (EOF) and `exit`/`quit` shut down gracefully.
//! - **Piped** (stdin is not a terminal): each stdin line is evaluated in turn,
//!   so `echo "config app_name" | cargo artisan tinker` scripts the REPL.
//!
//! # Output
//!
//! Both modes stream the welcome banner and every result straight to the
//! terminal (stdout) as each line is evaluated, and errors to stderr. Nothing
//! is written into the [`Io`] buffer, so the binary's post-run flush does not
//! duplicate output after the session ends.
//!
//! Errors never abort the loop: a bad command renders a friendly message and
//! the prompt returns.

use std::io::IsTerminal;

use async_trait::async_trait;

use crate::artisan::{Command, Io};
use crate::error::CliResult;
use crate::tinker::{TinkerOutcome, TinkerSession};

/// Prompt rendered by the interactive editor.
const PROMPT: &str = "tinker> ";

/// `tinker` — interact with a booted application from the terminal.
pub struct Tinker;

#[async_trait]
impl Command for Tinker {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "tinker"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("tinker")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Interact with your application (interactive REPL)")
    }

    /// Execute: launch the REPL over the published application source.
    ///
    /// Output is streamed straight to the process streams (see the module
    /// docs), so the [`Io`] buffer is intentionally left empty: the binary
    /// prints it after `Artisan::call` returns, and writing here as well would
    /// duplicate every line once the session ends.
    async fn run(&self, _args: Vec<String>, _io: &mut Io) -> CliResult<()> {
        let session = TinkerSession::from_registry();
        if std::io::stdin().is_terminal() {
            run_interactive(&session).await;
        } else {
            run_piped(&session).await;
        }
        Ok(())
    }
}

/// Run the interactive `rustyline` loop.
///
/// Streams the banner and each result to stdout (errors to stderr) as they
/// happen, so the operator sees responses immediately rather than only after
/// exiting. Falls back to the piped reader when the editor cannot initialize
/// (for example an unsupported terminal), so `tinker` never fails to start.
async fn run_interactive(session: &TinkerSession) {
    let mut editor = match rustyline::DefaultEditor::new() {
        Ok(editor) => editor,
        Err(err) => {
            eprintln!("tinker: line editor unavailable ({err}); reading stdin");
            run_piped(session).await;
            return;
        }
    };

    print_banner();
    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                // Persisting history is best-effort: an in-memory backend never
                // fails, and a file-backed one must not abort the session.
                // Blank lines are skipped so they do not pollute history.
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match session.eval(&line).await {
                    TinkerOutcome::Print(text) => println!("{text}"),
                    TinkerOutcome::Exit => break,
                    TinkerOutcome::Noop => {}
                }
            }
            // Ctrl+C cancels the current line and keeps the session alive.
            Err(rustyline::error::ReadlineError::Interrupted) => continue,
            // Ctrl+D (EOF) shuts down gracefully.
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("tinker: {err}");
                break;
            }
        }
    }
}

/// Read stdin line-by-line, evaluating each line (non-TTY scripting mode).
///
/// Streams the banner and each result to stdout (errors to stderr) so a piped
/// session behaves like the interactive one: output appears as each line is
/// evaluated and is not replayed after the process exits.
async fn run_piped(session: &TinkerSession) {
    use std::io::BufRead as _;

    print_banner();
    let stdin = std::io::stdin();
    loop {
        // Read one line while holding the stdin lock only briefly: the guard is
        // released before the line is evaluated so the returned future stays
        // `Send` (the lock guard is not `Send`).
        let line = {
            let mut guard = stdin.lock();
            let mut buffer = String::new();
            match guard.read_line(&mut buffer) {
                Ok(0) => break,
                Ok(_) => Ok(buffer),
                Err(err) => Err(err),
            }
        };
        match line {
            Ok(line) => match session.eval(&line).await {
                TinkerOutcome::Print(text) => println!("{text}"),
                TinkerOutcome::Exit => break,
                TinkerOutcome::Noop => {}
            },
            Err(err) => {
                eprintln!("tinker: {err}");
                break;
            }
        }
    }
}

/// Print the welcome banner directly to stdout.
fn print_banner() {
    println!("RustaSea tinker — type `help` for commands, `exit` to leave.");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command advertises the `tinker` signature and a help line.
    #[test]
    fn tinker_command_metadata() {
        assert_eq!(Tinker.signature(), "tinker");
        assert!(Tinker.help().is_some());
    }

    /// Piped mode evaluates each line and continues past a friendly error.
    #[tokio::test]
    async fn piped_mode_evaluates_and_survives_errors() {
        use crate::tinker::{clear_tinker_source, set_tinker_source, TinkerSource};

        /// Serializes tests sharing the process-wide source registry.
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = LOCK.lock().await;

        /// Minimal source exposing one config key.
        struct Source;
        impl TinkerSource for Source {
            fn config(&self, key: &str) -> Option<String> {
                (key == "app_name").then(|| "\"RustaSea\"".to_string())
            }
            fn container_keys(&self) -> Vec<String> {
                Vec::new()
            }
            fn container_entry(&self, _key: &str) -> Option<String> {
                None
            }
            fn environment(&self) -> Option<String> {
                Some("local".to_string())
            }
        }

        set_tinker_source(Source);
        let session = TinkerSession::from_registry();

        let mut io = Io::default();
        for line in ["config app_name", "bogus command", "config app_name"] {
            match session.eval(line).await {
                TinkerOutcome::Print(text) => io.line(text),
                TinkerOutcome::Exit => {}
                TinkerOutcome::Noop => {}
            }
        }
        clear_tinker_source();

        assert!(
            io.stdout.contains("app_name = \"RustaSea\""),
            "{}",
            io.stdout
        );
        assert!(io.stdout.contains("unknown command"), "{}", io.stdout);
        // Two successful evals prove the session survived the bad command.
        assert_eq!(io.stdout.matches("app_name =").count(), 2, "{}", io.stdout);
    }
}
