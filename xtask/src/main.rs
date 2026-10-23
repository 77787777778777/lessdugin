// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project automation: `cargo xtask <command>`.
//!
//! Commands:
//!   ci                 fmt, clippy (native + wasm32), tests (native + wasm32; the social
//!                      vectors also with `final-ns`), lexicon lint, web checks
//!   test-wasm [-- <test args>]   run the interop, vector and property tests of atp, lexicon and
//!                      markdown on wasm32-unknown-unknown under node (wasm-bindgen-test-runner),
//!                      after checking that the wasm32 test binaries hold the native tests
//!   wasm [--release|--dev] [--budget] [--report] [--kv-test]
//!                      build the engine wasm + JS bindings into packages/engine/pkg
//!                      (wasm-bindgen = the workspace pin, then binaryen wasm-opt) and report
//!                      its raw and gzipped size; --budget fails above 2.5 MB gzipped (warns
//!                      above 1.5 MB gzipped or 4 MB raw), --report adds the `verify` marginal, --kv-test
//!                      builds the test-only KV conformance module into pkg/kvtest
//!   e2e [--rust] [--web] [--parity] [--no-build]
//!                      the end-to-end tests against the local devnet (ADR 0006;
//!                      packages/e2e/README.md): --rust the Rust e2e tests (LESSDUGIN_E2E=1),
//!                      --web the SPA's devnet Playwright suite including the M2 parity
//!                      scenario, --parity only that scenario; none of them: --rust and --web.
//!                      The web runs first build the engine wasm, the e2e SPA, the CLI (embedding
//!                      it) and the TUI parity driver, unless --no-build
//!   lex lint           lint lexicons with goat (if installed)
//!   vectors            run shared test vectors (Rust)
//!   fuzz [<target>…|all] [--seconds N] [--jobs J] [--sanitizer S] [--list] [-- <libFuzzer args>]
//!                      seed the cargo-fuzz corpora (fuzz/, its own workspace) from the vectors
//!                      and fixtures, then run each target for N seconds (default: every target,
//!                      600 s each, the M4 acceptance run) with cargo-fuzz on a nightly toolchain
//!                      ($LESSDUGIN_FUZZ_TOOLCHAIN, default `nightly`); see fuzz/README.md

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

/// The engine wasm's `gzip -9` size (after wasm-opt) above which `wasm --budget` fails. It is a
/// tripwire for something linked in by mistake (tokio, reqwest, ammonia, debug info), not a
/// performance budget (ADR 0008, superseding ADR 0002 decision 4; sizes are decimal).
const WASM_BUDGET_GZ: u64 = 2_500_000;
/// The gzipped size above which `wasm` prints a warning (ADR 0008).
const WASM_TARGET_GZ: u64 = 1_500_000;
/// The raw size above which `wasm` warns (compile time scales with raw bytes; ADR 0008).
const WASM_WARN_RAW: u64 = 4_000_000;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("test-wasm") => test_wasm(&args[1..]),
        Some("wasm") => wasm(&args[1..]),
        Some("e2e") => e2e(&args[1..]),
        Some("lex") => lex(&args[1..]),
        Some("vectors") => run(
            "cargo",
            &["test", "-p", "lessdugin-testkit", "--", "vectors"],
        ),
        Some("fuzz") => fuzz(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <ci|test-wasm|wasm|e2e|lex|vectors|fuzz> [args]");
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

/// [`run`] with extra environment variables.
fn run_env(cmd: &str, args: &[&str], vars: &[(String, String)]) -> Result<(), String> {
    let shown: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    eprintln!("$ {} {cmd} {}", shown.join(" "), args.join(" "));
    let status = Command::new(cmd)
        .args(args)
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to run {cmd}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} failed: {status}"))
    }
}

/// Cargo's target directory (`CARGO_TARGET_DIR`, else `<root>/target`).
fn target_dir() -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(d) if !d.is_empty() => root().join(d),
        _ => root().join("target"),
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
    // the social-fold vectors at the final namespace (the vectors are written in it; the default
    // build runs only the namespace-independent parts)
    run(
        "cargo",
        &[
            "test",
            "-p",
            "lessdugin-core",
            "--features",
            "final-ns",
            "--test",
            "social_vectors",
            "--test",
            "social_fold",
            "--test",
            "social_query",
            "--test",
            "model_reorder",
        ],
    )?;
    // the TypeScript form of the DTOs (testdata/dto/dto-1.d.ts, which the engine package's
    // types.parity.test.ts compares types.ts with) is what the Rust types generate
    run(
        "cargo",
        &[
            "test",
            "-p",
            "lessdugin-wasm",
            "--features",
            "dto-decl",
            "--test",
            "dto_decl",
        ],
    )?;
    // M2 acceptance: the 50-member cold sync in under 5 s, on an optimised build (the debug
    // run above asserts it only off CI; a release build has devnet mode only with `devnet`)
    run(
        "cargo",
        &[
            "test",
            "--release",
            "-p",
            "lessdugin",
            "--features",
            "devnet",
            "--test",
            "cold_sync",
        ],
    )?;
    if have(TEST_RUNNER) && have("node") {
        test_wasm(&[])?;
    } else {
        eprintln!(
            "(skipping wasm32 tests: needs node and {TEST_RUNNER}: {})",
            install_runner_hint()
        );
    }
    lex(&["lint".to_owned()])?;
    // the engine wasm and its size report, and the KV conformance module the engine package's
    // tests load; both before the web checks, which type-check against pkg/. Being over the size
    // budget fails the run after the web checks, so they still report.
    let mut over_budget = None;
    if have("wasm-bindgen") {
        over_budget = wasm_build(&["--budget".to_owned(), "--report".to_owned()])?;
        wasm(&["--kv-test".to_owned()])?;
    } else {
        eprintln!(
            "(skipping the engine wasm build and size report: needs wasm-bindgen: {})",
            install_runner_hint()
        );
    }
    if root().join("node_modules").exists() {
        run("pnpm", &["-r", "--if-present", "run", "check"])?;
        run("pnpm", &["-r", "--if-present", "run", "test"])?;
    } else {
        eprintln!("(skipping web checks: run `pnpm install` first)");
    }
    match over_budget {
        Some(over) => Err(over),
        None => Ok(()),
    }
}

/// The Rust end-to-end tests against the devnet: (package, test targets). They run only with
/// `LESSDUGIN_E2E=1`; each starts its own devnet (`packages/e2e/devnet.mjs`) unless
/// `LESSDUGIN_DEVNET_JSON` names a running one.
const RUST_E2E: &[(&str, &[&str])] = &[
    // identity resolution, login binding and `verify`
    ("lessdugin-testkit", &["e2e_devnet"]),
    // the native host against the reference PDS
    ("lessdugin-native", &["devnet"]),
    // the CLI: app-password and OAuth logins, concurrent refresh, the forum scenario with
    // parity mode, `lessdugin web`'s browser sign-in
    ("lessdugin", &["e2e_devnet", "e2e_forum", "web_e2e_devnet"]),
    // the TUI's reply through $EDITOR and its parity capture
    ("lessdugin-tui", &["e2e_devnet"]),
];

/// `cargo xtask e2e`: the end-to-end tests against the local devnet (`docs/plan.md` §4.1,
/// ADR 0006). `--rust`: [`RUST_E2E`]. `--web`: the SPA's devnet Playwright suite
/// (`web/e2e/devnet`: hosted mode, parity mode, `lessdugin web`, and the M2 parity scenario
/// `scenario.spec.ts`). `--parity`: only the parity scenario. Without a selection, `--rust` and
/// `--web`. The Playwright runs first build what they use (unless `--no-build`): the engine
/// wasm, the e2e SPA (`web/dist-e2e`), the CLI embedding it (so the `lessdugin web` spec runs)
/// and the TUI parity driver (`crates/tui/examples/parity_driver.rs`).
fn e2e(args: &[String]) -> Result<(), String> {
    let (mut rust, mut web, mut parity, mut build) = (false, false, false, true);
    for a in args {
        match a.as_str() {
            "--rust" => rust = true,
            "--web" => web = true,
            "--parity" => parity = true,
            "--no-build" => build = false,
            _ => {
                return Err(format!(
                    "usage: cargo xtask e2e [--rust] [--web] [--parity] [--no-build] (unknown \
                     argument {a})"
                ));
            }
        }
    }
    if !(rust || web || parity) {
        rust = true;
        web = true;
    }
    let mut vars: Vec<(String, String)> = vec![("LESSDUGIN_E2E".into(), "1".into())];
    // Chromium: the preinstalled one when the environment names none
    if env::var_os("PLAYWRIGHT_BROWSERS_PATH").is_none() && Path::new("/opt/pw-browsers").exists() {
        vars.push(("PLAYWRIGHT_BROWSERS_PATH".into(), "/opt/pw-browsers".into()));
    }
    // The CLI tests' OAuth approver loads Playwright from PLAYWRIGHT_MODULE; pnpm keeps it out
    // of the root node_modules, so point it at the web package's copy (the pinned version).
    let global_playwright = Path::new("/opt/node22/lib/node_modules/playwright");
    let web_playwright = root().join("web/node_modules/@playwright/test");
    if env::var_os("PLAYWRIGHT_MODULE").is_none()
        && !global_playwright.exists()
        && web_playwright.exists()
    {
        vars.push((
            "PLAYWRIGHT_MODULE".into(),
            web_playwright.display().to_string(),
        ));
    }
    if rust {
        for (package, tests) in RUST_E2E {
            let mut a = vec!["test", "-p", package];
            for t in *tests {
                a.extend(["--test", t]);
            }
            run_env("cargo", &a, &vars)?;
        }
    }
    if web || parity {
        let dist = root().join("web/dist-e2e").display().to_string();
        if build {
            wasm(&[])?;
            run("pnpm", &["--filter", "@lessdugin/web", "run", "build:e2e"])?;
            // the CLI embeds the e2e SPA, so bridge.spec.ts (`lessdugin web`) runs too
            run_env(
                "cargo",
                &["build", "-p", "lessdugin"],
                &[("LESSDUGIN_WEB_DIST".into(), dist.clone())],
            )?;
            run(
                "cargo",
                &["build", "-p", "lessdugin-tui", "--example", "parity_driver"],
            )?;
            eprintln!(
                "note: the debug `lessdugin` now embeds the e2e SPA ({dist}); the next \
                 `cargo build -p lessdugin` without LESSDUGIN_WEB_DIST embeds web/dist again"
            );
        }
        let debug = target_dir().join("debug");
        vars.push((
            "LESSDUGIN_BIN".into(),
            debug.join("lessdugin").display().to_string(),
        ));
        vars.push((
            "LESSDUGIN_TUI_DRIVER".into(),
            debug.join("examples/parity_driver").display().to_string(),
        ));
        // the scenario fails instead of skipping when a binary is missing
        vars.push(("LESSDUGIN_PARITY_REQUIRED".into(), "1".into()));
        let mut a = vec![
            "--filter",
            "@lessdugin/web",
            "exec",
            "playwright",
            "test",
            "-c",
            "e2e/devnet/playwright.config.ts",
        ];
        if !web {
            a.push("scenario.spec.ts");
        }
        run_env("pnpm", &a, &vars)?;
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

/// `wasm` options.
#[derive(Default)]
struct WasmOpts {
    /// Dev profile, no wasm-opt (fast; not measured against the budget).
    dev: bool,
    /// Fail above [`WASM_BUDGET_GZ`] (warn above [`WASM_WARN_RAW`] in any case).
    budget: bool,
    /// Also build without `verify` and print its marginal size (ADR 0002 rule 5).
    report: bool,
    /// The test-only `kv-conformance` build into `packages/engine/pkg/kvtest` (dev profile).
    kv_test: bool,
}

fn wasm_opts(args: &[String]) -> Result<WasmOpts, String> {
    let mut o = WasmOpts::default();
    for a in args {
        match a.as_str() {
            "--dev" => o.dev = true,
            "--release" => o.dev = false,
            "--budget" => o.budget = true,
            "--report" => o.report = true,
            "--kv-test" => o.kv_test = true,
            _ => {
                return Err(format!(
                    "usage: cargo xtask wasm [--release|--dev] [--budget] [--report] [--kv-test] \
                     (unknown argument {a})"
                ));
            }
        }
    }
    Ok(o)
}

/// The engine's sizes after wasm-bindgen and wasm-opt.
struct WasmSize {
    raw: u64,
    gz: u64,
    glue_gz: u64,
    optimised: bool,
}

/// `cargo xtask wasm`: builds `lessdugin-wasm` for wasm32 (profile `wasm`: opt-level z, fat LTO,
/// panic=abort), runs `wasm-bindgen --target web` (the CLI must equal the workspace pin) into
/// `packages/engine/pkg`, optimises with binaryen's `wasm-opt` (ADR 0002's flags) and reports the
/// raw and `gzip -9` sizes. `--budget` fails above 2.5 MB gzipped; above 1.5 MB gzipped or 4 MB raw
/// it warns.
/// `--report` adds the marginal size of the `verify` feature. `--kv-test` builds the test-only
/// `kv-conformance` module into `packages/engine/pkg/kvtest` for the engine package's vitest.
fn wasm(args: &[String]) -> Result<(), String> {
    match wasm_build(args)? {
        Some(over) => Err(over),
        None => Ok(()),
    }
}

/// `cargo xtask wasm`; `Ok(Some(message))` when `--budget` is given and the module is over it
/// (the build itself succeeded, so later checks can still use `pkg/`).
fn wasm_build(args: &[String]) -> Result<Option<String>, String> {
    let o = wasm_opts(args)?;
    check_wasm_bindgen_cli()?;
    check_no_ammonia()?;
    if o.kv_test {
        let out = root().join("packages/engine/pkg/kvtest");
        build_wasm(&["--features", "kv-conformance"], "dev", &out, false)?;
        eprintln!(
            "kv-conformance module: {}",
            out.join("lessdugin_engine_bg.wasm").display()
        );
        return Ok(None);
    }
    let profile = if o.dev { "dev" } else { "wasm" };
    let out = root().join("packages/engine/pkg");
    let size = build_wasm(&[], profile, &out, !o.dev)?;
    let mut report = vec![format!(
        "engine wasm ({}): {} raw, {} gzipped (warn above {}, fail above {}); JS glue {} gzipped",
        if o.dev {
            "dev, not optimised"
        } else {
            "release"
        },
        bytes(size.raw),
        bytes(size.gz),
        bytes(WASM_TARGET_GZ),
        bytes(WASM_BUDGET_GZ),
        bytes(size.glue_gz),
    )];
    if !size.optimised && !o.dev {
        report.push("wasm-opt not found: the sizes are not final (install binaryen)".into());
    }
    if o.report && !o.dev {
        let tmp = root().join("target/wasm-report/no-verify");
        let nv = build_wasm(
            &["--no-default-features", "--features", "math"],
            profile,
            &tmp,
            true,
        )?;
        let marginal = size.gz as i64 - nv.gz as i64;
        report.push(format!(
            "without `verify`: {} raw, {} gzipped; verify's marginal size {marginal} bytes gzipped \
             (ADR 0008: split into a lazy verify.wasm only above the 1.5 MB gz warning line with a \
             marginal of at least 100 KB gz)",
            bytes(nv.raw),
            bytes(nv.gz)
        ));
    }
    let mut failed = None;
    if size.raw > WASM_WARN_RAW && !o.dev {
        report.push(format!(
            "WARNING: {} raw is above {} (compile time scales with raw size, ADR 0002)",
            bytes(size.raw),
            bytes(WASM_WARN_RAW)
        ));
        github_annotation("warning", &report[report.len() - 1]);
    }
    if size.gz > WASM_TARGET_GZ && !o.dev {
        report.push(format!(
            "note: {} gzipped is above the {} target",
            bytes(size.gz),
            bytes(WASM_TARGET_GZ)
        ));
    }
    if o.budget && !o.dev && size.gz > WASM_BUDGET_GZ {
        let msg = format!(
            "wasm over budget: {} gzipped > {}",
            bytes(size.gz),
            bytes(WASM_BUDGET_GZ)
        );
        github_annotation("error", &msg);
        failed = Some(msg);
    }
    for l in &report {
        eprintln!("{l}");
    }
    step_summary(&report);
    Ok(failed)
}

/// `1,234,567 B (1.23 MB)`.
fn bytes(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if n >= 1_000_000 {
        format!("{grouped} B ({:.2} MB)", n as f64 / 1e6)
    } else {
        format!("{grouped} B ({:.1} KB)", n as f64 / 1e3)
    }
}

/// A GitHub Actions annotation (`::warning::…`) when running there.
fn github_annotation(level: &str, msg: &str) {
    if env::var_os("GITHUB_ACTIONS").is_some() {
        println!("::{level}::{msg}");
    }
}

/// Appends the size report to the GitHub Actions job summary when running there.
fn step_summary(lines: &[String]) {
    let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut text = String::from("### Engine wasm size\n\n");
    for l in lines {
        text.push_str("- ");
        text.push_str(l);
        text.push('\n');
    }
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
}

/// The wasm-bindgen CLI must be the workspace's exact `wasm-bindgen` version: the CLI reads the
/// module's bindgen metadata, which changes between versions.
fn check_wasm_bindgen_cli() -> Result<(), String> {
    let want = wasm_bindgen_version()?;
    let hint = format!("cargo install wasm-bindgen-cli --version ={want} --locked");
    let out = Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .map_err(|_| format!("wasm-bindgen not found: {hint}"))?;
    let got = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if got.split_whitespace().nth(1) != Some(want.as_str()) {
        return Err(format!(
            "`{got}` does not match the workspace wasm-bindgen {want}: {hint}"
        ));
    }
    Ok(())
}

/// ADR 0002 decision 3: ammonia must not be in the wasm32 dependency tree (lessdugin-wasm also
/// fails to compile with it, see `ammonia_must_stay_out_of_wasm32`).
fn check_no_ammonia() -> Result<(), String> {
    let out = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "lessdugin-wasm",
            "--target",
            "wasm32-unknown-unknown",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(root())
        .output()
        .map_err(|e| format!("cargo tree: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let tree = String::from_utf8_lossy(&out.stdout);
    for banned in ["ammonia ", "html5ever "] {
        if tree.lines().any(|l| l.starts_with(banned)) {
            return Err(format!(
                "{}is in the wasm32 dependency tree of lessdugin-wasm (ADR 0002: native only)",
                banned
            ));
        }
    }
    Ok(())
}

/// The binaryen `wasm-opt`: on PATH, else the workspace's `binaryen` npm package.
fn wasm_opt() -> Option<String> {
    if have("wasm-opt") {
        return Some("wasm-opt".to_owned());
    }
    let local = root().join("node_modules/.bin/wasm-opt");
    local.exists().then(|| local.to_string_lossy().into_owned())
}

/// Builds lessdugin-wasm with `features` and `profile`, binds it into `out`
/// (`lessdugin_engine.js` + `lessdugin_engine_bg.wasm`) and, if `optimise`, runs wasm-opt.
fn build_wasm(
    features: &[&str],
    profile: &str,
    out: &Path,
    optimise: bool,
) -> Result<WasmSize, String> {
    let mut cargo = vec![
        "build",
        "-p",
        "lessdugin-wasm",
        "--lib",
        "--target",
        "wasm32-unknown-unknown",
        "--profile",
        profile,
    ];
    cargo.extend_from_slice(features);
    run("cargo", &cargo)?;
    let dir = if profile == "dev" { "debug" } else { profile };
    let wasm = root().join(format!(
        "target/wasm32-unknown-unknown/{dir}/lessdugin_wasm.wasm"
    ));
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
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
    let mut optimised = false;
    if optimise {
        match wasm_opt() {
            Some(o) => {
                // ADR 0002: exactly the target features rustc 1.97 writes, then -Oz; no
                // --converge
                let mut a = vec!["-Oz"];
                a.extend(WASM_OPT_FEATURES);
                a.extend([
                    "--strip-debug",
                    "--strip-producers",
                    "-o",
                    bg.to_str().unwrap(),
                    bg.to_str().unwrap(),
                ]);
                run(&o, &a)?;
                optimised = true;
            }
            None => eprintln!("(wasm-opt not found; skipping size optimisation)"),
        }
    }
    let raw = std::fs::metadata(&bg).map_err(|e| e.to_string())?.len();
    Ok(WasmSize {
        raw,
        gz: gz_size(&bg)?,
        glue_gz: gz_size(&out.join("lessdugin_engine.js"))?,
        optimised,
    })
}

/// The `target_features` section rustc 1.97 writes for wasm32-unknown-unknown (ADR 0002).
const WASM_OPT_FEATURES: &[&str] = &[
    "--enable-bulk-memory",
    "--enable-bulk-memory-opt",
    "--enable-call-indirect-overlong",
    "--enable-multivalue",
    "--enable-mutable-globals",
    "--enable-nontrapping-float-to-int",
    "--enable-reference-types",
    "--enable-sign-ext",
];

fn gz_size(path: &Path) -> Result<u64, String> {
    let out = Command::new("gzip")
        .args(["-9", "-c"])
        .arg(path)
        .output()
        .map_err(|e| format!("gzip: {e}"))?;
    if !out.status.success() {
        return Err(format!("gzip {} failed", path.display()));
    }
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

// ---------------------------------------------------------------------------------------------
// fuzz

/// The cargo-fuzz targets (`fuzz/fuzz_targets/<name>.rs`, see fuzz/README.md) with their libFuzzer
/// `-max_len` (bytes) and `-timeout` (seconds per input). `fuzz/dicts/<name>.dict` is used when
/// present. Every run also has `-rss_limit_mb=2048` (and so `-malloc_limit_mb=2048`).
const FUZZ_TARGETS: &[(&str, usize, u32)] = &[
    ("car", 16_384, 10),
    ("commit", 4_096, 10),
    ("dagcbor", 16_384, 10),
    ("lexicon", 16_384, 10),
    ("mst", 16_384, 10),
    ("render", 32_768, 10),
];

/// `cargo xtask fuzz`: seeds `fuzz/corpus/<target>/` from the vectors and fixtures (and the kept
/// reproducers in `fuzz/regressions/`), then runs each target in turn. Fails if any target stops
/// on a crash, timeout or OOM (the input is in `fuzz/artifacts/<target>/`).
fn fuzz(args: &[String]) -> Result<(), String> {
    let mut targets: Vec<&str> = Vec::new();
    let mut seconds = 600u64;
    let mut jobs: Option<u32> = None;
    let mut sanitizer = "address".to_owned();
    let mut extra: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => {
                seconds = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--seconds needs a number of seconds")?;
            }
            "--jobs" => {
                jobs = Some(
                    it.next()
                        .and_then(|s| s.parse().ok())
                        .ok_or("--jobs needs a number")?,
                );
            }
            "--sanitizer" => {
                sanitizer = it
                    .next()
                    .cloned()
                    .ok_or("--sanitizer needs address, none, …")?;
            }
            "--list" => {
                for (t, ..) in FUZZ_TARGETS {
                    println!("{t}");
                }
                return Ok(());
            }
            "--" => extra.extend(it.by_ref().cloned()),
            "all" => targets.extend(FUZZ_TARGETS.iter().map(|t| t.0)),
            t => match FUZZ_TARGETS.iter().find(|x| x.0 == t) {
                Some(x) => targets.push(x.0),
                None => {
                    let names: Vec<&str> = FUZZ_TARGETS.iter().map(|x| x.0).collect();
                    return Err(format!(
                        "unknown fuzz target or option {t:?} (targets: {})",
                        names.join(", ")
                    ));
                }
            },
        }
    }
    if targets.is_empty() {
        targets.extend(FUZZ_TARGETS.iter().map(|t| t.0));
    }
    let toolchain = format!(
        "+{}",
        env::var("LESSDUGIN_FUZZ_TOOLCHAIN").unwrap_or_else(|_| "nightly".into())
    );
    let has_fuzz = Command::new("cargo")
        .args([toolchain.as_str(), "fuzz", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_fuzz {
        return Err(format!(
            "cargo-fuzz on the {} toolchain is needed: rustup toolchain install {} --profile \
             minimal && cargo install cargo-fuzz --locked",
            &toolchain[1..],
            &toolchain[1..]
        ));
    }
    let root = root();
    let target = target_dir().join("fuzz");
    let target = target.to_string_lossy().into_owned();
    let corpus = root.join("fuzz/corpus").to_string_lossy().into_owned();
    let mut seed_args = vec![
        toolchain.as_str(),
        "run",
        "--quiet",
        "--manifest-path",
        "fuzz/Cargo.toml",
        "-p",
        "lessdugin-fuzz-seeds",
        "--target-dir",
        &target,
        "--",
        &corpus,
    ];
    seed_args.extend(targets.iter().copied());
    run("cargo", &seed_args)?;
    let mut failed = Vec::new();
    for t in &targets {
        let (_, max_len, timeout) = FUZZ_TARGETS
            .iter()
            .find(|x| x.0 == *t)
            .copied()
            .unwrap_or((t, 4096, 10));
        let mut a: Vec<String> = [
            toolchain.as_str(),
            "fuzz",
            "run",
            "--fuzz-dir",
            "fuzz",
            "--target-dir",
            &target,
            "--sanitizer",
            &sanitizer,
            t,
            "--",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        a.push(format!("-max_total_time={seconds}"));
        a.push(format!("-max_len={max_len}"));
        a.push(format!("-timeout={timeout}"));
        a.push("-rss_limit_mb=2048".into());
        a.push("-print_final_stats=1".into());
        let dict = root.join(format!("fuzz/dicts/{t}.dict"));
        if dict.exists() {
            a.push(format!("-dict={}", dict.display()));
        }
        if let Some(j) = jobs {
            a.push(format!("-fork={j}"));
        }
        a.extend(extra.iter().cloned());
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        if run("cargo", &a).is_err() {
            failed.push(*t);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "fuzz target(s) failed: {} (inputs in fuzz/artifacts/<target>/; reproduce with \
             `cargo +nightly fuzz run <target> <input>`, minimise with `cargo +nightly fuzz tmin`)",
            failed.join(", ")
        ))
    }
}
