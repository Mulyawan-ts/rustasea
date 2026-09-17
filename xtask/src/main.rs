//! RustaSea `cargo xtask` entrypoint.
//!
//! Provides the CI-facing task surface documented for M5: `cargo xtask ci`
//! gates the workspace on rustfmt + clippy (C-04), `cargo xtask deps:check`
//! fails on dependency-version drift from `[workspace.dependencies]`
//! (ADOPT-031), `cargo xtask check-cycles` validates the crate DAG stays
//! acyclic (architecture §3), `cargo xtask lines:check` enforces the 500-line
//! file limit (ADR-0009), and `cargo xtask migrate` runs the framework's
//! registered migrations. The `docker:up` / `docker:down` / `docker:logs` tasks
//! drive the dev compose stack (ADOPT-007). Toolchain tasks shell out to
//! `cargo`; graph analysis, dependency scanning, line-limit enforcement, and
//! migration execution live in submodules.

mod cycles;
mod deps;
mod docker;
mod lines;
mod migrate;

use std::process::Command;

/// Exit code returned on task failure.
pub(crate) const FAILURE: i32 = 1;

/// Program entry point: dispatch the first CLI argument as the task name.
fn main() {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_else(|| "ci".to_string());
    let rest: Vec<String> = args.collect();
    let code = match task.as_str() {
        "ci" => run_ci(),
        "fmt" => run("cargo", &["fmt", "--all", "--", "--check"]),
        "clippy" => run(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        "check-cycles" => cycles::run(),
        "deps:check" => deps::run(),
        "lines:check" => lines::run(),
        "migrate" => migrate::run(&rest),
        "docker:up" => docker::up(),
        "docker:down" => docker::down(),
        "docker:logs" => docker::logs(&rest),
        other => {
            eprintln!(
                "xtask: unknown task `{other}` (expected ci|fmt|clippy|check-cycles|deps:check|lines:check|migrate|docker:up|docker:down|docker:logs)"
            );
            FAILURE
        }
    };
    std::process::exit(code);
}

/// Run the full CI gate: fmt → clippy → deps:check → cycle check.
fn run_ci() -> i32 {
    let steps: &[(&str, &[&str])] = &[
        ("fmt", &["fmt", "--all", "--", "--check"]),
        (
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
    ];
    for (label, args) in steps {
        println!("xtask ci: running {label}…");
        let code = run("cargo", args);
        if code != 0 {
            eprintln!("xtask ci: {label} failed with exit code {code}");
            return code;
        }
    }
    println!("xtask ci: checking workspace dependencies…");
    let code = deps::run();
    if code != 0 {
        eprintln!("xtask ci: deps:check failed with exit code {code}");
        return code;
    }
    println!("xtask ci: checking file line limits…");
    let code = lines::run();
    if code != 0 {
        eprintln!("xtask ci: lines:check failed with exit code {code}");
        return code;
    }
    println!("xtask ci: checking crate DAG cycles…");
    cycles::run()
}

/// Run one command, inheriting stdio; returns its exit code.
fn run(program: &str, args: &[&str]) -> i32 {
    Command::new(program)
        .args(args)
        .status()
        .map(|status| status.code().unwrap_or(FAILURE))
        .unwrap_or_else(|err| {
            eprintln!("xtask: failed to run {program}: {err}");
            FAILURE
        })
}
