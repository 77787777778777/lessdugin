// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project automation: `cargo xtask <command>`.
//!
//! Commands:
//!   ci                 fmt, clippy (native + wasm32), tests, lexicon lint, web checks
//!   wasm [--dev] [--budget]   build the engine wasm + JS bindings into packages/engine/pkg
//!   lex lint           lint lexicons with goat (if installed)
//!   vectors            run shared test vectors (Rust)

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Crates that must compile for wasm32-unknown-unknown (no tokio, no clocks).
const WASM_SAFE: &[&str] = &[
    "lessdugin-atp",
    "lessdugin-lexicon",
    "lessdugin-markdown",
    "lessdugin-core",
    "lessdugin-wasm",
];

/// Gzipped size budget for the engine wasm (docs/spec/limits.md).
const WASM_BUDGET_GZ: u64 = 1_100 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("wasm") => wasm(&args[1..]),
        Some("lex") => lex(&args[1..]),
        Some("vectors") => run(
            "cargo",
            &["test", "-p", "lessdugin-testkit", "--", "vectors"],
        ),
        _ => {
            eprintln!("usage: cargo xtask <ci|wasm|lex|vectors> [args]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    eprintln!("$ {cmd} {}", args.join(" "));
    let status = Command::new(cmd)
        .args(args)
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to run {cmd}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} failed: {status}"))
    }
}

fn have(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ci() -> Result<(), String> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    let mut wasm_args = vec!["clippy", "--target", "wasm32-unknown-unknown"];
    for c in WASM_SAFE {
        wasm_args.push("-p");
        wasm_args.push(c);
    }
    wasm_args.extend(["--", "-D", "warnings"]);
    run("cargo", &wasm_args)?;
    run("cargo", &["test", "--workspace"])?;
    lex(&["lint".to_owned()])?;
    if root().join("node_modules").exists() {
        run("pnpm", &["-r", "--if-present", "run", "check"])?;
        run("pnpm", &["-r", "--if-present", "run", "test"])?;
    } else {
        eprintln!("(skipping web checks: run `pnpm install` first)");
    }
    Ok(())
}

fn wasm(args: &[String]) -> Result<(), String> {
    let dev = args.iter().any(|a| a == "--dev");
    let budget = args.iter().any(|a| a == "--budget");
    let profile = if dev { "dev" } else { "wasm" };
    run(
        "cargo",
        &[
            "build",
            "-p",
            "lessdugin-wasm",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            profile,
        ],
    )?;
    let dir = if dev { "debug" } else { "wasm" };
    let wasm = root().join(format!(
        "target/wasm32-unknown-unknown/{dir}/lessdugin_wasm.wasm"
    ));
    let out = root().join("packages/engine/pkg");
    run(
        "wasm-bindgen",
        &[
            wasm.to_str().unwrap(),
            "--target",
            "web",
            "--out-dir",
            out.to_str().unwrap(),
            "--out-name",
            "lessdugin_engine",
        ],
    )?;
    let bg = out.join("lessdugin_engine_bg.wasm");
    if !dev {
        let opt = if have("wasm-opt") {
            Some("wasm-opt".to_owned())
        } else {
            let local = root().join("node_modules/.bin/wasm-opt");
            local.exists().then(|| local.to_string_lossy().into_owned())
        };
        match opt {
            Some(o) => run(
                &o,
                &[
                    "-Oz",
                    "--enable-bulk-memory",
                    "--enable-nontrapping-float-to-int",
                    "-o",
                    bg.to_str().unwrap(),
                    bg.to_str().unwrap(),
                ],
            )?,
            None => eprintln!("(wasm-opt not found; skipping size optimisation)"),
        }
    }
    let raw = std::fs::metadata(&bg).map_err(|e| e.to_string())?.len();
    let gz = gz_size(&bg)?;
    eprintln!("engine wasm: {raw} bytes raw, {gz} bytes gzipped (budget {WASM_BUDGET_GZ})");
    if budget && gz > WASM_BUDGET_GZ {
        return Err(format!(
            "wasm over budget: {gz} > {WASM_BUDGET_GZ} bytes gzipped"
        ));
    }
    Ok(())
}

fn gz_size(path: &Path) -> Result<u64, String> {
    let out = Command::new("gzip")
        .args(["-9", "-c"])
        .arg(path)
        .output()
        .map_err(|e| format!("gzip: {e}"))?;
    Ok(out.stdout.len() as u64)
}

fn lex(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("lint") => {
            let goat = ["goat", "/root/go/bin/goat"].into_iter().find(|g| {
                Command::new(g)
                    .arg("--help")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            });
            let Some(goat) = goat else {
                eprintln!("(goat not installed; skipping lexicon lint)");
                return Ok(());
            };
            run(goat, &["lex", "lint", "-I", "large-string", "lexicons"])
        }
        _ => Err("usage: cargo xtask lex lint".into()),
    }
}
