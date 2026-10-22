// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project automation: `cargo xtask <command>`.
//!
//! Commands:
//!   ci                 fmt, clippy (native + wasm32), tests (native + wasm32), lexicon lint,
//!                      web checks
//!   test-wasm [-- <test args>]   run the interop, vector and property tests of atp, lexicon and
//!                      markdown on wasm32-unknown-unknown under node (wasm-bindgen-test-runner),
//!                      after checking that the wasm32 test binaries hold the native tests
//!   wasm [--dev] [--budget]   build the engine wasm + JS bindings into packages/engine/pkg
//!   lex lint           lint lexicons with goat (if installed)
//!   vectors            run shared test vectors (Rust)

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// Crates that must compile for wasm32-unknown-unknown (no tokio, no clocks).
const WASM_SAFE: &[&str] = &[
    "lessdugin-atp",
    "lessdugin-lexicon",
    "lessdugin-markdown",
    "lessdugin-core",
    "lessdugin-wasm",
];

/// Crates whose integration tests (interop, vector and property tests) also run on wasm32 under
/// node (`test-wasm`); every integration test of these crates must build for wasm32.
const WASM_TESTED: &[&str] = &["lessdugin-atp", "lessdugin-lexicon", "lessdugin-markdown"];

/// Gzipped size budget for the engine wasm (docs/spec/limits.md).
const WASM_BUDGET_GZ: u64 = 1_100 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("test-wasm") => test_wasm(&args[1..]),
        Some("wasm") => wasm(&args[1..]),
        Some("lex") => lex(&args[1..]),
        Some("vectors") => run(
            "cargo",
            &["test", "-p", "lessdugin-testkit", "--", "vectors"],
        ),
        _ => {
            eprintln!("usage: cargo xtask <ci|test-wasm|wasm|lex|vectors> [args]");
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
    if have(TEST_RUNNER) && have("node") {
        test_wasm(&[])?;
    } else {
        eprintln!(
            "(skipping wasm32 tests: needs node and {TEST_RUNNER}: {})",
            install_runner_hint()
        );
    }
    lex(&["lint".to_owned()])?;
    if root().join("node_modules").exists() {
        run("pnpm", &["-r", "--if-present", "run", "check"])?;
        run("pnpm", &["-r", "--if-present", "run", "test"])?;
    } else {
        eprintln!("(skipping web checks: run `pnpm install` first)");
    }
    Ok(())
}

/// The wasm-bindgen-test runner (from `wasm-bindgen-cli`); cargo runs each wasm32 test binary
/// through it, and it runs the tests in node.
const TEST_RUNNER: &str = "wasm-bindgen-test-runner";

/// The exact `wasm-bindgen` pin of the workspace (`wasm-bindgen = "=X"` in the root manifest); the
/// runner must be the same version, since it reads the test binary's wasm-bindgen metadata.
fn wasm_bindgen_version() -> Result<String, String> {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).map_err(|e| e.to_string())?;
    manifest
        .lines()
        .find_map(|l| {
            let v = l.strip_prefix("wasm-bindgen = \"=")?;
            Some(v[..v.find('"')?].to_owned())
        })
        .ok_or_else(|| "no exact `wasm-bindgen = \"=X\"` pin in Cargo.toml".to_owned())
}

fn install_runner_hint() -> String {
    match wasm_bindgen_version() {
        Ok(v) => format!("cargo install wasm-bindgen-cli --version ={v} --locked"),
        Err(e) => e,
    }
}

/// `cargo test` of the `WASM_TESTED` crates' integration tests for wasm32-unknown-unknown, run in
/// node by wasm-bindgen-test-runner. The tests are the native ones: on wasm32 they are
/// `#[wasm_bindgen_test]`s and read their fixtures from copies embedded at compile time
/// ([`check_wasm_parity`] runs first). `args` are passed on to the test binaries (e.g.
/// `-- --nocapture`).
fn test_wasm(args: &[String]) -> Result<(), String> {
    let want = wasm_bindgen_version()?;
    let out = Command::new(TEST_RUNNER).arg("--version").output();
    let got = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        _ => {
            return Err(format!(
                "{TEST_RUNNER} not found: {}",
                install_runner_hint()
            ));
        }
    };
    if got.rsplit(' ').next() != Some(want.as_str()) {
        return Err(format!(
            "`{got}` does not match the workspace wasm-bindgen {want}: {}",
            install_runner_hint()
        ));
    }
    if !have("node") {
        return Err("node not found (the wasm32 tests run in node)".into());
    }
    check_wasm_parity()?;
    let mut cargo_args = vec!["test", "--target", "wasm32-unknown-unknown"];
    for c in WASM_TESTED {
        cargo_args.extend(["-p", c]);
    }
    // integration tests only: the crates' unit tests are plain `#[test]`s (native only)
    cargo_args.extend(["--test", "*"]);
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .skip_while(|a| *a == "--")
        .collect();
    if !rest.is_empty() {
        cargo_args.push("--");
        cargo_args.extend(rest);
    }
    eprintln!(
        "$ cargo {} ({TEST_RUNNER} {want}, node)",
        cargo_args.join(" ")
    );
    let status = Command::new("cargo")
        .args(&cargo_args)
        .env("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", TEST_RUNNER)
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wasm32 tests failed: {status}"))
    }
}

/// Integration tests of `WASM_TESTED` that run natively only, as `(test file, test name)`: each
/// compares a list embedded for wasm32 with a directory on disk. Every other one must also be a
/// wasm32 test ([`check_wasm_parity`]).
const NATIVE_ONLY_TESTS: &[(&str, &str)] = &[
    (
        "crates/lexicon/tests/schemas.rs",
        "embedded_list_matches_the_directory",
    ),
    (
        "crates/markdown/tests/vectors.rs",
        "wasm_embeds_every_vector_file",
    ),
];

/// Checks that the wasm32 test binaries of `WASM_TESTED` hold exactly their native integration
/// tests, except [`NATIVE_ONLY_TESTS`]. Without it a test file whose tests are not
/// `#[wasm_bindgen_test]`s on wasm32 would pass silently: the runner says "no tests to run!" and
/// exits 0 for a binary without any.
fn check_wasm_parity() -> Result<(), String> {
    let mut native_args = vec!["test"];
    for c in WASM_TESTED {
        native_args.extend(["-p", c]);
    }
    native_args.extend(["--test", "*"]);
    let mut wasm_args = native_args.clone();
    wasm_args.extend(["--target", "wasm32-unknown-unknown"]);
    let native = test_binaries(&native_args)?;
    let wasm: BTreeMap<String, PathBuf> = test_binaries(&wasm_args)?.into_iter().collect();
    let mut problems = Vec::new();
    let (mut total, mut native_only) = (0, 0);
    for (file, exe) in &native {
        let mut want = listed_tests(Command::new(exe).args(["--list", "--format", "terse"]))?;
        let before = want.len();
        want.retain(|t| !NATIVE_ONLY_TESTS.contains(&(file.as_str(), t.as_str())));
        native_only += before - want.len();
        let Some(wasm_exe) = wasm.get(file) else {
            problems.push(format!("{file}: no wasm32 test binary"));
            continue;
        };
        let got = listed_tests(Command::new(TEST_RUNNER).arg(wasm_exe).arg("--list"))?;
        for t in want.iter().filter(|t| !got.contains(t)) {
            problems.push(format!("{file}: `{t}` is not a wasm32 test"));
        }
        for t in got.iter().filter(|t| !want.contains(t)) {
            problems.push(format!("{file}: `{t}` is a wasm32 test only"));
        }
        total += got.len();
    }
    for file in wasm.keys().filter(|f| !native.iter().any(|(n, _)| n == *f)) {
        problems.push(format!("{file}: a wasm32 test binary only"));
    }
    if native_only != NATIVE_ONLY_TESTS.len() {
        problems.push(format!(
            "NATIVE_ONLY_TESTS lists {} tests, {native_only} of them exist",
            NATIVE_ONLY_TESTS.len()
        ));
    }
    if !problems.is_empty() {
        return Err(format!(
            "the wasm32 tests differ from the native ones (on wasm32 each test file needs \
             `use wasm_bindgen_test::wasm_bindgen_test as test;`; a native-only test belongs in \
             NATIVE_ONLY_TESTS):\n  {}",
            problems.join("\n  ")
        ));
    }
    eprintln!(
        "wasm32 has the native tests: {total} in {} test files ({native_only} native-only)",
        native.len()
    );
    Ok(())
}

/// The test binaries `cargo <args> --no-run` builds, as (test file relative to the repository,
/// executable), from cargo's JSON messages.
fn test_binaries(args: &[&str]) -> Result<Vec<(String, PathBuf)>, String> {
    let mut all = args.to_vec();
    all.extend(["--no-run", "--message-format=json-render-diagnostics"]);
    eprintln!("$ cargo {}", all.join(" "));
    let out = Command::new("cargo")
        .args(&all)
        .current_dir(root())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo failed: {}", out.status));
    }
    let root = root();
    let mut bins = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !line.contains(r#""reason":"compiler-artifact""#) || !line.contains(r#""kind":["test"]"#)
        {
            continue;
        }
        if let (Some(src), Some(exe)) = (json_str(line, "src_path"), json_str(line, "executable")) {
            let src = PathBuf::from(src);
            let rel = src.strip_prefix(&root).unwrap_or(&src);
            bins.push((rel.to_string_lossy().replace('\\', "/"), PathBuf::from(exe)));
        }
    }
    if bins.is_empty() {
        return Err(format!("`cargo {}` built no test binaries", all.join(" ")));
    }
    bins.sort();
    Ok(bins)
}

/// The value of the first `"key":"…"` string in a line of compact JSON.
fn json_str(line: &str, key: &str) -> Option<String> {
    let start = line.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = line[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    None
}

/// The tests a test binary lists (`<name>: test` lines of `--list`), sorted.
fn listed_tests(cmd: &mut Command) -> Result<Vec<String>, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to list tests: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "listing tests failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .map(str::to_owned)
        .collect();
    names.sort();
    Ok(names)
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
