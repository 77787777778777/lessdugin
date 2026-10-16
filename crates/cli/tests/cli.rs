// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline tests of the `lessdugin` binary: help, usage errors and exit codes, refusal of
//! passwords in argv and of main passwords, `--json` shapes (`"dto": 1`), terminal sanitising of
//! error messages, parse errors and successful output, and devnet mode (explicit only, warned
//! about, kept in its own directories). Network commands run against a "devnet" on a closed
//! port or a tiny local server, so nothing leaves the machine.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

/// A closed local port standing in for an unreachable devnet.
const DEAD: [&str; 5] = [
    "--dev",
    "--plc-url",
    "http://127.0.0.1:9",
    "--pds-url",
    "http://127.0.0.1:9",
];

fn bin(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
    for (k, _) in std::env::vars() {
        if k.starts_with("LESSDUGIN_") {
            c.env_remove(k);
        }
    }
    c.env("LESSDUGIN_DATA_DIR", dir)
        .env("LESSDUGIN_KEYRING", "off")
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env_remove("BROWSER");
    c
}

fn stderr_json(out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON error on stderr: {text}"));
    serde_json::from_str(line).unwrap()
}

#[test]
fn help_lists_the_commands_and_no_password_option() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    for c in [
        "login",
        "logout",
        "whoami",
        "accounts",
        "resolve",
        "verify",
        "doctor",
        "completions",
        "--json",
        "--dev",
        "Exit codes",
    ] {
        assert!(help.contains(c), "missing {c} in\n{help}");
    }
    let out = bin(dir.path()).args(["login", "--help"]).output().unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--app-password") && help.contains("--password-stdin"));
    assert!(help.contains("--no-browser") && help.contains("--oauth-client"));
    assert!(!help.contains("--password "), "{help}");
    // Bare invocation prints help.
    let out = bin(dir.path()).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage"));
    // Version.
    let out = bin(dir.path()).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("lessdugin "));
}

#[test]
fn usage_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    bin(dir.path()).arg("frobnicate").assert().code(2);
    bin(dir.path()).arg("resolve").assert().code(2);
    bin(dir.path())
        .args(["--plc-url", "http://127.0.0.1:9", "resolve", "x.test"])
        .assert()
        .code(2);
    bin(dir.path())
        .args(["--dev", "resolve", "x.test"])
        .assert()
        .code(2);
    bin(dir.path())
        .args(["login", "--app-password", "--no-browser", "alice.test"])
        .assert()
        .code(2);
}

#[test]
fn passwords_in_argv_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["login", "alice.test", "--password", "abcd-efgh-ijkl-mnop"],
        vec!["login", "alice.test", "-p", "hunter2"],
        vec!["login", "alice.test", "--password=hunter2"],
        vec!["login", "--app-password", "abcd-efgh-ijkl-mnop"],
        vec!["login", "abcd-efgh-ijkl-mnop"],
    ] {
        let out = bin(dir.path()).args(DEAD).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("never accepted"), "{args:?}: {err}");
        assert!(!err.contains("hunter2"), "{err}");
    }
    // Anything after the handle is refused the same way, and never echoed.
    for extra in ["abcd-efgh-ijkl-mnop", "hunter2"] {
        let out = bin(dir.path())
            .args(["login", "alice.test", extra])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("never accepted"), "{err}");
        assert!(!err.contains(extra), "{err}");
    }
}

#[test]
fn parse_errors_never_echo_passwords_or_controls() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path())
        .args(["whoami", "abcd-efgh-ijkl-mnop"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("abcd-efgh-ijkl-mnop"), "{err}");
    assert!(
        err.contains("<redacted>") && err.contains("revoke"),
        "{err}"
    );
    let out = bin(dir.path())
        .arg("frob\u{1b}]0;pwned\u{7}\u{202e}x")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8(out.stderr).unwrap();
    for bad in ['\u{1b}', '\u{7}', '\u{202e}'] {
        assert!(!err.contains(bad), "{bad:?} in {err:?}");
    }
    assert!(err.contains("unrecognized subcommand 'frob"), "{err:?}");
}

#[test]
fn main_passwords_are_refused_before_any_network() {
    let dir = tempfile::tempdir().unwrap();
    // The devnet is unreachable: a network attempt would give `network`, not `invalid_input`.
    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "login", "alice.test", "--app-password"])
        .env("LESSDUGIN_APP_PASSWORD", "correct horse battery staple")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let e = stderr_json(&out);
    assert_eq!(e["code"], "invalid_input");
    assert_eq!(e["detail"]["kind"], "notAnAppPassword");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("horse"));
    assert!(out.stdout.is_empty());

    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "login", "alice.test", "--password-stdin"])
        .write_stdin("hunter2\n")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr_json(&out)["code"], "invalid_input");

    // No terminal and no password source: a usage error.
    let out = bin(dir.path())
        .args(DEAD)
        .args(["login", "alice.test", "--app-password"])
        .write_stdin("")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--password-stdin"));
}

#[test]
fn app_password_reaches_the_network_and_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path())
        .args(DEAD)
        .args([
            "--json",
            "login",
            "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
            "--password-stdin",
        ])
        .write_stdin("abcd-efgh-ijkl-mnop\n")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let e = stderr_json(&out);
    assert_eq!(e["code"], "network");
    assert_eq!(e["retryable"], true);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("abcd-efgh"));
}

#[test]
fn resolve_errors_have_codes() {
    let dir = tempfile::tempdir().unwrap();
    // A DID whose PLC directory is down: network (exit 1).
    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "resolve", "did:plc:ewvi7nxzyoun6zhxrhs64oiz"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let e = stderr_json(&out);
    assert_eq!(e["code"], "network");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("did:plc:ewvi7nxzyoun6zhxrhs64oiz")
    );

    // A handle whose resolver is down: no answer (not 0, a structured error).
    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "resolve", "alice.test"])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(stderr_json(&out)["code"].is_string());

    // Not an identifier: invalid_ref.
    let out = bin(dir.path())
        .args(["--json", "resolve", "not a handle!"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr_json(&out)["code"], "invalid_ref");

    // `.test` outside devnet mode.
    let out = bin(dir.path())
        .args(["resolve", "alice.test"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("devnet"));
}

#[test]
fn error_messages_are_sanitised() {
    let dir = tempfile::tempdir().unwrap();
    let evil = "\u{1b}[31mevil\u{1b}]0;title\u{7}\u{202e}x\u{9b}";
    let out = bin(dir.path()).args(["resolve", evil]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    for bad in ['\u{1b}', '\u{7}', '\u{202e}', '\u{9b}'] {
        assert!(!err.contains(bad), "{bad:?} in {err:?}");
    }
    assert!(err.contains("^[[31mevil"), "{err:?}");
    // --color always: only our SGR sequences.
    let out = bin(dir.path())
        .args(["--color", "always", "resolve", evil])
        .output()
        .unwrap();
    let err = String::from_utf8(out.stderr).unwrap();
    for (i, _) in err.match_indices('\u{1b}') {
        let rest = &err[i..];
        assert!(
            [
                "\x1b[0m", "\x1b[1m", "\x1b[2m", "\x1b[7m", "\x1b[31m", "\x1b[32m", "\x1b[33m"
            ]
            .iter()
            .any(|p| rest.starts_with(p)),
            "{err:?}"
        );
    }
}

#[test]
fn account_commands_without_login() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path()).args(["--json", "whoami"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_json(&out)["code"], "auth_required");
    bin(dir.path()).arg("logout").assert().code(3);
    bin(dir.path())
        .args(["--account", "alice.test", "whoami"])
        .assert()
        .code(3);
    bin(dir.path())
        .args(["accounts", "use", "alice.test"])
        .assert()
        .code(3);
    let out = bin(dir.path())
        .args(["--json", "accounts"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, serde_json::json!({"dto": 1, "accounts": []}));
}

#[test]
fn verify_rejects_bad_refs() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path())
        .args(["--json", "verify", "not-an-at-uri"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr_json(&out)["code"], "invalid_ref");
    let out = bin(dir.path())
        .args(["--json", "verify", "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz"])
        .output()
        .unwrap();
    assert_eq!(stderr_json(&out)["code"], "invalid_ref");
}

#[test]
fn doctor_reports_unreachable_devnet() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dto"], 1);
    let checks = v["checks"].as_array().unwrap();
    let by_name = |n: &str| {
        checks
            .iter()
            .find(|c| c["name"].as_str().is_some_and(|x| x.starts_with(n)))
            .unwrap_or_else(|| panic!("no check {n}: {v}"))
    };
    assert_eq!(by_name("config")["status"], "ok");
    assert_eq!(by_name("paths")["status"], "ok");
    assert_eq!(by_name("keyring")["status"], "ok");
    assert_eq!(by_name("plc")["status"], "fail");
    assert_eq!(by_name("pds")["status"], "fail");
    assert_eq!(by_name("range cursor")["status"], "skip");
    // Devnet mode contacts no public infrastructure.
    assert!(
        !checks
            .iter()
            .any(|c| c["name"].as_str().unwrap().starts_with("doh"))
    );
    // Text output.
    let out = bin(dir.path()).args(DEAD).arg("doctor").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("FAIL  plc 127.0.0.1:9"), "{text}");
}

#[test]
fn completions_are_generated() {
    let dir = tempfile::tempdir().unwrap();
    for shell in ["bash", "zsh", "fish"] {
        let out = bin(dir.path())
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("lessdugin"));
    }
}

#[test]
fn devnet_mode_is_explicit_and_announced() {
    let dir = tempfile::tempdir().unwrap();
    // LESSDUGIN_DEV is not read: the devnet URLs alone are a usage error.
    let out = bin(dir.path())
        .env("LESSDUGIN_DEV", "1")
        .env("LESSDUGIN_PLC_URL", "http://127.0.0.1:9")
        .env("LESSDUGIN_PDS_URL", "http://127.0.0.1:9")
        .args(["resolve", "alice.test"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("need --dev"));
    // Every devnet run says so on stderr (not with -q), and keeps stdout clean.
    let out = bin(dir.path())
        .args(DEAD)
        .args(["--json", "resolve", "alice.test"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("warning: devnet mode"), "{err}");
    assert!(out.stdout.is_empty());
    let out = bin(dir.path())
        .args(DEAD)
        .args(["-q", "resolve", "alice.test"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("devnet mode"));
    // A devnet profile in config.toml is announced too.
    let cfg = tempfile::tempdir().unwrap();
    std::fs::write(
        cfg.path().join("config.toml"),
        "[network]\nmode = \"devnet\"\nplc = \"http://127.0.0.1:9\"\npds = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let out = bin(cfg.path())
        .args(["resolve", "alice.test"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: devnet mode"));
}

#[test]
fn devnet_data_lives_apart_from_production() {
    let dir = tempfile::tempdir().unwrap();
    bin(dir.path())
        .args(DEAD)
        .args(["resolve", "did:plc:ewvi7nxzyoun6zhxrhs64oiz"])
        .output()
        .unwrap();
    assert!(
        !dir.path().join("state.sqlite").exists(),
        "the production state store is untouched"
    );
    assert!(!dir.path().join("cache").join("cache.sqlite").exists());
    let devnet: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("devnet-"))
        .collect();
    assert_eq!(devnet.len(), 1, "{devnet:?}");
    assert!(dir.path().join(&devnet[0]).join("state.sqlite").exists());
    assert!(
        dir.path()
            .join("cache")
            .join(&devnet[0])
            .join("cache.sqlite")
            .exists()
    );
    // Another devnet (another PDS) gets another directory.
    bin(dir.path())
        .args([
            "--dev",
            "--plc-url",
            "http://127.0.0.1:9",
            "--pds-url",
            "http://127.0.0.1:10",
            "resolve",
            "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
        ])
        .output()
        .unwrap();
    let n = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("devnet-")
        })
        .count();
    assert_eq!(n, 2);
}

/// A one-thread HTTP server that answers every GET with `body` (a devnet PLC and PDS).
fn serve_forever(body: String) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let (status, body) = if request_line.contains("/did:plc:") {
                ("200 OK", body.as_str())
            } else {
                ("400 Bad Request", r#"{"error":"HandleNotFound"}"#)
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[test]
fn successful_output_is_sanitised() {
    let did = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    let doc = serde_json::json!({
        "id": did,
        "alsoKnownAs": ["at://evil\u{1b}]0;pwned\u{7}\u{202e}x.test"],
        "verificationMethod": [{"id": "#atproto", "type": "Multikey\u{1b}[31m",
                                "publicKeyMultibase": "z\u{1b}[2J"}],
        "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer",
                     "serviceEndpoint": "http://127.0.0.1:1"}],
    });
    let port = serve_forever(doc.to_string());
    let url = format!("http://127.0.0.1:{port}");
    let dir = tempfile::tempdir().unwrap();
    let out = bin(dir.path())
        .args([
            "--dev",
            "--plc-url",
            &url,
            "--pds-url",
            &url,
            "resolve",
            did,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    for bad in ['\u{1b}', '\u{7}', '\u{202e}'] {
        assert!(!text.contains(bad), "{bad:?} in {text:?}");
    }
    assert!(text.contains("evil^[]0;pwned"), "{text}");
    assert!(text.contains("signing key"), "{text}");
    // --color always adds only our SGR sequences.
    let out = bin(dir.path())
        .args([
            "--dev",
            "--plc-url",
            &url,
            "--pds-url",
            &url,
            "--color",
            "always",
            "resolve",
            did,
        ])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for (i, _) in text.match_indices('\u{1b}') {
        let rest = &text[i..];
        assert!(
            ["\x1b[0m", "\x1b[1m", "\x1b[2m", "\x1b[7m"]
                .iter()
                .any(|p| rest.starts_with(p)),
            "{text:?}"
        );
    }
    // --json carries "dto": 1; piped output is the exact data.
    let out = bin(dir.path())
        .args([
            "--dev",
            "--plc-url",
            &url,
            "--pds-url",
            &url,
            "--json",
            "resolve",
            did,
        ])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dto"], 1);
    assert_eq!(v["identity"]["did"], did);
    assert_eq!(v["identity"]["handleState"], "invalid");
}
