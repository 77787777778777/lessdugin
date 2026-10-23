// SPDX-License-Identifier: AGPL-3.0-or-later
//! The embedded SPA (`build.rs`, `threat-model.md` WEB-5) and the placeholder page served when
//! the binary was built without `web/dist`.
//!
//! Files are looked up by exact path in a table compiled into the binary: nothing is read from
//! the file system at run time, so there is no path traversal to defend against.

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
}

/// The page served at `/` when the SPA is not embedded. It has no script and no inline style,
/// so the production CSP applies to it unchanged.
pub const PLACEHOLDER_HTML: &str = "<!doctype html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
<title>lessdugin web: the web app is not built in</title>
</head>
<body>
<h1>The lessdugin web app is not built into this binary</h1>
<p>This <code>lessdugin</code> was compiled without <code>web/dist</code>, so
<code>lessdugin web</code> can only show this page. To build the web app into it, run in the
source tree:</p>
<pre>pnpm install
pnpm --filter @lessdugin/web build
cargo build -p lessdugin</pre>
<p>then start <code>lessdugin web</code> again. Release binaries always contain the web app.</p>
</body>
</html>
";

/// One file to serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asset {
    /// The `Content-Type`.
    pub content_type: &'static str,
    /// The contents.
    pub body: &'static [u8],
    /// `Cache-Control`: `no-cache` for `index.html` (`threat-model.md` §6.3), long-lived for the
    /// content-hashed build outputs under `assets/`.
    pub cache_control: &'static str,
}

/// The files `lessdugin web` serves.
#[derive(Clone, Copy, Debug)]
pub struct Assets {
    files: &'static [(&'static str, &'static [u8])],
    placeholder: bool,
}

/// `Content-Type` by extension (the same table as the e2e static server, `web/e2e/serve.mjs`).
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl Assets {
    /// The SPA compiled into this binary (or the placeholder, if it was built without one).
    pub fn embedded() -> Self {
        Self {
            files: embedded::FILES,
            placeholder: embedded::PLACEHOLDER,
        }
    }

    /// Only the placeholder page.
    #[cfg(test)]
    pub fn placeholder() -> Self {
        Self {
            files: &[],
            placeholder: true,
        }
    }

    /// True when only the placeholder page is served.
    pub fn is_placeholder(&self) -> bool {
        self.placeholder
    }

    /// The SPA's build hash (`build-info.json` `sha256`, SUP-4), when the SPA is embedded.
    pub fn build_hash(&self) -> Option<String> {
        let (_, bytes) = self.files.iter().find(|(p, _)| *p == "build-info.json")?;
        let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let h = v.get("sha256")?.as_str()?;
        (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_owned())
    }

    /// The file for a request path (`/` is `index.html`); `None` if there is none.
    pub fn get(&self, path: &str) -> Option<Asset> {
        let rel = match path.strip_prefix('/')? {
            "" => "index.html",
            p => p,
        };
        if self.placeholder {
            return (rel == "index.html").then_some(Asset {
                content_type: "text/html; charset=utf-8",
                body: PLACEHOLDER_HTML.as_bytes(),
                cache_control: "no-cache",
            });
        }
        let i = self.files.binary_search_by(|(p, _)| (*p).cmp(rel)).ok()?;
        let (name, body) = self.files[i];
        let cache_control = if name.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        Some(Asset {
            content_type: content_type(name),
            body,
            cache_control,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_serves_only_its_page() {
        let a = Assets::placeholder();
        assert!(a.is_placeholder());
        let page = a.get("/").unwrap();
        assert_eq!(page, a.get("/index.html").unwrap());
        assert!(page.content_type.starts_with("text/html"));
        assert_eq!(page.cache_control, "no-cache");
        assert!(a.get("/assets/x.js").is_none());
        assert!(a.get("relative").is_none());
        assert!(a.build_hash().is_none());
        let text = std::str::from_utf8(page.body).unwrap();
        assert!(text.contains("pnpm --filter @lessdugin/web build"));
        // CSP-clean: no script and no inline style.
        assert!(!text.contains("<script") && !text.contains("style"));
    }

    #[test]
    fn embedded_table_is_sorted_and_relative() {
        let a = Assets::embedded();
        for w in embedded::FILES.windows(2) {
            assert!(w[0].0 < w[1].0, "{} !< {}", w[0].0, w[1].0);
        }
        for (p, _) in embedded::FILES {
            assert!(
                !p.starts_with('/') && !p.contains("..") && !p.contains('\\'),
                "{p}"
            );
        }
        if !a.is_placeholder() {
            assert!(a.get("/").is_some());
            assert!(a.get("/index.html").is_some());
        }
        assert!(a.get("/../Cargo.toml").is_none());
        assert!(a.get("/does-not-exist").is_none());
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("a/b.wasm"), "application/wasm");
        assert_eq!(content_type("x.js.map"), "application/json");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }
}
