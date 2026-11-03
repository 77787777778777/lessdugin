// SPDX-License-Identifier: AGPL-3.0-or-later
//! `cargo xtask site --origin <url> [--ns temp|final] [--out <dir>] [--no-build]`: the hosted
//! SPA for one origin (threat-model.md SITE-1, SITE-2, SITE-4; data-model.md §10.4
//! "Self-hosting").
//!
//! It writes two trees under `--out` (default `target/site`):
//!
//! - `headers/`, for Cloudflare Pages or Netlify (the default hosts, SITE-1): the release build
//!   of the SPA (`vite build`, checked by `web/scripts/check-release-bundle.mjs`), the
//!   `_headers` file of `deploy/site` (threat-model.md §6.1 and §6.3: the production CSP and the
//!   other headers), and both client metadata documents of data-model.md §10.4 under `oauth/`;
//! - `pages/`, the GitHub Pages fallback (threat-model.md §6.3): the `vite build --mode pages`
//!   build, whose `index.html` carries the policy as the first element of `<head>` (the §6.1
//!   text minus `frame-ancestors`; the SPA's frame guard, SPA-10, makes up for that), the same
//!   `oauth/` documents and `.nojekyll`.
//!
//! The documents name the origin in `client_id`, `client_uri` and `redirect_uris`; the scope is
//! §10.4's SUPERSET in the namespace `--ns` names. A `.temp` build (the default, and the only
//! SPA this repository builds before the release, docs/plan.md M7) asks for the enumerated
//! `.temp` collections and no `include:` token, since `.temp` publishes no permission set. A
//! build writes only the documents of the namespace the SPA requests scopes in
//! (`packages/engine/src/auth-bridge.ts`, `NS`); the other namespace's are written only with
//! `--no-build` (the deployed `deploy/site` files are the final ones, and a final-namespace SPA
//! does not exist before M7).

use std::path::{Path, PathBuf};

use crate::{root, run};

/// The record collections of the scope, in data-model.md §10.3/§10.4 order.
const COLLECTIONS: [&str; 10] = [
    "forum",
    "membership",
    "post",
    "comment",
    "revision",
    "opinion",
    "modAction",
    "wikiRevision",
    "sequence",
    "profile",
];
const BLOBS: &str = "blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf";
const FINAL_NS: &str = "online.7778777.lessdugin";

/// Which namespace the documents' scope names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ns {
    /// `online.7778777.lessdugin.temp` (development builds, data-model.md §1.3).
    Temp,
    /// `online.7778777.lessdugin` (the release).
    Final,
}

impl Ns {
    fn prefix(self) -> String {
        match self {
            Ns::Temp => format!("{FINAL_NS}.temp"),
            Ns::Final => FINAL_NS.to_owned(),
        }
    }
}

/// The namespace the SPA in `source` (packages/engine/src/auth-bridge.ts) asks scopes for: its
/// `export const NS = "…";`, which web/src/host/scopes.ts imports.
fn spa_namespace(source: &str) -> Result<Ns, String> {
    let ns = source
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("export const NS = \"")?
                .strip_suffix("\";")
        })
        .ok_or("packages/engine/src/auth-bridge.ts declares no `export const NS`")?;
    match ns {
        n if n == Ns::Temp.prefix() => Ok(Ns::Temp),
        n if n == Ns::Final.prefix() => Ok(Ns::Final),
        n => Err(format!(
            "the SPA's namespace {n} is neither {FINAL_NS} nor its .temp"
        )),
    }
}

/// data-model.md §10.4 `SUPERSET` in `ns` (a `.temp` scope has no `include:` token).
pub fn superset(ns: Ns) -> String {
    let p = ns.prefix();
    let include = match ns {
        Ns::Final => format!(" include:{p}.authFull"),
        Ns::Temp => String::new(),
    };
    let repo = COLLECTIONS
        .iter()
        .map(|c| format!("collection={p}.{c}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("atproto{include} repo?{repo} {BLOBS}")
}

/// The site's base URL for `origin`: `https://host[:port]/[path/]`, with a trailing slash.
/// Hosted client metadata needs `https`, and the base must not carry credentials, a query or a
/// fragment (they would end up in the client id).
pub fn base_url(origin: &str) -> Result<String, String> {
    let rest = origin
        .strip_prefix("https://")
        .ok_or_else(|| format!("--origin must be an https URL: {origin}"))?;
    if rest.contains(['?', '#', '@', '\\', ' ', '"']) {
        return Err(format!(
            "--origin must have no credentials, query or fragment: {origin}"
        ));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.split(':').next().unwrap_or_default();
    let valid_host = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !host.starts_with('.')
        && !host.ends_with('.');
    let valid_port = authority
        .split_once(':')
        .is_none_or(|(_, p)| !p.is_empty() && p.parse::<u16>().is_ok());
    if !valid_host || !valid_port || authority.matches(':').count() > 1 {
        return Err(format!("--origin has no valid host: {origin}"));
    }
    let path = path.trim_end_matches('/');
    Ok(if path.is_empty() {
        format!("https://{}/", authority.to_ascii_lowercase())
    } else {
        format!("https://{}/{path}/", authority.to_ascii_lowercase())
    })
}

/// A JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One client metadata document, laid out as in data-model.md §10.4.
fn document(
    client_id: &str,
    name: &str,
    client_uri: &str,
    kind: &str,
    redirect: &str,
    scope: &str,
) -> String {
    format!(
        "{{\n  \"client_id\": {},\n  \"client_name\": {},\n  \"client_uri\": {},\n  \
         \"application_type\": {},\n  \"grant_types\": [\"authorization_code\", \
         \"refresh_token\"],\n  \"response_types\": [\"code\"],\n  \"redirect_uris\": [{}],\n  \
         \"scope\": {},\n  \"token_endpoint_auth_method\": \"none\",\n  \
         \"dpop_bound_access_tokens\": true\n}}\n",
        json_str(client_id),
        json_str(name),
        json_str(client_uri),
        json_str(kind),
        json_str(redirect),
        json_str(scope),
    )
}

/// The web client and the hosted native client documents of data-model.md §10.4 for `base`
/// (a [`base_url`]): `(oauth/client-metadata.json, oauth/cli-client-metadata.json)`.
pub fn metadata(base: &str, ns: Ns) -> (String, String) {
    let scope = superset(ns);
    let web = document(
        &format!("{base}oauth/client-metadata.json"),
        "lessdugin",
        base,
        "web",
        base,
        &scope,
    );
    let cli = document(
        &format!("{base}oauth/cli-client-metadata.json"),
        "lessdugin CLI",
        base,
        "native",
        "http://127.0.0.1/callback",
        &scope,
    );
    (web, cli)
}

/// threat-model.md §6.1's policy minus `frame-ancestors`, as the GitHub Pages `<meta>` carries
/// it (`;`-separated directives of `policy`, in order).
fn meta_policy(policy: &str) -> String {
    policy
        .split("; ")
        .filter(|d| !d.starts_with("frame-ancestors "))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The `Content-Security-Policy` of the `/*` block of a `_headers` file.
fn headers_csp(headers: &str) -> Option<&str> {
    let mut in_all = false;
    for line in headers.lines() {
        if !line.starts_with(' ') && !line.starts_with('#') && !line.is_empty() {
            in_all = line.trim() == "/*";
            continue;
        }
        if in_all && let Some(v) = line.trim().strip_prefix("Content-Security-Policy: ") {
            return Some(v);
        }
    }
    None
}

/// Checks a Pages build's `index.html`: its `<head>` starts with the `<meta>` policy, which is
/// `policy` (the header text) minus `frame-ancestors`.
fn check_pages_index(html: &str, policy: &str) -> Result<(), String> {
    let head = html
        .find("<head>")
        .ok_or("the Pages index.html has no <head>")?;
    let first = html[head + "<head>".len()..].trim_start();
    let expected = format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\" />",
        meta_policy(policy)
            .replace('&', "&amp;")
            .replace('"', "&quot;")
    );
    if first.starts_with(&expected) {
        Ok(())
    } else {
        Err(format!(
            "the Pages index.html does not start <head> with the <meta> policy {expected}"
        ))
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn usage() -> String {
    "usage: cargo xtask site --origin <https URL> [--ns temp|final] [--out <dir>] [--no-build]"
        .into()
}

/// `cargo xtask site` (see the module documentation).
pub fn site(args: &[String]) -> Result<(), String> {
    let (mut origin, mut ns, mut out, mut build) = (None, Ns::Temp, None, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--origin" => origin = it.next().cloned(),
            "--ns" => {
                ns = match it.next().map(String::as_str) {
                    Some("temp") => Ns::Temp,
                    Some("final") => Ns::Final,
                    _ => return Err(usage()),
                }
            }
            "--out" => out = it.next().map(PathBuf::from),
            "--no-build" => build = false,
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            _ => return Err(format!("{} (unknown argument {a})", usage())),
        }
    }
    let base = base_url(&origin.ok_or_else(usage)?)?;
    // the documents' scope must be the one the built SPA requests (until the release, M7, it
    // is the .temp one), so a build refuses the other namespace; --no-build writes either
    let bridge = std::fs::read_to_string(root().join("packages/engine/src/auth-bridge.ts"))
        .map_err(|e| format!("packages/engine/src/auth-bridge.ts: {e}"))?;
    if build && spa_namespace(&bridge)? != ns {
        return Err(format!(
            "the SPA in this tree requests the {} scopes: --ns {} writes the other \
             namespace's documents only with --no-build",
            spa_namespace(&bridge)?.prefix(),
            match ns {
                Ns::Temp => "temp",
                Ns::Final => "final",
            }
        ));
    }
    let out = out.unwrap_or_else(|| crate::target_dir().join("site"));
    let out = if out.is_absolute() {
        out
    } else {
        root().join(out)
    };
    let (headers_dir, pages_dir) = (out.join("headers"), out.join("pages"));
    let headers = std::fs::read_to_string(root().join("deploy/site/_headers"))
        .map_err(|e| format!("deploy/site/_headers: {e}"))?;
    let policy = headers_csp(&headers)
        .ok_or("deploy/site/_headers has no Content-Security-Policy for /*")?
        .to_owned();
    if build {
        crate::wasm(&["--release".to_owned()])?;
        for (dir, mode) in [(&headers_dir, None), (&pages_dir, Some("pages"))] {
            let dir_s = dir.display().to_string();
            let mut a = vec!["--filter", "@lessdugin/web", "exec", "vite", "build"];
            if let Some(m) = mode {
                a.extend(["--mode", m]);
            }
            a.extend(["--outDir", &dir_s, "--emptyOutDir"]);
            run("pnpm", &a)?;
            run(
                "pnpm",
                &[
                    "--filter",
                    "@lessdugin/web",
                    "exec",
                    "node",
                    "scripts/check-release-bundle.mjs",
                    &dir_s,
                ],
            )?;
        }
        let index = std::fs::read_to_string(pages_dir.join("index.html"))
            .map_err(|e| format!("the Pages build: {e}"))?;
        check_pages_index(&index, &policy)?;
    }
    let (web, cli) = metadata(&base, ns);
    for dir in [&headers_dir, &pages_dir] {
        write(&dir.join("oauth/client-metadata.json"), &web)?;
        write(&dir.join("oauth/cli-client-metadata.json"), &cli)?;
    }
    write(&headers_dir.join("_headers"), &headers)?;
    write(&pages_dir.join(".nojekyll"), "")?;
    eprintln!(
        "site for {base} ({}): {} (Cloudflare Pages or Netlify) and {} (GitHub Pages fallback){}",
        match ns {
            Ns::Temp => "NS.temp",
            Ns::Final => "final NS",
        },
        headers_dir.display(),
        pages_dir.display(),
        if build {
            ""
        } else {
            "; site files only (--no-build)"
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(rel: &str) -> String {
        std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    /// The first fenced code block after `marker` in `text`, with its final newline.
    fn block_after(text: &str, marker: &str) -> String {
        let at = text.find(marker).unwrap_or_else(|| panic!("no {marker:?}"));
        let rest = &text[at..];
        let open = rest.find("```").expect("a fenced block");
        let body = &rest[open..];
        let start = body.find('\n').expect("a fence line") + 1;
        let end = body[start..].find("```").expect("a closing fence");
        body[start..start + end].to_owned()
    }

    fn section_10_4() -> String {
        let text = spec("docs/spec/data-model.md");
        let at = text.find("### 10.4 Client metadata").expect("§10.4");
        let end = text[at..].find("### 10.5").expect("§10.5");
        text[at..at + end].to_owned()
    }

    /// The final documents for the canonical origin are §10.4's, byte for byte, and so is its
    /// SUPERSET.
    #[test]
    fn the_canonical_documents_are_data_model_10_4() {
        let s = section_10_4();
        let base = base_url("https://lessdugin.7778777.online").unwrap();
        assert_eq!(base, "https://lessdugin.7778777.online/");
        let (web, cli) = metadata(&base, Ns::Final);
        assert_eq!(web, block_after(&s, "**Web client**"));
        assert_eq!(cli, block_after(&s, "**Native client, hosted metadata**"));
        assert_eq!(
            format!("{}\n", superset(Ns::Final)),
            block_after(&s, "`SUPERSET` is the scope string")
        );
    }

    /// A self-hosted `.temp` site: §10.4 with the origin substituted in `client_id`,
    /// `client_uri` and `redirect_uris`, the `.temp` collections (§1.3) and no `include:`.
    #[test]
    fn a_temp_site_for_another_origin_substitutes_the_origin_and_drops_include() {
        let s = section_10_4();
        let base = base_url("https://lessdugin-dev.7778777.online/").unwrap();
        let (web, cli) = metadata(&base, Ns::Temp);
        for (got, marker) in [
            (&web, "**Web client**"),
            (&cli, "**Native client, hosted metadata**"),
        ] {
            let expected = block_after(&s, marker)
                .replace(
                    "https://lessdugin.7778777.online/",
                    "https://lessdugin-dev.7778777.online/",
                )
                .replace(" include:online.7778777.lessdugin.authFull", "")
                .replace(
                    "online.7778777.lessdugin.",
                    "online.7778777.lessdugin.temp.",
                );
            assert_eq!(got, &expected, "{marker}");
            assert!(!got.contains("include:"));
            assert!(!got.contains("transition:generic") && !got.contains("blob:*/*"));
        }
        // the native document keeps the loopback redirect (its port is ignored when matching)
        assert!(cli.contains("\"redirect_uris\": [\"http://127.0.0.1/callback\"]"));
    }

    /// The SPA derives its client id from its own directory (`web/src/host/hosted.ts`), so a
    /// site under a path keeps that path; `client_uri` has the host of `client_id`.
    #[test]
    fn the_base_url_keeps_a_path_and_refuses_what_a_client_id_cannot_carry() {
        assert_eq!(
            base_url("https://Example.org:8443/forum").unwrap(),
            "https://example.org:8443/forum/"
        );
        let (web, _) = metadata(&base_url("https://example.org/a/b/").unwrap(), Ns::Temp);
        assert!(
            web.contains("\"client_id\": \"https://example.org/a/b/oauth/client-metadata.json\"")
        );
        assert!(web.contains("\"client_uri\": \"https://example.org/a/b/\""));
        for bad in [
            "http://example.org",
            "https://",
            "https://user@example.org",
            "https://example.org/?x=1",
            "https://example.org/#f",
            "https://exa mple.org",
            "https://example.org:http",
            "example.org",
        ] {
            assert!(base_url(bad).is_err(), "{bad}");
        }
    }

    /// The `_headers` the site ships carries threat-model.md §6.1's policy, exactly, for every
    /// path, and the §6.3 headers.
    #[test]
    fn the_headers_file_carries_the_production_policy_of_the_spec() {
        let tm = spec("docs/spec/threat-model.md");
        let policy = block_after(&tm, "### 6.1 Production policy")
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let headers = spec("deploy/site/_headers");
        assert_eq!(headers_csp(&headers), Some(policy.as_str()));
        for h in [
            "Referrer-Policy: no-referrer",
            "X-Content-Type-Options: nosniff",
            "Cross-Origin-Opener-Policy: same-origin",
            "Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=()",
            "Access-Control-Allow-Origin: *",
        ] {
            assert!(headers.contains(h), "{h}");
        }
    }

    /// The Pages fallback check accepts a `<head>` that starts with the §6.3 `<meta>` and
    /// refuses one that keeps `frame-ancestors` or puts something first.
    #[test]
    fn the_pages_index_check_wants_the_meta_policy_first() {
        let policy = "default-src 'none'; frame-ancestors 'none'; trusted-types a b";
        let good = "<html><head>\n    <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; trusted-types a b\" />\n<title>x</title>";
        assert!(check_pages_index(good, policy).is_ok());
        let with_frames =
            good.replace("'none'; trusted", "'none'; frame-ancestors 'none'; trusted");
        assert!(check_pages_index(&with_frames, policy).is_err());
        let late = good.replace("<head>\n", "<head>\n<script src=x></script>");
        assert!(check_pages_index(&late, policy).is_err());
    }

    /// A build's documents name the namespace the SPA requests scopes in (auth-bridge.ts `NS`):
    /// the .temp one in this tree until the release.
    #[test]
    fn the_spa_namespace_comes_from_the_engine_bridge() {
        let bridge = spec("packages/engine/src/auth-bridge.ts");
        assert_eq!(spa_namespace(&bridge), Ok(Ns::Temp));
        assert_eq!(
            spa_namespace("export const NS = \"online.7778777.lessdugin\";\n"),
            Ok(Ns::Final)
        );
        assert!(spa_namespace("export const NS = \"com.example\";").is_err());
        assert!(spa_namespace("const ns = 1;").is_err());
    }

    #[test]
    fn site_refuses_a_final_namespace_build_and_bad_arguments() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(site(&args(&["--origin", "https://example.org", "--ns", "final"])).is_err());
        assert!(site(&args(&["--ns", "temp"])).is_err());
        assert!(site(&args(&["--origin", "https://example.org", "--ns", "x"])).is_err());
        assert!(site(&args(&["--help"])).is_ok());
    }
}
