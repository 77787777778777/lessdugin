<!-- SPDX-License-Identifier: CC0-1.0 -->

# LFM/1 implementation notes (M1)

Notes from implementing `docs/spec/lfm-1.md` in `crates/markdown` and writing these vectors.
They record where the spec was ambiguous (and what the implementation and vectors chose), and
spec bugs found on the way. Section numbers refer to `lfm-1.md`.

## Decisions where the spec was ambiguous

### Semantics (`lfm/1` layer)

1. **Rendered footnotes (§4.6).** "A definition that is never referenced" is read as: not
   reachable from a body reference. The rendered set is the fixed point "first definition of a
   label referenced from the body or from a rendered definition"; references inside
   unrendered or duplicate definitions do not count, get no ordinal and no `K`. So
   `[^u]: u[^v]` with `u` unreferenced renders neither `u` nor `v`
   (`footnotes.json#reference-only-from-unrendered-definition`). Nested definitions are
   extracted as definitions of their own.
2. **Derived metadata scope (§1.5, §6.3, §12).** `links` and `mentions` are collected from
   what is rendered: SL (after the limit stage and the output budget) and then the rendered
   footnote definitions. A link in an unreferenced footnote definition or in a subtree the limit
   stage flattened is invisible, so it must not notify anyone or count as a pingback
   (`at-links.json#unreferenced-footnote-not-derived`, `#depth-flattened-not-derived`).
   `media_used` stays on S6, as §1.5 says (bug 10).
3. **`mailto` (§5.1).** The address part (before the first `?`), percent-decoded, must be
   exactly one address: a non-empty local part without `@ , ; < >`, one `@`, and a domain under
   the `http` host grammar (ASCII `[a-z0-9._-]+`, not only dots). The query may not add
   recipients: no `to`, `cc` or `bcc` field (names percent-decoded, ASCII case-insensitive).
   Anything else is dropped. The host shown next to the link (§5.5, §9.4) is then the only domain
   the mail goes to; "the part after the last `@`" would hide `evil.com` in
   `mailto:a@evil.com?cc=b@google.com`, and an empty address (`mailto:?to=…`) would show no host
   at all (`links.json#mailto-*`, `safe-href.json#mailto-*`).
4. **Malformed `cid:` (§6.1).** A `cid:` URL whose CID fails `^b[a-z2-7]{1,127}$` is a miss.
5. **Record NSIDs (§5.2).** A syntactically valid NSID whose authority has upper-case letters
   is rejected (the link is dropped): `Record` carries a canonical `AtUri`, whose authority is
   lower-case (`lessdugin-atp`).
6. **`did:web` hostname (§5.2).** "Lowercase hostname" is handle syntax in lower case: at
   least two labels, top-level label not starting with a digit (`Did::check_lessdugin_policy`).
7. **Details summary trim (§4.2).** "Trailing White_Space trimmed from the last Text" applies
   when the last summary event is a `Text`; `trim_start` and the trim use Unicode White_Space.
8. **Math pre-scan tokenisation (§7.1).** One left-to-right scan: `\` + letters is a control
   word; `\` + any other character (including `\{`, `\}`, `\\`) is a two-character control
   symbol. So `\\href` is `\\` followed by letters, not `\href` (`math.json#double-backslash-…`).
9. **NUL (§3 step 3).** U+0000 becomes U+FFFD in every text payload, as CommonMark 0.31.2 §2.3
   requires; pulldown-cmark 0.13.4 passes NUL through (see bug 2).
10. **P removes controls (§10.2).** Before NFC, P drops every `Cc` code point that is not
    White_Space (C0 other than TAB/LF/VT/FF/CR, DEL, C1 other than NEL) and the bidi controls
    (U+061C, U+200E/F, U+202A–U+202E, U+2066–U+2069); White_Space is then collapsed as before.
    P, P_x and the excerpt (published as `post.excerpt`) can then never carry a terminal escape
    (`projection.json#control-chars-removed`, `#terminal-escapes-removed`). Quote normalisation
    (§11) removes the same set, so quotes keep matching P. Tested invariants: no whitespace
    other than U+0020 and none of these characters in P or the excerpt.

### Presentation (`RENDERER_VERSION` layer)

11. **Truncation notice (§8.1).** Written `<p class="notice" dir="auto">`: the Direction rule
    gives every `p` `dir="auto"`, although the §8.1 row shows the notice without it.
12. **Anchors (§12 vs §1.5).** `anchors` lists the ids actually emitted in the HTML, i.e. after
    the limit stage (a flattened heading has no id), as §12 says; §1.5 lists S6 as the source.
13. **Limit-stage costs (§8.4).** A link dropped by the URL policy opens no element, so it
    costs 0 and is never dropped; images are leaves (their alt events are not depth-checked);
    `flat()` that yields nothing emits nothing (no empty paragraph).
14. **Real host (§5.5).** For `ext-image` the visible text used by the host test is the alt
    (`plain(inner)`), not `"image: …"` (which is never host-like). The test runs on the text as
    it is displayed: Default_Ignorable code points removed (U+200B, U+00AD, U+2060, U+FEFF, …),
    NFKC (fullwidth forms), U+3002/U+FF61 mapped to `.`, ASCII-lowercased. The text is host-like
    when it starts with a dotted name of any script (two runs of letters, marks, numbers, `_` or
    `-` joined by `.`, whatever follows), so `google.com!`, `ɡoogle.com`, Cyrillic `gооgle.com`
    and `google.com&#8203;` all get the span. The span is left out only when the text starts
    with exactly the link's host (one `www.` stripped from both, trailing dots ignored) followed
    by something other than a name character or dot (`links.json#host-display-*`).
15. **Gallery items with odd CIDs (§6.3).** A `media[i]` whose CID does not match the
    `data-cid` grammar renders as `span.media-missing` with its alt.
16. **MathML filter (§7.3).** Besides `a` and `span`, the HTML text-style wrappers math-core 0.9
    emits inside `mtext` (`b i em strong code s u`) are unwrapped (see bug 5). Token elements
    (`mi mn mo ms mtext`) keep text only, `mspace`/`none`/`mprescripts` are emitted empty, text
    directly inside other elements is dropped, `math`'s own `display` comes only from the
    event kind, and an unknown character entity in the engine output is a math error.
17. **Engine guard (§7.2).** math-core 0.9 parses recursively without a depth limit; argument
    chains such as `\sqrt\sqrt…x` or `\frac a\frac a…` overflow a 1 MiB (wasm) stack within
    4,096 bytes. The engine therefore refuses expressions whose estimated parser nesting (groups,
    `\left`, `\begin`, scripts and the argument counts of math-core 0.9's commands) exceeds 96;
    a refusal is an engine error, i.e. a math error. Anything that deep would exceed the
    64-level MathML cap anyway (`math.json#engine-nesting-guard`; stack test in
    `tests/props.rs`).

    math-core 0.9's operator inference (`semantic::enrich_formula`) also takes time exponential
    in the number of unmatched opening delimiters: `$((((…(x$` with 28 `(` took 9.6 s, and each
    extra `(` doubles it (bug 19). The guard therefore also simulates delimiter matching and
    refuses more than 8 unmatched opening delimiters per expression (a blow-up of at most 2^8)
    and more than 64 open delimiters within one row. The count is an upper bound by
    construction: every token math-core 0.9 can turn into an opening delimiter counts (`(`, `[`,
    `\{`, `\lbrace`, `\langle`, `\lAngle`, `\lvert`, `\lVert`, `\lceil`, `\lfloor`, `\lgroup`,
    `\llbracket`, `\lmoustache`, `\llcorner`, `\ulcorner`, `\bigl…`, `\mathopen`'s argument, the
    Unicode opening brackets and every other General_Category Ps character), also as a command
    argument or script and inside a group; a closing delimiter (including `!`, which math-core
    0.9 lexes as one) cancels an opening one only when both sit directly in the same row and no
    row break (`&`, `\\`, `\newline`, `\over`, …) lies between them. Balanced formulas,
    `\left…\right` and the half-open intervals of real mathematics are unaffected
    (`math.json#guard-*`; timing test in `tests/props.rs`).
18. **`RENDERER_VERSION`.** `2` (the presentation changes of decisions 17 and 25–31); builds
    without the `math` feature (Temml mode, §7.5) set bit 31, so the mode is part of the cache
    key.
19. **`Rendered.error`.** `Option<BodyError>` added next to the §1.4 fields, so callers can tell a
    body error from content.
20. **Escaping record strings.** The HTML escaper also maps CRLF/CR to LF and NUL to U+FFFD.
    Content never carries them after pass 0, but `media.alt`/`media.name` may, and the HTML
    parser would rewrite them, breaking `sanitize(html) == html`.
21. **`StyledDoc` (§9.3).** One id counter is shared by spoiler and details summaries; an empty
    list item emits an empty `Para`; display math inside a heading, summary or table cell is an
    inline `$$tex$$` `MATH` span; a hard break in those single-line contexts is a space; soft
    breaks next to display math do not leave stray spaces; leading and trailing empty TeX lines
    are dropped; the code-block `lang` uses the HTML class rule; an external image inside a link
    is `[image: A]` carrying the outer link (no image host, as in HTML), a standalone one is
    `[image: A ⟨host⟩]` (`[image ⟨host⟩]` without alt); `media.name` uses the Name context.
22. **Printer (§9.4).** For plain stdout a hidden spoiler prints one `[spoiler hidden]` line in
    place of its summary and content; the F1 ` ⟨host⟩` suffix is not repeated when the link text
    already shows exactly that host (the standalone external-image placeholder); a link split
    into several styled spans gets one suffix. SGR and OSC 8 open once per run of equally styled
    text on an output line and close at its end, never per word or grapheme
    (`terminal/escapes.json#osc8-once-per-line`, `#osc8-and-sgr-per-line`). Gutters print as
    `│ ` (quote, alert), `┆ ` (spoiler), two spaces (details), `• `/`N. ` (items) and `[N] `
    (first block of a footnote).
23. **`TermText` JSON.** A plain string when every run is plain, else
    `[{"plain": …} | {"escaped": …}, …]`.
24. **Hover previews (§5.3).** `preview_eligible(uri, ns)` takes the build's `NS` (the markdown
    crate does not depend on the lexicon crate).
25. **`alt` (§6.2, §8.3).** `media.alt` (up to 20,000 bytes in the lexicon) is the fallback alt
    only for the first reference to each item whose markdown alt is empty, in render order (the
    body, then the rendered footnotes); later references get `alt=""`. Otherwise
    `![](cid:x) ` repeated over a 100 KB body wrote 176 MB of HTML and 265 MB of terminal output
    (`images.json#cid-alt-fallback-*`, `#media-alt-amplification-*`,
    `terminal/escapes.json#media-alt-amplification-*`). Every `alt` (markdown alt, `media.alt`,
    gallery) is trimmed of White_Space and U+FEFF, a superset of ECMAScript `String.prototype.trim`,
    which DOMPurify applies to attribute values; untrimmed, `sanitize(html) == html` failed
    (`images.json#alt-trimmed`, `xss/html.json#alt-whitespace-trimmed`).
26. **Footnote definitions inside a spoiler (§8.1, §9.3).** A rendered definition written inside
    a spoiler keeps its place in the footnotes section, but its content is wrapped in a
    `details.spoiler` of its own (before the back-links), and in the `StyledDoc` it starts with a
    spoiler `Summary` whose blocks carry a `Spoiler` gutter, so neither the HTML nor the printer
    reveals it (`footnotes.json#definition-inside-spoiler`,
    `terminal/escapes.json#footnote-in-spoiler-*`). Its limit-stage base depth is one more.
27. **`flat(Spoiler)` (§8.4).** A spoiler deeper than the limit flattens to the fixed summary
    text `Spoiler` only (its content is dropped), instead of an open paragraph
    (`limits.json#spoiler-flattened-to-summary`; bug 4).
28. **Output budget (presentation).** Some constructs expand the input many times (escaping,
    fixed markup around short syntax, record fields). After the limit stage, an upper-bound
    estimate of the HTML of each SL event is summed over the body and then the rendered
    footnotes (the gallery reserved first); past 48 MiB the flow is cut there, every open tag is
    closed, the remaining footnotes are dropped and `flags.truncated` is set, as when the event
    cap truncates (§3 step 1). The HTML writer and the `StyledDoc` builder consume the same cut
    events. Real documents stay far below it (a 1 MB body of plain prose is estimated at about
    6 MB); unit tests check that the estimate bounds the HTML and that cuts stay balanced.
    MathML has its own budget: once the expressions of a document have produced 16 MiB of
    MathML, further expressions render as math errors (`math.json#mathml-document-budget`).
29. **MIME types (§6.2).** The declared `mimeType` is compared exactly as written, like the §6.2
    table: `IMAGE/PNG`, `image/png; x=y` or ` image/png` are files, not images
    (`images.json#mime-exact-match`).
30. **Code language (§8.1).** The first word of the info string is split on ASCII whitespace
    (as HTML splits `class`), so `a\u00a0b` is one (invalid) token and gets no class
    (`blocks.json#code-language-ascii-whitespace-only`).
31. **Printer bounds (§9.4).** Table columns are at most 40 columns wide (or the terminal
    width); longer cells wrap within the column, and when padding would still exceed 8 × the
    content + 4,096 bytes the rows print unaligned. Gutters print at most 32 columns: deeper
    blocks show `… ` and their innermost gutters. OSC 8 wrappers stop after 1 MiB of OSC 8 bytes
    per document (the text still prints). Before, a 100 KB table printed 400 MB and a 98 KB link
    99–198 MB (`terminal/escapes.json#table-*`, `#osc8-*`, `#deep-gutters-elided`).
32. **`derive_metadata`.** `render` always runs the HTML writer (it yields `anchors` and
    `flags.math_errors`), and so the math engine. `derive_metadata(src, media)` returns P, P_x,
    the excerpt, ToC, counts, `links`, `mentions` and `media_used` without the HTML writer,
    the `StyledDoc` builder or the math engine; each field equals `render`'s (checked over every
    render vector and by a property test). Indexers and the mention and pingback pipelines
    should use it, and search snippets should come from its `text_x` (P_x) so they never reveal
    a spoiler (bug 20).
33. **Sanitiser configurations (§8.5).** `allowlist::dompurify_config_json()` is complete:
    `ALLOWED_TAGS`/`ALLOWED_ATTR` from the allowlist, `ALLOW_ARIA_ATTR: false`,
    `ALLOW_DATA_ATTR: false`, `ADD_URI_SAFE_ATTR` (every allowlisted attribute but `href`, so
    DOMPurify does not drop `dir`, `rel`, `id`, …) and `SAFE_FOR_XML: false` (its pattern
    removes attributes containing `-->`, `]]>` or `</title`, which `alt` legitimately may; the
    renderer escapes `<` and `>` in attributes, so the mXSS it guards against cannot arise:
    `xss/html.json#*-safe-for-xml-patterns`). The `ammonia` check allows relative URLs only with
    the `#/at/` and `#/u/` prefixes the renderer emits.
34. **Determinism.** No `HashMap`/`HashSet` (std `RandomState` is fixed on wasm32, a HashDoS
    route on footnote labels and slugs): footnote labels, slugs and `media_used` use
    `BTreeMap`/`BTreeSet`. The Unicode crates are exact pins. Slugs and anchors lowercase with
    std, then keep only letters and numbers of the pinned General_Category table (Unicode
    16.0), so a code point assigned later is dropped whatever std maps it to; a unit test
    requires a toolchain whose Unicode version is not older than that table. pulldown-cmark
    0.13.4 itself still keeps link reference and footnote definitions in std `HashMap`s keyed by
    labels from the input; on wasm32 their keys are predictable, so colliding labels are a
    residual (unverified) slow path that only an upstream change or a fork can close.

## Spec bugs and gaps

1. **§4.1 trailing period never links.** `valid_domain(t[d0..d1])` runs on the domain run
   *including* a trailing `.`, so "Visit www.example.com." does not autolink at all, unlike
   GFM. The second check `valid_domain(t[d0..min(d1, e)])` is then dead code: once the first
   check passes the domain ends in `[A-Za-z0-9-]`, which `trim_trailing` never removes.
   Implemented literally (`autolink.json#trailing-punctuation`). Suggested fix, still linear:
   make the first check `d1 − d0 ≤ 253 and valid_domain(t[d0..d1′])` with `d1′` = `d1` minus
   trailing `.`/`_`, and keep the second.
2. **§2/§3 NUL.** pulldown-cmark 0.13.4 does not replace U+0000 (CommonMark §2.3 requires it).
   The implementation does it in pass 0 step 3 (decision 9); the spec should say so.
3. **§2 pulldown CR quirks, frozen by "as implemented".** CRLF inside a code span becomes two
   spaces (`normalise.json#crlf-in-inline-code`), and lone-CR line endings in indented code
   keep the continuation lines' indentation (`#cr-in-indented-code`). Normalising line endings
   in the source before parsing would fix both (an `lfm/2` change once frozen).
4. **§8.4 flattening reveals spoilers.** `flat()` of a spoiler deeper than the limit put its
   content in a plain paragraph; P_x and excerpts were unaffected. Fixed as `flat(Spoiler)` =
   the fixed summary only (decision 27); the spec should say so. `flat()` also drops a figure's
   caption, which P keeps.
5. **§7.3 MathML filter vs math-core 0.9.** The spec says math-core emits only `a` and `span`
   inside `mtext`; 0.9 also emits `b i em code s u` (and `span class/style`) for `\textbf`,
   `\texttt`, `\emph`, … Read literally, the filter would delete their text (decision 16).
6. **§7.2 engine recursion.** Unbounded recursion in math-core is a crash vector (decision 17);
   the spec should either define a nesting limit or require one from the engine (S10).
7. **§1.5 vs §12 `anchors`** disagree on the source (decision 12); **§8.1 notice `dir`**
   disagrees with the Direction rule (decision 11).
8. **§5.5 host-like test.** The spec's test is ASCII-only, so Cyrillic `gооgle.com`, `ɡoogle.com`,
   `google.com&#8203;` or `google.com!` got no `link-host` span. Implemented as decision 14; the
   spec should adopt it.
9. **§10.2 controls in P.** P kept non-whitespace C0/C1 and bidi controls, which reached
   `post.excerpt` and search indexes. Implemented as decision 10 (an `lfm/1` semantic, so it
   must land in the spec before the freeze).
10. **§6.3 media in unrendered footnotes.** With `media_used` over S6 (decision 2), an image
    referenced only from an unreferenced footnote is neither shown inline nor in the gallery.
11. **§8.1 footnote reference inside a link** is unreachable with pulldown-cmark 0.13.4: a GFM
    footnote reference in link text prevents the link (`footnotes.json#reference-in-link-text-
    prevents-link`). The writer still implements the anchor-free form.
12. **§4.3 figures in tight lists.** Tight list items have no `Paragraph` events, so a lone
    titled image in a tight item is never a figure (`figure.json#figure-in-list`).
13. **§9.2 `Gutter::Footnote(N)`** has no `first` flag; printers must detect a footnote's first
    block themselves.
14. **§11 react anchors.** `anchor = range` refers to a range format defined elsewhere
    (`social.md`); only `qkey` is implemented here.
15. **§13 format and tooling.** §13 describes each file as a bare JSON array and blessing via
    `xtask vectors --bless`; the repository uses the object format of `testdata/vectors/README.md`
    (as `keys/` does) and `LESSDUGIN_BLESS=1` with the Rust runner.
16. **Dependencies.** The workspace pins `math-core = "=0.9.0"`. The Unicode crates
    (`unicode-segmentation`, `unicode-normalization`, `unicode-width`, `unicase`,
    `unicode-general-category`) are exact pins in `crates/markdown/Cargo.toml`, since their data
    is part of the frozen semantics; the unused `unicode-security` was removed.
    `rust-toolchain.toml` still floats on `stable` (decision 34 bounds what that can change);
    pinning it is a workspace decision. `lessdugin-atp` brings its crypto dependencies (k256,
    p256, cid, …) into the markdown crate; a syntax-only feature there would avoid them.
17. **§9.1 Prose row.** "The rest are kept" includes VT and FF, which contradicts the `TermText`
    invariant (no C0 but TAB and LF in plain runs). The implementation escapes them; the spec
    should say so.
18. **§8.3/§8.5 DOMPurify.** DOMPurify trims attribute values (ECMAScript whitespace and
    U+FEFF) and, with its default `SAFE_FOR_XML`, removes attributes matching
    `/((--!?|])>)|<\/(style|script|title|xmp|textarea|noscript|iframe|noembed|noframes)/i`;
    jsdom also serialises `<`/`>` in attributes unescaped. The spec should record the alt
    trimming (decision 25), `SAFE_FOR_XML: false` (decision 33) and the jsdom attribute shim,
    and CI should run DOMPurify over every vector (`web/src/lib/sanitize.vectors.test.ts`).
19. **§7.2 engine time.** math-core 0.9.0 is exponential in unmatched opening delimiters (a
    failed `enrich_bracketed` re-enriches the rest of the row, T(n) = 2·T(n−1)). The guard of
    decision 17 bounds it; the proper fix is upstream (memoise the failed attempt or reuse its
    inner result) or a patched math-core, which needs a workspace `[patch]` and an ADR. §8.5's
    poison-pill quarantine only covers crashes: the SPA worker should also be terminated and the
    body quarantined after a time limit.
20. **§12 search snippets.** Snippets built from P reveal spoilers; they should come from P_x
    (`derive_metadata(..).text_x`, decision 32).
21. **§1.3 `RenderOpts`.** Options cannot skip the HTML writer while `anchors` and
    `flags.math_errors` are part of every `Rendered`; the spec should allow those fields to be
    absent when HTML is not computed, or name `derive_metadata` (decision 32) as the
    math-free path.
