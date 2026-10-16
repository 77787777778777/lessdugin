<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0002 — S3/S10: wasm size budget and the math engine

- Status: Accepted
- Date: 2026-10-16
- Spikes: S3 (wasm size with and without math-core and k256/p256) and S10 (math-core coverage and
  the cost of a Temml fallback)

## Context

- The SPA runs the Rust engine as one `wasm-bindgen --target web` module in a Web Worker
  (synthesis §6.6). Budget: **900 KB gzipped target, CI fails above 1.1 MB**. If verification pushes
  the build over budget, it moves into a lazily loaded `verify.wasm`.
- LFM/1 math (§2.7) renders TeX to MathML through math-core. If S10 fails, the fallback is
  `<span data-tex>` plus Temml loaded lazily. D15 says one Rust renderer produces HTML+MathML for the
  SPA and for export, and P4 requires native and wasm output to be byte-identical.
- `lessdugin-markdown` lists ammonia behind a native-only `ammonia` feature. This spike checks that
  it has to stay out of the wasm build.
- The SPA CSP is `style-src 'self'`, with no `'unsafe-inline'`, and DOMPurify must leave the
  renderer's output unchanged (§5.2, M4). So the exact markup a math engine emits matters, not only
  whether it parses the TeX.

## Method

### Environment

| Tool | Version |
|---|---|
| rustc / cargo | 1.97.0 (2026-07-07) / 1.97.0, target `wasm32-unknown-unknown` |
| wasm-bindgen CLI | 0.2.129 (`~/.cargo/bin/wasm-bindgen`) |
| wasm-opt (binaryen) | version 132 (`/home/user/lessdugin/node_modules/.bin/wasm-opt`) |
| twiggy | 0.8 (installed into the scratch directory for attribution only) |
| gzip | 1.12 (`gzip -9`); brotli is Node 22.22.0 `zlib.brotliCompressSync`, quality 11 |

### Scratch project

The scratch project was a standalone cargo project, outside the repo workspace, with its own empty
`[workspace]` table. It was a `cdylib`. Each variant is a cargo feature. Each feature adds one
dependency group and a `#[wasm_bindgen]` export that really uses the dependency, with all inputs
coming from JS:

| Export | Feature | What it does |
|---|---|---|
| `roundtrip(JsValue, &str)` | always | `serde_wasm_bindgen::from_value` into a nested struct, `serde_json::to_string`, `serde_json::from_str`, then `to_value` |
| `render_markdown(&str)` | `md` | pulldown-cmark `Parser::new_ext` with `TABLES\|FOOTNOTES\|STRIKETHROUGH\|TASKLISTS\|MATH\|GFM` and a custom event loop (about 200 lines: escaping, `safe_href`, alerts, tables with `ta-*` classes, footnote ordinals, task lists, `cid:` images). Without a math engine it emits `<span data-tex>`. |
| (same, math path) | `mathcore` | `LatexToMathML::new(MathCoreConfig::default(), vec![])` then `convert_with_local_state(tex, Inline\|Block)`. An error becomes `<code class="math-error">`. |
| (same, math path) | `pdlatex` | `pulldown_latex::{Parser, Storage, push_mathml, RenderConfig}` |
| (same, sanitising) | `ammonia` | `ammonia::Builder` with MathML tags added, then `.clean(&html)` |
| `verify_block(block, key, curve, cid)` | `crypto` | `serde_ipld_dagcbor::from_slice::<Ipld>`, walks the links, CIDv1(dag-cbor, sha2-256) of the block and compares it with the expected CID string. Removes `sig`, re-encodes, sha256, then `VerifyingKey::from_sec1_bytes` + `Signature::from_slice` + `verify_prehash` + a low-S check, for k256 or p256. |
| `x_*` | `x-norm`, `x-seg`, `x-sec`, `x-sec-mixed`, `x-width`, `x-url`, `x-mb`, `x-libm`, `x-fut` | One call each into the other wasm-safe dependencies listed in §6.2: NFC, graphemes and words, `skeleton` + `is_single_script`, `is_single_script` + `identifier_allowed` only, width, `Url::parse`, `multibase::decode` + `BASE32_NOPAD`, `libm::exp/log/pow`, `JsFuture` + `Uint8Array` |

Dependencies, as resolved by `cargo generate-lockfile` on 2026-10-16:
`wasm-bindgen =0.2.129`, `serde 1` (derive), `serde_json 1.0.154`, `serde-wasm-bindgen 0.6.5`,
`pulldown-cmark =0.13.4` (`default-features=false`), `math-core 0.9.0`
(with `math-core-renderer-internal 0.9.0`), `pulldown-latex 0.8.0`,
`k256 0.13.4` / `p256 0.13.2` (`default-features=false, features=["ecdsa","alloc"]`),
`sha2 0.10.9`, `cid 0.11.3`, `serde_ipld_dagcbor 0.7.0`, `ipld-core 0.4.3`, `ammonia 4.2.3`
(`html5ever 0.40.1`), `url 2.5.8` (idna 1.1.0, idna_adapter 1.2.2 = ICU4X),
`unicode-normalization 0.1`, `unicode-segmentation 1`, `unicode-security 0.1.2`,
`unicode-width 0.2`, `multibase 0.9`, `data-encoding 2`, `libm 0.2`, `wasm-bindgen-futures 0.4`,
`js-sys 0.3`. An extra feature, `crypto14`, uses `k256 0.14.0` / `p256 0.14.0` / `sha2 0.11.1` with
the same feature flags.

Profile, the same as the workspace `[profile.wasm]`:

```toml
[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
```

### Build and measure (one run per variant)

```sh
cargo build -q --profile release --target wasm32-unknown-unknown --features "$FEAT"
wasm-bindgen --target web --out-dir "$OUT" --out-name pkg target/wasm32-unknown-unknown/release/wasmsize.wasm
wasm-opt -Oz --enable-bulk-memory --enable-bulk-memory-opt --enable-call-indirect-overlong \
  --enable-multivalue --enable-mutable-globals --enable-nontrapping-float-to-int \
  --enable-reference-types --enable-sign-ext --strip-debug --strip-producers \
  "$OUT/pkg_bg.wasm" -o "$OUT/final.wasm"
gzip -9 -c "$OUT/final.wasm" | wc -c
```

The `--enable-*` list is exactly the `target_features` section that rustc 1.97 writes
(`+bulk-memory +bulk-memory-opt +call-indirect-overlong +multivalue +mutable-globals
+nontrapping-fptoint +reference-types +sign-ext`). wasm-opt 132 also reads that section by default,
so plain `-Oz` worked too.

### Attribution and smoke test

- **Attribution.** `wasm-opt -Oz -g …` (keeps names), then `twiggy top -n 100000 -f csv`. Function
  sizes were summed by the crate in the demangled name. Data segments cannot be attributed this
  way.
- **Smoke test** (Node 22.22.0). `initSync({module})` on `final.wasm` of every variant. It calls
  each export, renders a 330-byte LFM sample with 2 formulas (and that sample repeated 200 times),
  and verifies two signed DAG-CBOR commit-shaped blocks (secp256k1 and P-256). The blocks were
  generated natively with k256/p256 `sign_prehash` and low-S normalisation, and a tampered copy was
  also checked.

### Math coverage

- A native bin uses math-core 0.9.0 and pulldown-latex 0.8.0 in display mode. A Node script loads
  `temml@0.14.0` (`renderToString`, `throwOnError:true, trust:false`) and `katex@0.19.0`
  (`output:"mathml"`, same flags) from the downloaded tarballs. Two corpora were run:
  1. **63 representative snippets**: 27 "core" forum constructs (everything listed in the S3/S10
     brief), 23 "ext" constructs, and 13 "sec" inputs (`\href \url \includegraphics \class \style
     \htmlClass \newcommand \def \let`, a 2¹¹ macro-expansion bomb, attribute-injection attempts).
  2. **476 single-command probes**: 312 symbols and operator names, 94 one-argument commands
     (accents, fonts, text, boxes, sizes, extensible arrows), 13 two-argument commands, 25
     environments, and 32 miscellaneous constructs. 3 environment probes (`gathered`, `gather`,
     `gather*`) were first written with `&`, which those environments reject. They were rerun with a
     valid body, and the totals below use the corrected runs.
- A failure is an `Err` for math-core, a `<merror>` in the output for pulldown-latex (it reports
  errors inline), and a thrown error for Temml and KaTeX. A KaTeX `\href` rendered as red error text
  under `trust:false` is reported as "rendered as error text".
- **Native vs wasm parity.** Every snippet of both corpora was rendered through the wasm V2 build
  (`render_markdown("$$…$$")`), and the `<math>` element was compared byte-for-byte with native
  math-core.

### JS packages and network use

- `npm view <pkg> version time dist.*`, then `npm pack temml@0.14.0`, `katex@0.19.0` and
  `math-core@0.9.0` (its wasm build), each extracted into its own empty scratch directory.
  `gzip -9` was run on the dist files.
- Network: about 10 npm registry requests (`npm view` ×7, `npm pack` ×3), 4 crates.io API calls
  (`GET /api/v1/crates/{math-core,pulldown-latex,k256,p256}/versions?per_page=6`, with
  `User-Agent: lessdugin-spike/0.0 (+https://lessdugin.7778777.online)`), and cargo's own sparse
  index and crate downloads. Nothing was written anywhere. No PDS was contacted.

## Results

### 1. Size per variant (bytes; "final" = after wasm-opt; gz = `gzip -9`)

| Variant | Contents | cargo `.wasm`¹ | after wasm-bindgen² | **final** | **final gz** | final br | JS glue gz |
|---|---|---:|---:|---:|---:|---:|---:|
| V0 | wasm-bindgen + serde_json + serde-wasm-bindgen | 2,187,890 | 186,589 | **97,573** | **45,974** | 39,444 | 4,159 |
| V1 | V0 + pulldown-cmark 0.13.4 | 2,425,748 | 426,395 | **260,582** | **117,375** | 98,208 | 4,265 |
| V2 | V1 + math-core 0.9.0 | 2,669,954 | 678,614 | **420,895** | **185,896** | 154,277 | 4,265 |
| V3 | V1 + pulldown-latex 0.8.0 | 2,627,881 | 621,297 | **394,598** | **167,333** | 139,251 | 4,265 |
| V4 | V0 + k256/p256 0.13 + sha2 + cid + serde_ipld_dagcbor | 2,562,211 | 559,797 | **305,653** | **114,281** | 93,356 | 4,414 |
| V5 | V2 + V4 + ammonia 4.2.3 | 4,001,935 | 2,000,833 | **1,258,057** | **517,522** | 415,032 | 4,475 |
| V6 | V2 + V4 (expected production set) | 3,033,326 | 1,041,742 | **620,678** | **251,426** | 205,381 | 4,475 |

¹ Contains std debug sections and the `__wasm_bindgen_unstable` section. It is meaningless for the
budget.
² Still contains the `name` section, which wasm-opt drops. **The budget must be measured on the
wasm-opt output.**

Additional variants:

| Variant | Contents | final | final gz |
|---|---|---:|---:|
| V6s | V6 built with `opt-level="s"` | 658,394 | 263,547 |
| V1a | V1 + ammonia | 906,774 | 385,493 |
| V4-nomb | V4, but CID strings via a 40-line base32-lower encoder/decoder instead of `Cid: Display/TryFrom<&str>` (multibase) | 211,843 | 92,429 |
| V6-nomb | V6 with the same change | **531,114** | **230,965** |
| V0+cbor | V0 + sha2 + cid + dagcbor, no ECDSA (nomb) | 161,864 | 71,093 |
| V2+cbor | V2 + sha2 + cid + dagcbor, no ECDSA (nomb) | 481,307 | 209,115 |
| V4b / V6b | V4 / V6 with k256 0.14.0 + p256 0.14.0 + sha2 0.11.1 | 382,961 / 697,953 | 140,782 / 278,192 |
| V4b-nomb / V6b-nomb | same, nomb | 289,348 / 608,426 | 118,872 / **258,475** |
| V7 | V6 + every `x-*` extra (unicode ×4, url, multibase, libm, futures) | 1,116,258 | 460,853 |
| V7-lean | V6-nomb + all extras, `idna_adapter =1.0.0` | 992,022 | **409,561** |
| V7-lean-nosec | V7-lean without unicode-security | 855,631 | 361,556 |

### 2. Marginal cost of each dependency (final, after wasm-opt)

| Dependency | Measured as | Δ raw | Δ gz |
|---|---|---:|---:|
| Runtime base (std fmt, dlmalloc, serde_json, wasm-bindgen) | V0 | 97,573 | 45,974 |
| pulldown-cmark 0.13.4 + our event loop | V1 − V0 | +163,009 | +71,401 |
| **math-core 0.9.0** | V2 − V1 | **+160,313** | **+68,521** |
| pulldown-latex 0.8.0 | V3 − V1 | +134,016 | +49,958 |
| sha2 + cid + serde_ipld_dagcbor + ipld-core (no multibase) | V0+cbor − V0 | +64,291 | +25,119 |
| **ECDSA k256 0.13 + p256 0.13** | V6-nomb − V2+cbor | **+49,807** | **+21,850** |
| ECDSA k256 0.14 + p256 0.14 (sha2 0.11) | V6b-nomb − V2+cbor | +127,119 | +49,360 |
| multibase via `Cid` string impls | V6 − V6-nomb | +89,564 | +20,461 |
| **ammonia 4.2.3 (html5ever 0.40)** | V5 − V6 | **+637,379** | **+266,096** |
| ammonia (second measurement) | V1a − V1 | +646,192 | +268,118 |
| unicode-normalization (NFC) | V6+norm − V6 | +121,426 | +61,400 |
| unicode-segmentation | V6+seg − V6 | +53,386 | +20,964 |
| unicode-security `skeleton` (confusables) + mixed script | V6+sec − V6 | +245,774 | +104,204 |
| unicode-security mixed script + `identifier_allowed` only | V6-nomb+mixed − V6-nomb | +40,015 | +17,390 |
| unicode-width | V6+width − V6 | +11,952 | +4,717 |
| url 2.5.8, default idna_adapter 1.2.2 (ICU4X) | V6+url − V6 | +160,259 | +68,617 |
| url with `idna_adapter =1.1.0` (unicode-rs backend) | | +301,533 | +129,389 |
| url with `idna_adapter =1.0.0` (ASCII-only, no IDNA) | | +51,966 | +22,504 |
| multibase + data-encoding (already present through cid) | V6+mb − V6 | +441 | +176 |
| libm | V6+libm − V6 | +3,205 | +1,592 |
| wasm-bindgen-futures + js-sys | V6+fut − V6 | +8,568 | +3,251 (+1,124 JS gz) |

Other pipeline observations:
- wasm-opt `-Oz` takes V6 from 1,041,742 to 620,678 bytes (−40 % raw, −14 % gz).
- `opt-level="s"` is 12,121 bytes gz *larger* than `"z"`.
- `wasm-opt -Oz --converge` on V6-nomb saves 175 bytes gz (230,965 → 230,790) and costs 20 s. It
  is not worth it.

### 3. Where the bytes are (twiggy, V6-nomb, 531,253 bytes of code and data)

| Bytes | Share | Item |
|---:|---:|---|
| 156,272 | 29.4 % | data segments. In V1, one 53,867-byte segment is pulldown-cmark's HTML5 entity table (`CounterClockwiseContourIntegral` etc.), which CommonMark requires. The rest is math-core symbol/phf tables, unicase, and curve constants. |
| 67,208 + 17,113 | 15.9 % | math_core + math_core_renderer_internal |
| 58,097 | 10.9 % | wasm-bindgen export shims with inlined callees (mostly pulldown-cmark and math-core code inlined into `render_markdown`) |
| 41,202 | 7.8 % | core (fmt, f64 Display 13.9 KB, slices) |
| 30,691 | 5.8 % | pulldown_cmark (not inlined) |
| 16,391 | 3.1 % | k256 |
| 15,797 / 12,303 / 11,234 | 7.4 % | serde_json / serde_ipld_dagcbor / ipld_core |

In V6 (with multibase), multibase (25,578) + data_encoding (30,105) + base_x (3,592) =
59,275 bytes. They come only from `Cid::to_string()` and `Cid::try_from(&str)`, which pull in every
multibase codec.

### 4. Smoke test (Node 22, `final.wasm` of each variant)

- Every export worked after wasm-opt.
  - The markdown output escaped `<script>` and dropped `javascript:` hrefs.
  - V2, V3 and V6 emitted `<math>`. V1 emitted `data-tex`.
  - `verify_block` returned the expected CID and `sig_ok=true low_s=true expected_cid=true` for both
    curves. The tampered block returned `sig_ok=false expected_cid=false`.
- Render time for the 330-byte sample ×200 (66 KB, 400 formulas), mean of 5 runs:

  | Build | ms |
  |---|---:|
  | V1 (no math) | 8.8 |
  | V2 (math-core) | 15.6 |
  | V3 (pulldown-latex) | 15.3 |
  | V6 | 12.1 |
  | **V5 (with ammonia)** | **72.4** |

  With ammonia it is about 5–6× slower.
- ECDSA verification time in wasm, per signature (including CBOR decode and re-encode):

  | Build | ms |
  |---|---:|
  | k256/p256 0.13 | 1.17–1.92 |
  | 0.14 | 0.94–1.10 |

  `precomputed-tables` was off in both.

### 5. Math coverage

**Representative corpus (63 snippets).** Rows where at least one engine fails:

| # | Snippet | math-core 0.9.0 | pulldown-latex 0.8.0 | Temml 0.14.0 | KaTeX 0.19.0 |
|---|---|---|---|---|---|
| 17 | `\overline{AB} + \widehat{xyz} + \widetilde{abc} + \underline{u}` | **FAIL** `UnknownCommand("widetilde")` | ok | ok | ok |
| 19 | `\boxed{E = mc^2}` | ok | **FAIL** unknown primitive | ok | ok |
| 25 | `\mathbf{x}^\top … + \mathscr{F}` | ok | **FAIL** (`\mathscr`) | ok | ok |
| 29 | `\left(\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}\right)` | **FAIL** `UnknownEnvironment("smallmatrix")` | ok | ok | ok |
| 30 | `\sum_{\substack{0<i<m \\ 0<j<n}} P(i,j)` | **FAIL** `UnknownCommand("substack")` | ok | ok | ok |
| 36 | `\phantom{x}y\vphantom{\int}` | ok | **FAIL** | ok | ok |
| 38 | `\begin{equation} E = mc^2 \tag{1} \end{equation}` | ok | **FAIL** (`\tag`) | ok | ok |
| 42 | `\cancel{x} + \bcancel{y} + \xcancel{z}` | ok | **FAIL** (`\bcancel`, `\xcancel`) | ok | ok |
| 47 | `\ce{H2O + CO2 -> H2CO3}` (mhchem) | **FAIL** | **FAIL** | ok | FAIL (needs contrib) |

All 27 "core" items the brief lists (fractions, sums with limits, pmatrix/bmatrix/vmatrix, aligned
and align, `\mathbb`, `\operatorname(*)`, `\text`, `\left(…\right)`, cases, accents,
over/underbrace, `\boxed`, `\mathcal`, `\frac` in exponents, `\binom`, roots, …) pass in math-core
**except `\widetilde`** (26/27). pulldown-latex passes 25/27, and Temml and KaTeX pass 27/27.

Totals without the "sec" group:

| Engine | ok (of 50) |
|---|---:|
| math-core | 46 |
| pulldown-latex | 44 |
| Temml | 50 |
| KaTeX | 49 |

**Per-command probes (476).**

| Engine | sym (312) | 1-arg (94) | 2-arg (13) | env (25) | misc (32) | **total** |
|---|---:|---:|---:|---:|---:|---:|
| math-core 0.9.0 | 308 | 77 | 12 | 21 | 24 | **442 (92.9 %)** |
| pulldown-latex 0.8.0 | 309 | 61 | 13 | 24 | 19 | **426 (89.5 %)** |
| Temml 0.14.0 | 312 | 93 | 13 | 25 | 31 | **474 (99.6 %)** |
| KaTeX 0.19.0 (MathML) | 312 | 91 | 13 | 24 | 27 | **467 (98.1 %)** |

The 34 commands math-core does not support:
- Symbols: `\smallsetminus \centerdot \dotsb \dotsc`.
- Accents: `\widetilde \utilde`.
- Fonts and text: `\mathnormal \pmb \textnormal \mbox \hbox \textup \textmd`.
- Boxes: `\smash \colorbox \rule`.
- `\sout`, `\tiny…\Huge` (×6): `NotValidInMathMode`, which is correct LaTeX behaviour.
- Environments: `smallmatrix alignat split CD`.
- `\substack`; `\tag` outside a numbered environment (`$$a=b \tag{1}$$`).
- `\ce \pu` (mhchem); `\intertext \verb \char`.

The 50 commands pulldown-latex does not support include `\boxed \fbox \phantom \hphantom \vphantom
\mathscr \bm \Bbb \bcancel \xcancel \xRightarrow \xLeftarrow \xmapsto \xhookrightarrow \xleftrightarrow
\over \atop \choose \pod \tag \raisebox \mathchoice \dddot \emph`, `\label`/`\eqref` and `CD`.

**Security inputs (math-core).**
- `\href \url \includegraphics \class \style \htmlClass` all fail with `UnknownCommand`.
- `\color{red" onmouseover="alert(1)}{x}` and `\label{a"b}` fail with `ExpectedAscii`.
- `\text{"><img src=x onerror=alert(1)>}` is escaped (`&lt;img …&gt;`).
- **But math-core supports `\newcommand`, `\def` and `\let`.** They are scoped to the snippet. A
  2¹¹ expansion bomb stops with `TooManyExpansions` (the default cap is 1,000, the same as KaTeX's
  `maxExpand`). LFM/1 lists these macros as disabled, so math-core alone does not enforce §2.7.

**Markup math-core emits** (it matters for CSP and the DOMPurify-equality rule):
- **Inline `style` attributes** in 7 of the 46 successful corpus snippets: aligned, align, cases,
  array, spacing, equation numbering and color. The values seen form a small closed set:
  - `text-align: left|right;justify-items: start|end;padding-left|right: 0;`
  - `padding-left:1em`
  - `width: 50%` / `width: 100%` (tag columns)
  - `border-top|right: 0.05em solid|dashed currentcolor`
  - `margin-left:-0.16666667em` (`\!`)
  - `color:#RRGGBB`

  From the renderer source: `font-size` and `font-variant-caps: small-caps` too. With
  `style-src 'self'` and no `'unsafe-inline'`, browsers ignore these attributes. That breaks matrix
  and cases alignment and `\color`.
- `id="<label>"` on `\label` and `<a href="#<label>">` for `\eqref` (prefix configurable through
  `id_prefix`). This conflicts with the `md-*` ID scheme and the "content never sets
  `location.hash`" rule.
- `class="menclose-*"` (needs math-core's `mathmlfixes.css`) and `class="math-core-*-font"` spans in
  `\textsf`/`\textrm`.
- Self-closing `<mspace …/>` (196 occurrences in the probes) and `<mprescripts/>`. The HTML
  serializer writes these back as `<mspace …></mspace>`, so `DOMPurify(html) === html` would fail on
  every formula with spacing.
- Elements used: `math menclose mfrac mi mn mo mover mpadded mphantom mroot mrow mspace msqrt msub
  msubsup msup mtable mtd mtext mtr munder munderover` (plus `a`/`span` inside `mtext`).
- Attributes: `display lspace rspace stretchy width displaystyle scriptlevel accent accentunder
  notation linethickness form mathvariant minsize maxsize class style id href`.

Temml puts `style="display:block math;"` on every display formula and padding styles on most
tables, so it has the same CSP problem, and worse. KaTeX's MathML output uses presentational
attributes instead (`columnalign`, `rowspacing`, `mathcolor`), most of which are not in MathML Core.

**Parity.** All 539 snippets (63 + 476) were rendered natively and inside the wasm V2 build.
489 were **byte-identical**, 50 failed in both, and **0 differed**.

### 6. JS fallback packages (npm tarballs, `gzip -9`)

| Package | File | raw | gz |
|---|---|---:|---:|
| temml 0.14.0 (2026-10-08) | `dist/temml.min.js` | 167,881 | **49,566** |
| | `dist/temml.mjs` (unminified ESM) | 478,952 | 115,318 |
| | `dist/Temml-*.css` (one theme) | 8,794–9,404 | 2,744–2,994 |
| | `dist/Temml.woff2` | 9,404 | — |
| | `contrib/mhchem/mhchem.min.js` | 34,762 | 8,259 |
| katex 0.19.0 (2026-10-01) | `dist/katex.min.js` | 272,868 | **76,010** |
| | `dist/katex.min.css` | 24,793 | 3,526 |
| | `dist/fonts/*.woff2` (20 files, HTML output only) | 259,792 | — |
| | `dist/contrib/mhchem.min.js` | 33,706 | 8,410 |
| math-core 0.9.0 npm (its own wasm build) | `dist/math_core_bg.wasm` | 298,372 | 122,496 |
| | `dist/math_core.js` | 16,745 | 3,524 |

### 7. Release and maintenance data (crates.io API, npm `time`)

- **math-core:**

  | Version | Published |
  |---|---|
  | 0.6.4 | 2026-06-12 |
  | 0.7.0 | 2026-07-03 |
  | 0.8.0 | 2026-08-27 |
  | 0.8.1 | 2026-08-30 |
  | 0.8.2 | 2026-09-01 |
  | **0.9.0** | **2026-10-12** |

  0.9.0 was 4 days old on the day of this spike. MSRV 1.96, edition 2024, MIT. Upstream states
  that the goal is KaTeX parity and small wasm.
- **pulldown-latex:** 0.7.1 (2024-11-21), then 0.8.0 (2026-07-28), a 20-month gap. It needs its
  own stylesheet and fonts (`menv-*` classes).
- **k256 0.14.0** (2026-07-08) and **p256 0.14.0** (2026-07-03) are the current stable releases.
  The workspace `Cargo.toml` still pins `0.13` and `sha2 0.10`.

## Decision

1. **Math engine: math-core.** Pin it exactly at `=0.9.0` (the workspace currently has `"0.9"`), and
   pin `math-core-renderer-internal` in lockstep through it. S10 passes:
   - 26/27 core constructs and 92.9 % of 476 probed commands.
   - Errors are returned as values.
   - Native and wasm output are byte-identical.
   - Its marginal cost is 68.5 KB gz.

   It is not perfect. The gaps that matter for forum use are `\widetilde`, `\substack`,
   `smallmatrix`, `\tag` in `$$…$$`, `\mbox`/`\textnormal` and mhchem. We report them upstream, and
   until they are fixed they render through the normal `<code class="math-error">` path.
   - **pulldown-latex is rejected.** It saves 18.6 KB gz but has lower coverage (89.5 %, and it
     misses `\boxed`, `\phantom`, `\tag` and `\mathscr`), reports errors in-band as `<merror>`
     that we would have to scrape, needs its own CSS and fonts, and is maintained slowly.
   - **Temml is not shipped in v1.** It has the best coverage (99.6 %) and is only 49.6 KB gz
     lazily loaded. But it breaks D15/P4 (no native MathML for the CLI `export` or the indexer, so
     outputs differ by client), and every formula would need its inline styles rewritten in JS. It
     stays the documented contingency, at a measured cost of 49.6 KB gz plus 8.3 KB gz for mhchem
     plus about 3 KB gz CSS. We switch to `<span data-tex>` + lazy Temml only if a math-core
     upgrade regresses the `testdata/vectors/mathml` corpus and cannot be pinned around.
2. **The LFM/1 math contract gets two mandatory Rust passes around math-core.**
   - **Pre-scan (lexical, before math-core).** Reject the §2.7 disabled list, which math-core does
     *not* enforce for `\def`, `\newcommand` and `\let`, plus their siblings and the
     label/reference commands. The full list:
     `\def \gdef \edef \xdef \let \futurelet \newcommand \renewcommand \providecommand
     \DeclareMathOperator \label \eqref \ref`. Keep `\href \url \includegraphics \class \style
     \html*` in the list as well, even though math-core rejects them today.
     Also enforce the existing limits: at most 4 KB per expression and brace depth 64.
     Failures render as `<code class="math-error">`.
   - **MathML normaliser (after math-core).** A small iterative tokenizer over math-core's
     well-formed output. It:
     - Maps the closed set of `style` values above to fixed classes (`mc-al`, `mc-ar`, `mc-pl1`,
       `mc-w50`, `mc-w100`, `mc-bt-solid`, `mc-bt-dashed`, `mc-br-solid`, `mc-negthin`, …), shipped
       in a static `mathml.css`.
     - Turns `color:#RRGGBB` into `mathcolor="#RRGGBB"`. It is a MathML Core global attribute and
       the value is validated as hex.
     - Drops any style value it does not know, keeping the content (fail closed).
     - Strips `id`/`href`.
     - Expands self-closing tags to `<x></x>`.
     - Rejects any element or attribute outside the allowlist.

     The output must satisfy `DOMPurify(html) === html` under the §5.2 MathML allowlist.
3. **ammonia stays out of the wasm build** (confirmed). It costs +637 KB raw / **+266 KB gz**,
   106 % of the whole V6 production set, and makes rendering 5–6× slower. It stays behind the
   native-only `ammonia` feature as the test-time equality oracle. DOMPurify is the second
   sanitiser layer in the SPA. `lessdugin-wasm` should `compile_error!` if `ammonia` is enabled on
   `wasm32`.
4. **The 900 KB gz budget is realistic. Keep the 900 KB target and the 1.1 MB CI failure line.**
   - The expected production set (V6, CID strings without multibase) is **531 KB raw / 231 KB gz**.
     With k256/p256 0.14 it is 608 KB raw / 258 KB gz.
   - Adding every other declared wasm-safe dependency in its lean form (V7-lean) gives **992 KB raw /
     410 KB gz**.
   - That leaves about **490 KB gz** for the engine's own code: fold, sync, outbox, lexicon
     validator with embedded lexicon JSON, and tsify/serde DTO glue. This part is not measured
     here; M2's `xtask wasm --report` measures it.
   - Measure gzip -9 of the wasm-opt output. Report raw size too: compile time scales with raw
     bytes, so CI should warn above 2.5 MB raw.
   - Keep `opt-level="z"` and do not use `--converge`.
5. **Lazy `verify.wasm` split threshold.** Do not split now.
   - The verify-only code is ECDSA: +21.9 KB gz with 0.13, +49.4 KB gz with 0.14. The MST/CAR
     walk adds a few KB more.
   - CBOR, CID and sha2 (+25 KB gz) are needed in the main module anyway for the `pds` trust level.
   - A separate `verify.wasm` would duplicate the runtime base (V0 = 46 KB gz) and the CBOR/CID code.
     Standalone it measured 92–119 KB gz (V4-nomb, V4b-nomb), so splitting saves at most about
     50 KB gz on first load and costs about 100 KB gz the first time verification runs.
   - **Rule:** split only when the main module's gz size is over 900 KB **and** the verify feature's
     marginal size is at least 100 KB gz (about 2× the duplicated base). Measure the marginal size
     in CI as `gz(main) − gz(main without --features verify)`.
6. **Size hygiene rules for the wasm-safe crates** (atp, lexicon, markdown, core):
   - Format and parse CID strings with our own base32-lower code (multibase `b` is the only form
     atproto uses). Do not call `Cid: Display`/`TryFrom<&str>` or `multibase::decode` on wasm code
     paths: they cost 20.5 KB gz. Decode did:key with a base58btc-only decoder.
   - Pin `idna_adapter =1.0.0` (ASCII-only), or keep the `url` crate out of the wasm-safe crates.
     ICU4X IDNA costs 68.6 KB gz, against 22.5 KB gz for ASCII-only. LFM's URL policy should then
     show non-ASCII (IDN) hosts as text. That also closes the IDN-homograph hole. The final choice is
     for the M2 owner of `safe_href`.
   - Use only `unicode-security`'s mixed-script and `identifier_allowed` APIs in wasm (+17.4 KB gz).
     The crate with `skeleton()` (confusables) costs +48 KB gz even when NFC is already linked
     (V7-lean − V7-lean-nosec), and +104 KB gz on its own. That is justified only if lookalike
     detection of handles or forum names becomes a requirement.
   - Use k256/p256 `=0.14.0` with `sha2 0.11`, as §6.2 asks for "latest stable". It costs +27.5 KB
     gz more than 0.13, is about 25 % faster per verify in wasm, and avoids a later breaking bump.
     This means updating the workspace pins from `0.13`/`0.10`, and nothing else may pull in
     `sha2 0.10`.

## Consequences

- **`crates/markdown`:**
  - Add `math/prescan.rs` (denylist and limits) and `math/normalize.rs` (style→class map,
    `mathcolor`, strip `id`/`href`, expand self-closing tags, allowlist).
  - Vectors go in `testdata/vectors/mathml`. Seed them with this spike's 63-snippet corpus and the
    476 probes, with the currently expected pass/fail recorded so that any math-core bump that
    changes coverage shows up in review.
  - The ADR's style list is the initial closed set. An unknown style is dropped, and a test asserts
    that the corpus produces no unknown styles.
- **`web/`:**
  - Ship math-core's `mathmlfixes.css` (MIT; `mtd` padding, the `menclose` polyfill for Chromium,
    accent margins) plus our `mc-*` classes in the static `mathml.css`.
  - math-core recommends the patched math fonts from `math-core-fonts` (New Computer Modern,
    Libertinus or Noto Sans Math) because of Chromium `ssty` and Safari accent bugs. M4 should
    consider using one of those, subset, instead of plain STIX Two Math.
- **CSP** stays `style-src 'self'`. Inline `style` attributes are never emitted, so
  `style-src-attr 'unsafe-inline'` is not needed.
- **`docs/spec/lfm-1.md`:**
  - The disabled-macro list grows by `\edef \xdef \futurelet \renewcommand \providecommand
    \DeclareMathOperator \label \eqref \ref`.
  - Equation numbering through `\tag` inside `equation` stays allowed. Bare `\tag` in `$$…$$` is an
    error until math-core supports it.
- **xtask:**
  - `wasm --budget` measures `gzip -9` of the wasm-opt output, with the explicit `--enable-*` list
    above plus `--strip-debug --strip-producers`.
  - `wasm --report` also builds without `verify` and prints the verify marginal size, used by rule 5.
  - Binaryen's `wasm-split` (splitting into secondary functions without duplicating the runtime)
    is the tool to evaluate if rule 5 ever triggers. It is not used now.
- **Supply chain:** math-core 0.9.0 is new and changes about monthly. It needs a cargo-vet audit
  entry. Every bump has to pass the mathml vectors unchanged, or the changes are reviewed and
  re-blessed.
- **Not done in this spike:**
  - Browser-side timing: streaming compile and instantiate on mid-range mobile.
  - The size of the real engine code.
  - The effect of `serde_json`'s `preserve_order` feature, which the workspace enables and which
    this spike did not measure.

  M2's CI size report covers these.

<details>
<summary>Appendix: full 63-snippet corpus results</summary>

| # | Group | Name | TeX | math-core | pulldown-latex | Temml | KaTeX (MathML) |
|---|---|---|---|---|---|---|---|
| 1 | core | frac | `\frac{a+b}{c-d}` | ok | ok | ok | ok |
| 2 | core | sum-limits | `\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}` | ok | ok | ok | ok |
| 3 | core | int | `\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}` | ok | ok | ok | ok |
| 4 | core | lim | `\lim_{x \to 0} \frac{\sin x}{x} = 1` | ok | ok | ok | ok |
| 5 | core | pmatrix | `\begin{pmatrix} a & b \\ c & d \end{pmatrix}` | ok | ok | ok | ok |
| 6 | core | bmatrix | `\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}` | ok | ok | ok | ok |
| 7 | core | vmatrix | `\det A = \begin{vmatrix} a & b \\ c & d \end{vmatrix}` | ok | ok | ok | ok |
| 8 | core | aligned | `\begin{aligned} f(x) &= (x+1)^2 \\ &= x^2 + 2x + 1 \end{aligned}` | ok | ok | ok | ok |
| 9 | core | align-env | `\begin{align} a &= b + c \\ d &= e \end{align}` | ok | ok | ok | ok |
| 10 | core | mathbb | `f\colon \mathbb{R}^n \to \mathbb{C}` | ok | ok | ok | ok |
| 11 | core | operatorname | `\operatorname{argmax}_{x \in X} f(x)` | ok | ok | ok | ok |
| 12 | core | operatorname* | `\operatorname*{arg\,min}_{\theta} L(\theta)` | ok | ok | ok | ok |
| 13 | core | text | `y = 1 \text{ if } x > 0` | ok | ok | ok | ok |
| 14 | core | left-right | `\left( \frac{a}{b} \right)^2 + \left\lVert v \right\rVert` | ok | ok | ok | ok |
| 15 | core | cases | `\|x\| = \begin{cases} x & x \ge 0 \\ -x & \text{otherwise} \end{cases}` | ok | ok | ok | ok |
| 16 | core | accents | `\hat{x} + \bar{y} + \vec{v} + \tilde{a} + \dot{x} + \ddot{x}` | ok | ok | ok | ok |
| 17 | core | wide-accents | `\overline{AB} + \widehat{xyz} + \widetilde{abc} + \underline{u}` | FAIL unknown `\widetilde` | ok | ok | ok |
| 18 | core | overbrace | `\overbrace{a + b + c}^{\text{sum}} = \underbrace{1+\dots+1}_{n\text{ times}}` | ok | ok | ok | ok |
| 19 | core | boxed | `\boxed{E = mc^2}` | ok | FAIL | ok | ok |
| 20 | core | mathcal | `\mathcal{L}(\theta) = \mathcal{O}(n \log n)` | ok | ok | ok | ok |
| 21 | core | frac-in-exp | `e^{\frac{x}{2}} + 2^{\frac{1}{n}}` | ok | ok | ok | ok |
| 22 | core | binom | `\binom{n}{k} = \frac{n!}{k!\,(n-k)!}` | ok | ok | ok | ok |
| 23 | core | sqrt-root | `\sqrt[3]{x} + \sqrt{x^2+1}` | ok | ok | ok | ok |
| 24 | core | prod-text-sub | `\prod_{p \text{ prime}} \frac{1}{1-p^{-s}}` | ok | ok | ok | ok |
| 25 | core | fonts | `\mathbf{x}^\top A \mathbf{x} + \mathfrak{g} + \mathsf{T} + \mathtt{x} + \mathrm{d}x + \mathscr{F}` | ok | FAIL | ok | ok |
| 26 | core | greek-relations | `\alpha \leq \beta \neq \gamma \approx \delta \equiv \varepsilon \pmod{n}` | ok | ok | ok | ok |
| 27 | core | dots-delims | `\{ x \mid x > 0 \} \cup \langle u, v \rangle, \ldots, \cdots, \vdots, \ddots` | ok | ok | ok | ok |
| 28 | ext | array-hline | `\begin{array}{c\|c} 1 & 2 \\ \hline 3 & 4 \end{array}` | ok | ok | ok | ok |
| 29 | ext | smallmatrix | `\left(\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}\right)` | FAIL unknown env | ok | ok | ok |
| 30 | ext | substack | `\sum_{\substack{0<i<m \\ 0<j<n}} P(i,j)` | FAIL unknown `\substack` | ok | ok | ok |
| 31 | ext | xrightarrow | `A \xrightarrow{f} B \xleftarrow[g]{} C` | ok | ok | ok | ok |
| 32 | ext | overset | `\overset{\text{def}}{=} \stackrel{?}{=}` | ok | ok | ok | ok |
| 33 | ext | dfrac-tfrac | `\dfrac{1}{2} + \tfrac{3}{4}` | ok | ok | ok | ok |
| 34 | ext | big-delims | `\big( \Big[ \bigg\{ \Bigg\langle x \Bigg\rangle \bigg\} \Big] \big)` | ok | ok | ok | ok |
| 35 | ext | spacing | `a\,b\;c\quad d\qquad e\!f \hspace{1em} g` | ok | ok | ok | ok |
| 36 | ext | phantom | `\phantom{x}y\vphantom{\int}` | ok | FAIL | ok | ok |
| 37 | ext | gathered | `\begin{gathered} a = b \\ c = d \end{gathered}` | ok | ok | ok | ok |
| 38 | ext | equation-tag | `\begin{equation} E = mc^2 \tag{1} \end{equation}` | ok | FAIL | ok | ok |
| 39 | ext | iint-oint | `\iint_D f \, dA + \oint_C \mathbf{F} \cdot d\mathbf{r}` | ok | ok | ok | ok |
| 40 | ext | nabla-partial | `\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}, \frac{\partial f}{\partial x}` | ok | ok | ok | ok |
| 41 | ext | more-accents | `\mathring{A} + \check{x} + \breve{x} + \acute{e} + \grave{a}` | ok | ok | ok | ok |
| 42 | ext | cancel | `\cancel{x} + \bcancel{y} + \xcancel{z}` | ok | FAIL | ok | ok |
| 43 | ext | color | `\color{red}{x} + \textcolor{blue}{y}` | ok | ok | ok | ok |
| 44 | ext | not-in | `a \not\in B, \nexists x, \notin` | ok | ok | ok | ok |
| 45 | ext | mod-funcs | `\gcd(a,b) = \max(\deg p, \det M) \bmod n` | ok | ok | ok | ok |
| 46 | ext | unicode-input | `α + β = γ → ∞` | ok | ok | ok | ok |
| 47 | ext | mhchem | `\ce{H2O + CO2 -> H2CO3}` | FAIL unknown `\ce` | FAIL | ok | FAIL |
| 48 | ext | matrix-dots | `\begin{pmatrix} a_{11} & \cdots & a_{1n} \\ \vdots & \ddots & \vdots \\ a_{m1} & \cdots & a_{mn} \end{pmatrix}` | ok | ok | ok | ok |
| 49 | ext | limits-nolimits | `\sum\nolimits_{i} x_i + \int\limits_0^1 f` | ok | ok | ok | ok |
| 50 | ext | displaystyle | `\textstyle \sum_i x_i \displaystyle \sum_i x_i` | ok | ok | ok | ok |
| 51 | sec | href | `\href{https://evil.example}{x}` | FAIL unknown | FAIL | FAIL | error text |
| 52 | sec | url | `\url{https://evil.example}` | FAIL unknown | FAIL | FAIL | error text |
| 53 | sec | includegraphics | `\includegraphics{https://evil.example/a.png}` | FAIL unknown | FAIL | FAIL | error text |
| 54 | sec | class | `\class{evil}{x}` | FAIL unknown | FAIL | FAIL | FAIL |
| 55 | sec | style | `\style{color:red}{x}` | FAIL unknown | FAIL | FAIL | FAIL |
| 56 | sec | htmlClass | `\htmlClass{evil}{x}` | FAIL unknown | FAIL | FAIL | error text |
| 57 | sec | newcommand | `\newcommand{\RR}{\mathbb{R}} x \in \RR` | **ok (expands)** | FAIL | ok | ok |
| 58 | sec | def | `\def\foo{y}\foo` | **ok (expands)** | ok | ok | ok |
| 59 | sec | let | `\let\foo\alpha \foo` | **ok (expands)** | ok | ok | ok |
| 60 | sec | macro-bomb | 12 nested `\newcommand`s, each doubling (2¹¹ expansions) | FAIL `TooManyExpansions` | FAIL | FAIL (too many expansions) | FAIL (too many expansions) |
| 61 | sec | quote-attr | `\text{"><img src=x onerror=alert(1)>}` | ok (escaped) | ok (escaped) | ok (escaped) | ok (escaped) |
| 62 | sec | color-inject | `\color{red" onmouseover="alert(1)}{x}` | FAIL `ExpectedAscii` | FAIL (unknown color) | FAIL | FAIL |
| 63 | sec | label-ref | `x \label{a"b} \eqref{a"b}` | FAIL `ExpectedAscii` | FAIL | ok (emits `href='#ab'`) | FAIL |

</details>
