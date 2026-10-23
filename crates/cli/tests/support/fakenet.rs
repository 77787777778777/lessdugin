// SPDX-License-Identifier: AGPL-3.0-or-later
//! A FakePds network served over real HTTP (testkit feature `http`) and `lessdugin --dev`
//! users against it: each user has their own data directory, logs in with an app password and
//! runs the binary with `assert_cmd`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use lessdugin_testkit::fake_pds::{AccountHandle, FakeNet, SystemClock};
use serde_json::Value;

/// The network.
pub struct Net {
    pub net: FakeNet,
}

/// One user: an account on the network and a data directory.
pub struct User {
    pub handle: String,
    pub account: AccountHandle,
    pub dir: tempfile::TempDir,
    plc: String,
    pds: String,
    /// The terminal session (`LESSDUGIN_SESSION`).
    pub session: String,
}

impl Net {
    /// A network with one PDS.
    pub fn new() -> Self {
        Self {
            net: FakeNet::serve_http(SystemClock),
        }
    }

    /// A new account and its CLI user (logged in when `login`).
    pub fn user(&self, handle: &str, login: bool) -> User {
        let account = self.net.add_account(handle);
        let u = User {
            handle: handle.to_owned(),
            pds: account.pds_base(),
            plc: self.net.plc(),
            account,
            dir: tempfile::tempdir().unwrap(),
            session: "t1".into(),
        };
        if login {
            let out = u
                .cmd()
                .args(["login", handle, "--password-stdin"])
                .write_stdin(format!("{}\n", u.account.app_password()))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "login {handle}: {}",
                text(&out.stderr)
            );
        }
        u
    }

    /// Another device of `u`'s account: its own data directory, logged in.
    pub fn second_device(&self, u: &User) -> User {
        let d = User {
            handle: u.handle.clone(),
            account: u.account.clone(),
            dir: tempfile::tempdir().unwrap(),
            plc: u.plc.clone(),
            pds: u.pds.clone(),
            session: "t1".into(),
        };
        let out = d
            .cmd()
            .args(["login", &u.handle, "--password-stdin"])
            .write_stdin(format!("{}\n", u.account.app_password()))
            .output()
            .unwrap();
        assert!(out.status.success(), "login: {}", text(&out.stderr));
        d
    }

    /// An anonymous reader (no account) on the same network.
    pub fn reader(&self, like: &User) -> User {
        User {
            handle: "anonymous".into(),
            account: like.account.clone(),
            dir: tempfile::tempdir().unwrap(),
            plc: like.plc.clone(),
            pds: like.pds.clone(),
            session: "t1".into(),
        }
    }
}

/// Lossy UTF-8.
pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

impl User {
    /// `lessdugin --dev` for this user, with a clean environment.
    pub fn std_cmd(&self) -> std::process::Command {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_lessdugin"));
        for (k, _) in std::env::vars() {
            if k.starts_with("LESSDUGIN_") {
                c.env_remove(k);
            }
        }
        c.arg("--dev")
            .env("LESSDUGIN_DATA_DIR", self.dir.path())
            .env("LESSDUGIN_KEYRING", "off")
            .env("LESSDUGIN_PLC_URL", &self.plc)
            .env("LESSDUGIN_PDS_URL", &self.pds)
            .env("LESSDUGIN_SESSION", &self.session)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .env_remove("BROWSER");
        c
    }

    /// [`User::std_cmd`] for `assert_cmd`.
    pub fn cmd(&self) -> Command {
        Command::from_std(self.std_cmd())
    }

    /// Runs a command (stdin empty).
    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).write_stdin("").output().unwrap()
    }

    /// Runs a command that must succeed; returns stdout.
    pub fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{} {args:?} failed ({:?}):\n{}\n{}",
            self.handle,
            out.status.code(),
            text(&out.stdout),
            text(&out.stderr)
        );
        text(&out.stdout)
    }

    /// Runs a command with `--json` that must succeed; returns the parsed stdout.
    pub fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let s = self.ok(&a);
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{args:?}: {e}: {s}"))
    }

    /// Runs a command that must fail with `code`; returns the JSON error on stderr (`--json`).
    pub fn fails(&self, code: i32, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let out = self.run(&a);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{args:?}:\n{}\n{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let err = text(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| l.starts_with('{'))
            .unwrap_or_else(|| panic!("no JSON error on stderr: {err}"));
        serde_json::from_str(line).unwrap()
    }

    /// The DID.
    pub fn did(&self) -> String {
        self.account.did().to_string()
    }

    /// The data directory the CLI uses in devnet mode (`<dir>/devnet-<id>`).
    pub fn devnet_dir(&self) -> PathBuf {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("devnet-"))
            })
            .collect();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        dirs.pop().unwrap()
    }

    /// Writes a file in the user's directory.
    pub fn file(&self, name: &str, content: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    /// A path in the user's directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// `p` as a `&str`.
pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}
