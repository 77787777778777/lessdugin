<!-- SPDX-License-Identifier: CC0-1.0 -->

# LFM/1 implementation notes

Notes from implementing `docs/spec/lfm-1.md` in `crates/markdown` and writing these vectors (M1),
reconciled with the spec at M4. Every item below is **closed**: its resolution is now normative
text in `lfm-1.md`, and the arrow points to it and to the vectors that pin it. Items about §7
(math) were closed with `docs/spec/README.md` K3, which folded ADR 0002 into §7; they are marked
"K3". Section numbers refer to `lfm-1.md`; `§14 n` is decision n there. Numbering is kept from
M1.

## Decisions where the spec was ambiguous

### Semantics (`lfm/1` layer)

1. **Rendered footnotes.** The fixed point "first definition of a label referenced from the
   body or from a rendered definition"; nested definitions are extracted.
   → §4.6, §14 14 (`footnotes.json#reference-only-from-unrendered-definition`,
   `#nested-definition-extracted`, `#nested-definition-unreferenced`).
2. **Derived metadata scope.** *Changed at M4.* `links`, `mentions` and `media_used` come from
   the rendered content of S6 (the body and the rendered footnote definitions, outside image
   alts), not from SL: they are `lfm/1` values and must not depend on the limit stage or the
   output budget, or two releases could publish different `markdown.mentions` for one text. A
   flattened link therefore still counts (it is still shown); an unreferenced or duplicate
   definition and an image alt do not. → §1.2, §1.5, §6.3, §12 "Scope", §14 26
   (`at-links.json#unreferenced-footnote-not-derived`, `#duplicate-definition-not-derived`,
   `#link-in-image-alt-not-derived`, `#depth-flattened-still-derived`,
   `#spoiler-content-derived`; `gallery.json#image-only-in-unreferenced-footnote`,
   `#image-in-referenced-footnote-used`, `#image-only-inside-alt-not-used`).
3. **`mailto`.** One mailbox: one address before `?`, `http` host grammar, no `to`/`cc`/`bcc`
   fields. → §5.1, §14 29 (`links.json#mailto-*`, `safe-href.json#mailto-*`).
4. **Malformed `cid:`** is a miss; a `cid:` *link* (not an image) is dropped. → §6.1
   (`images.json#cid-link-not-image-dropped`).
5. **Record NSIDs.** An upper-case NSID authority is rejected; `Record` carries the canonical
   URI (scheme `at`). → §5.2 (`at-links.json#uppercase-nsid-authority`, `#uppercase-scheme`,
   `#record-canonical-data-at-uri`).
6. **`did:web`.** Handle syntax in lower case, ≥ 2 labels, top-level label not starting with a
   digit. → §5.2 (`safe-href.json#at-did-web-*`, `mention.json#did-web-port-rejected`).
7. **Details summary trim** uses White_Space on the last `Text`. → §4.2
   (`spoiler-details.json#details-summary-trailing-nbsp`).
8. **Math pre-scan tokenisation** (`\\href` is `\\` + letters). → §7.1, K3
   (`math.json#double-backslash-*`). *Extended at M4:* `\label`, `\eqref` and `\ref` are
   disabled (ADR 0002; `math.json#disabled-label-eqref-ref`).
9. **NUL** becomes U+FFFD, in the source (step 0) and in payloads from character references
   (step 3). → §3 steps 0 and 3 (`normalise.json#nul-replaced`,
   `#nul-in-code-and-link-destination`, `#character-reference-cr-and-nul`).
10. **P removes controls** (non-White_Space `Cc`, bidi controls) before NFC. → §10.2
    (`projection.json#control-chars-removed`, `#terminal-escapes-removed`).

### Presentation (`RENDERER_VERSION` layer)

11. **Truncation notice** carries `dir="auto"`. → §8.1 Truncation row.
12. **`anchors`** are the ids the HTML writer emits (after the limit stage and the budget).
    → §1.4, §1.5, §12, §14 22.
13. **Limit-stage costs.** A dropped link costs 0; images are leaves; an empty `flat()` emits
    nothing. → §8.4 (`limits.json#link-dropped-at-boundary`, `#leaf-exemption`,
    `#flattened-empty-emits-nothing`).
14. **Real host.** The test runs on the displayed text (Default_Ignorable removed, NFKC,
    U+3002/U+FF61, names of any script); the `ext-image` text is its alt. → §5.5, §14 30
    (`links.json#host-display-*`).
15. **Gallery items with odd CIDs** render as `span.media-missing`. → §6.3.
16. **MathML normaliser.** *Changed at M4 (ADR 0002):* math-core 0.9's closed set of `style`
    values becomes `mc-*` classes (styled by `crates/markdown/assets/mathml.css` after
    `mathmlfixes.css`) and `color` becomes `mathcolor`; its text-style wrappers inside `mtext`
    are unwrapped into classes of the `mtext`; unknown styles are dropped. → §7.3, §8.2, K3
    (`math.json#normalise-*`, `xss/mathml.json#math-color-*`, `mathml/math-core-0.9.json#norm-*`).
17. **Engine guard** (nesting and unmatched-delimiter limits) and, *added at M4*, the engine
    budget (`len << unmatched`, 2^22 per document). → §7.2, K3
    (`math.json#engine-nesting-guard`, `#guard-*`, `#engine-document-budget`).
18. **`RENDERER_VERSION`** is 3 at M4 (2 before the normaliser landed); Temml builds set bit 31.
    → §1.2. The M4 reconciliation changed presentation again (decisions 2, 27 and bugs 1, 3, 4)
    and the normaliser of decision 16 changed it once more, so the version was bumped with them
    in the same commit, as §1.2 requires.
19. **`Rendered.error`.** → §1.3, §1.4.
20. **Escaping record strings** (CR, CRLF → LF, NUL → U+FFFD in attribute values). → §8.3.
21. **`StyledDoc`.** Shared summary ids, empty item `Para`, single-line contexts, code `lang`,
    external-image placeholders, Name context for `media.name`. → §9.3
    (`styled.json#single-line-contexts`, `#ext-image-in-link-and-standalone`,
    `blocks.json#list-item-empty-styled`).
22. **Printer.** Hidden-spoiler line, host suffix once, SGR/OSC 8 per run, gutters. → §9.2,
    §9.4 (`terminal/escapes.json#osc8-*`, `#footnote-in-spoiler-*`).
23. **`TermText` JSON.** → §9.1.
24. **Hover previews.** `preview_eligible(uri, NS)`; every record link carries `data-at-uri`,
    the client derives eligibility. → §5.2, §5.3 (`at-links.json#every-collection-gets-data-at-uri`).
25. **`alt`.** `media.alt` only on the first rendered reference with an empty markdown alt (an
    image inside another image's alt is never rendered and takes none); every `alt` trimmed.
    → §6.2, §8.3, §14 25 (`images.json#cid-alt-fallback-*`, `#alt-trimmed`,
    `#nested-cid-image-takes-no-alt-fallback`, `#media-alt-amplification-*`).
26. **Footnote definitions inside a spoiler** stay hidden in the footnotes section; base depth 4.
    → §8.1, §8.4, §9.3, §14 14 (`footnotes.json#definition-inside-spoiler`,
    `limits.json#footnote-in-spoiler-base-depth`, `terminal/escapes.json#footnote-in-spoiler-*`).
27. **`flat()`.** *Extended at M4:* keeps images and footnote references (leaves) and a
    figure's caption; a spoiler contributes only `Spoiler`. → §8.4, §14 27
    (`limits.json#spoiler-flattened-to-summary`, `#figure-at-depth-limit`,
    `#flattened-keeps-footnote-reference-and-image`).
28. **Output budget** (48 MiB estimate after the limit stage; presentation). → §1.5, §8.4
    "Output budget", §14 28. The MathML document budget → §7.3 "Limits", K3
    (`math.json#mathml-document-budget`).
29. **MIME types** compared exactly. → §6.2 (`images.json#mime-exact-match`).
30. **Code language** split on ASCII whitespace. → §8.1 CodeBlock row
    (`blocks.json#code-language-ascii-whitespace-only`).
31. **Printer bounds** (column width, padding cap, 32-column gutters, OSC 8 budget). → §9.4
    (`terminal/escapes.json#table-*`, `#osc8-*`, `#deep-gutters-elided`).
32. **`derive_metadata`.** → §1.3, §1.6, §12.
33. **Sanitiser configurations.** → §8.5, §14 31 (`xss/html.json#*-safe-for-xml-patterns`).
34. **Determinism** (no hash maps keyed by author strings; pinned Unicode data; std lowercasing
    bounded by the pinned General_Category table). → §1.5 "Determinism", §2 "Unicode data".
    Residual: pulldown-cmark's own label `HashMap`s (§1.5).

## Spec bugs and gaps

1. **§4.1 trailing period never linked.** *Fixed:* the first check runs on the domain without
   trailing `.`/`_` (`d1′`), the 253 cap on the whole run. → §4.1, §14 3
   (`autolink.json#trailing-punctuation`, `#trailing-dots-and-underscores`,
   `#sentence-final-period`).
2. **NUL.** → §3 steps 0 and 3 (decision 9).
3. **pulldown-cmark CR quirks.** *Fixed* by normalising line endings and NUL in the source
   before parsing (step 0). → §3 step 0, §14 2 (`normalise.json#crlf-in-inline-code`,
   `#cr-in-indented-code`, `#source-cr-in-code-span-and-quote`).
4. **Flattening revealed spoilers and dropped captions.** → §8.4 `flat()` (decision 27).
5. **MathML filter vs math-core 0.9.** *Fixed at M4* by the normaliser. → §7.3, K3 (decision 16).
6. **Engine recursion.** → §7.2, K3 (decision 17).
7. **`anchors` source, notice `dir`.** → §1.5, §12, §8.1 (decisions 11, 12).
8. **ASCII-only host-like test.** → §5.5 (decision 14).
9. **Controls in P.** → §10.2 (decision 10).
10. **Media in unrendered footnotes.** *Fixed* with decision 2: `media_used` covers only the
    rendered content, so such media appears in the gallery. → §6.3
    (`gallery.json#image-only-in-unreferenced-footnote`).
11. **Footnote reference inside a link** is unreachable with pulldown-cmark 0.13.4. → §4.5
    (`footnotes.json#reference-in-link-text-prevents-link`).
12. **Figures in tight lists.** → §4.3 (`figure.json#figure-in-list`, `#figure-in-loose-list`).
13. **`Gutter::Footnote(N)` first block.** → §9.2.
14. **§11 react anchors.** → §11 (the range format is `social.md` §9.2's).
15. **§13 format and tooling.** → §13 "Format" and "Runners". `docs/spec/README.md` §5.3 still
    names `cargo xtask vectors --bless` and the testkit runner; that file's owner should align
    it.
16. **Dependencies.** `unicase` and `unicode-general-category` are exact pins in the root
    `[workspace.dependencies]`; `unicode-segmentation`, `unicode-normalization` and
    `unicode-width` are exact pins in `crates/markdown/Cargo.toml`. → §2 "Unicode data".
    Residuals (workspace decisions, not spec bugs): `rust-toolchain.toml` floats on `stable`
    (decision 34 bounds the effect); `lessdugin-atp` brings its crypto dependencies along.
17. **§9.1 VT and FF.** Escaped in Prose and Code, spaces in Name. → §9.1
    (`term-sanitize.json#prose-vt-ff-escaped`, `#code-vt-ff-escaped`, `#name-vt-ff-spaces`).
18. **DOMPurify trimming, `SAFE_FOR_XML`, jsdom serialisation.** → §8.3, §8.5 (decisions 25, 33).
19. **Engine time.** *Fixed at M4.* The guard bounds one expression (the worst 4,096 bytes
    measured take about 45 ms natively), but not their sum: a 1 MB body of 243 such expressions
    took 7.8 s. The engine budget bounds the sum (that body now takes 0.17 s, no longer than a
    balanced one), so math-core needs no patch. → §7.2, K3 (decision 17,
    `math.json#engine-document-budget`). A worker time limit for the poison-pill quarantine
    (§8.5) is left to that workflow and the SPA owner.
20. **Search snippets** come from P_x. → §12 "Search".
21. **`RenderOpts` vs `anchors`/`math_errors`.** → §1.3, §1.6 (decision 32).
22. **Hosts the URL parser rejects.** *Fixed at M4, found by fuzzing* (`fuzz/`, target `render`).
    The §5.1 grammar accepted ASCII hosts that the WHATWG URL parser rejects (`256.1.1.1`,
    `a.123`, `[1.2.3.4]`, malformed Punycode such as `xn--exmple-cuattps2`) or rewrites to
    another address (`0x7f.1`, `1.2.3`, `2130706433`). Such a link opens nothing, or opens a host
    the reader was not shown, and ammonia stripped its `href`, so `ammonia(html) == html` failed.
    `host_ok` limits hosts to what WHATWG keeps unchanged: dotted-quad IPv4, valid IPv6, and
    `xn--` labels that are well-formed Punycode (decoded only to be checked, never for display).
    Checked differentially against `url` 2.5.8 on 12 million random hosts. → §5.1, decision 8
    (`links.json#punycode-invalid-dropped`, `#punycode-valid-kept-undecoded`,
    `#ipv4-dotted-quad-only`, `#ipv6-must-parse`, `safe-href.json#ipv4-*`, `#punycode-*`,
    `#ipv6-not-an-address`).
