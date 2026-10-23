// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embeds the SPA for `lessdugin web` (`threat-model.md` WEB-5, `docs/design/synthesis-v1.md`
//! §6.13).
//!
//! Every regular file under `web/dist` (or `$LESSDUGIN_WEB_DIST`) is embedded with
//! `include_bytes!` into a generated table, `$OUT_DIR/web_assets.rs`:
//!
//! ```text
//! pub static FILES: &[(&str, &[u8])] = &[("index.html", include_bytes!("…")), …];
//! pub const PLACEHOLDER: bool = false;
//! ```
//!
//! When the directory is missing or has no `index.html`, the table is empty, `PLACEHOLDER` is
//! true, and `lessdugin web` serves a placeholder page that explains how to build the SPA. A
//! release build refuses to embed the placeholder (release binaries MUST embed the real SPA)
//! unless `LESSDUGIN_WEB_PLACEHOLDER=1` says that is intended. Symbolic links are not followed,
//! and file names must be UTF-8 without `\` (they become URL paths).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Overrides the directory that is embedded.
const DIST_ENV: &str = "LESSDUGIN_WEB_DIST";
/// Allows a release build without the SPA.
const PLACEHOLDER_ENV: &str = "LESSDUGIN_WEB_PLACEHOLDER";

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &path, out)?;
        } else if kind.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("a file under the root")
                .components()
                .map(|c| c.as_os_str().to_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>();
            match rel {
                Some(parts) if parts.iter().all(|p| !p.contains('\\')) => {
                    out.push((parts.join("/"), path));
                }
                _ => println!(
                    "cargo:warning=skipping {} (not a UTF-8 file name)",
                    path.display()
                ),
            }
        }
        // Symbolic links and other file types are never embedded.
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={DIST_ENV}");
    println!("cargo:rerun-if-env-changed={PLACEHOLDER_ENV}");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let dist = std::env::var_os(DIST_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../web/dist"));
    // A directory is scanned recursively for changes; a missing path makes the script rerun.
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut files = Vec::new();
    // Without the `web` feature there is no `lessdugin web`, so nothing is embedded.
    let web = std::env::var_os("CARGO_FEATURE_WEB").is_some();
    let real = dist.join("index.html").is_file();
    if web && real {
        let root = dist.canonicalize().expect("web/dist is readable");
        collect(&root, &root, &mut files).expect("web/dist is readable");
        files.sort();
    } else if web {
        let release = std::env::var("PROFILE").is_ok_and(|p| p == "release");
        let allowed = std::env::var(PLACEHOLDER_ENV).is_ok_and(|v| v == "1");
        if release && !allowed {
            panic!(
                "{} has no index.html: release builds of lessdugin must embed the SPA \
                 (threat-model.md WEB-5). Build it first (pnpm install && pnpm --filter \
                 @lessdugin/web build), or set {PLACEHOLDER_ENV}=1 to embed the placeholder page.",
                dist.display()
            );
        }
        println!(
            "cargo:warning={} has no index.html: `lessdugin web` will serve a placeholder page \
             (build the SPA with `pnpm --filter @lessdugin/web build`, then rebuild)",
            dist.display()
        );
    }

    let mut code = String::from(
        "/// The embedded SPA: (path relative to `web/dist`, contents), sorted by path.\n\
         pub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for (rel, path) in &files {
        let abs = path.to_str().expect("UTF-8 path");
        writeln!(code, "    ({rel:?}, include_bytes!({abs:?})),").expect("write to a String");
    }
    code.push_str("];\n");
    writeln!(
        code,
        "/// True when `web/dist` was missing: the placeholder page is served.\n\
         pub const PLACEHOLDER: bool = {};",
        web && !real
    )
    .expect("write to a String");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("web_assets.rs"), code).expect("OUT_DIR is writable");
}
