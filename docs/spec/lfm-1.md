<!-- SPDX-License-Identifier: CC0-1.0 -->

# LFM/1: lessdugin flavoured markdown (`lfm/1`)

| | |
|---|---|
| Status | Normative. Frozen at the first release that writes the final `NS` (M7). Before that it changes only together with its vectors. |
| Behaviour ID | `lfm/1`. It is also the `profile` knownValue of `NS.markdown` (`data-model.md` §5). |
| Implementation | `crates/markdown` (`lessdugin-markdown`, wasm-safe). It is the only implementation: CLI, TUI, SPA (wasm) and indexer call the same code, and their outputs are byte-identical. |
| Vectors | `testdata/vectors/lfm1/` (§13), plus `terminal/`, `xss/` and `mathml/` |
| Derived from | synthesis §2.7, §5.2 (F1, F5–F9, F16), §5.3, §6.8, §10 (C11); completeness review (pingbacks, streaming blobs) |

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

`NS` is the build-time namespace constant: `online.7778777.lessdugin`, or `online.7778777.lessdugin.temp` in development builds. "Our collections" always means the collections under the build's `NS`.

---

## 1. Scope, layers, inputs and outputs

### 1.1 What this document defines

LFM/1 is CommonMark plus a fixed set of extensions, parsed by one pinned parser, then rewritten by ordered post-passes. The result is projected into four outputs:

1. **HTML+MathML** for the SPA and for `read --html`.
2. **`StyledDoc`** made of `TermText` spans, for the TUI and the CLI.
3. **The plain-text projection P**. Quote anchors, excerpts, word counts and local search all use it.
4. **Derived metadata**: table of contents, links, mentions, used media and anchors.

### 1.2 Two layers with different change rules

| Layer | Covers | Change rule |
|---|---|---|
| **`lfm/1` semantics** | The parser profile (§2). Pass 0 and post-passes 1–6 (§3, §4), including the set of rendered footnote definitions (§4.6). `safe_href` and the URL policy (§5). `cid:` resolution and `media_used` (§6.1, §6.3). Math acceptance: delimiters, pre-scan and disabled macros (§7.1). P, P_x, the excerpt, word count, read time and ToC (§10). Quote resolution (§11). Derived links and mentions (§12). | Never changes. Any change requires `lfm/2`. |
| **Presentation**, versioned by `RENDERER_VERSION` | Exact HTML serialisation, classes and fixed strings (§8). The limit stage and the output budget (§8.4). MathML filtering details (§7.3). The `StyledDoc` shape and printer styling (§9). `anchors` and `flags.math_errors`, which the HTML writer yields. | May change in any release, provided all three of these happen in the same commit: bump `RENDERER_VERSION` (a `u32` constant, starting at 1, that is part of the render-cache key `(cid, RENDERER_VERSION)`; builds in Temml mode, §7.5, set its bit 31, so the mode is part of the key); re-bless the vectors; regenerate the DOMPurify and ammonia configurations from §8.2. Adding anything to the §8.2 allowlist needs a security review. |

No `lfm/1` value depends on a presentation stage: the limit stage, the output budget, the math engine and the writers never change P, the ToC, `links`, `mentions` or `media_used`.

### 1.3 Input

`render(src: &str, media: &[defs#media], opts: &RenderOpts) -> Rendered`

**Where the source comes from.** The caller takes the source from an `NS.markdown` object (`data-model.md` §5):
- `text`: used as is.
- `textBlob`: the caller fetches it with the streaming, CID-checked `Host::fetch_blob`, then:
  - decodes it as UTF-8 whatever the declared charset says;
  - strips one leading U+FEFF;
  - on invalid UTF-8, returns `BodyError::Encoding` without calling `render`.
- While the blob is unavailable, the body is **pending**. Pending is not empty: quote resolution returns `Pending` (§11), not `Orphaned`.

**Profile.** If `profile` is absent or `"lfm/1"`, render normally. Any other value is rendered as `lfm/1` with `flags.unknown_profile = true`, and clients show a badge.

**Size.** If `src` is longer than 1,000,000 bytes, it is not parsed. The result is `BodyError::TooLarge`. (Lexicon limits: inline text ≤ 100,000 bytes, `textBlob` ≤ 1,000,000 bytes.)

**On a `BodyError`,** `Rendered` has:
- `error` = that `BodyError`;
- `html` = the root element (§8.1) containing a single `p.notice` with the matching fixed string (§8.6);
- `styled` = a single `Notice` block with the same string;
- `text` = `""`;
- every list empty, and every flag false except `unknown_profile`.

**`RenderOpts`** only selects which outputs to compute: any subset of `{html, styled, text}`. It never changes their content. So a rendered output depends only on `(src, media, RENDERER_VERSION)`, and the render cache key `(cid, RENDERER_VERSION)` is sound. `anchors` and `flags.math_errors` come from the HTML writer, so `render` always runs it (and with it the math engine), whatever `RenderOpts` says; callers that need neither use `derive_metadata` (§1.6).

### 1.4 Output

```
Rendered {
  html:          String              // §8; root <div class="lfm" dir="auto">
  styled:        StyledDoc           // §9
  text:          String              // P (§10.1–10.2)
  excerpt:       String              // §10.4, computed from P_x
  toc:           Vec<TocEntry{level: u8, id: String, text: String}>   // §10.6
  word_count:    u32                 // §10.5
  read_minutes:  u32                 // §10.5
  links:         Vec<AtUri>          // §12; record links, deduplicated, document order, ≤ 256
  mentions:      Vec<Did>            // §12; mention DIDs, deduplicated, document order, ≤ 128
  media_used:    Vec<u16>            // §6.3; 0-based indexes into media[], ascending
  anchors:       Vec<String>         // §12; every element id emitted in the HTML, in document order
  flags:         { truncated, depth_limited, unknown_profile: bool, math_errors: u32 }
  error:         Option<BodyError>   // §1.3; None when the body rendered
}
```

### 1.5 Pipeline

```
src ─parse (§2)─► E ─pass 0 (§3)─► S0 ─pass 1 autolinks─► S1 ─pass 2 spoiler/details─► S2 ─pass 3 figures─► S3
S3 ──► P, P_x, heading text, slugs                                        (§10; quote targets §11)
S3 ─pass 4 mentions─► S4 ─pass 5 URL policy and media─► S5 ─pass 6 ids─► S6
S6 ──► ToC (§10.6); the rendered content (the body, then the rendered footnote definitions, §4.6) ──► links, mentions (§12), media_used (§6.3)
S6 ─limit stage and output budget (§8.4)─► SL
SL ──► HTML writer (§8) ──► html, anchors, flags.math_errors        SL ──► StyledDoc builder (§9)
```

Every stage MUST be one or more forward iterations over an event vector, with explicit stacks. No code path may recurse in proportion to input nesting (F8). This covers every pass, both writers, P, and the MathML filter and serialiser. The math engine is a third-party recursive parser: §7.2 bounds the nesting of what it is given.

Every stage MUST be deterministic, on every target. Implementations MUST NOT key hash maps or sets by author-controlled strings, nor iterate them: Rust's `RandomState` has fixed keys on wasm32, so an author could make labels collide on purpose. Footnote labels, slugs and `media_used` use ordered maps and sets. Residual: pulldown-cmark 0.13.4 itself keeps link reference and footnote definitions in `HashMap`s keyed by labels from the input; only an upstream change can remove that slow path.

### 1.6 `derive_metadata`

`derive_metadata(src: &str, media: &[defs#media]) -> Metadata{text, text_x, excerpt, toc, word_count, read_minutes, links, mentions, media_used, error}` returns the `lfm/1` values of a body without running the HTML writer, the `StyledDoc` builder or the math engine. Every field equals the field of the same name of `render` (`text_x` is P_x, §10.3). Writers (§12), the indexer, and the mention and pingback pipelines SHOULD use it; it never runs the math engine, so it is safe on bodies that are not displayed.

---

## 2. Parser profile

**Crate.** `pulldown-cmark = { version = "=0.13.4", default-features = false }`. The `html`, `getopts` and `simd` features are off, because lessdugin has its own writers.

**Base syntax.** CommonMark 0.31.2, as implemented by that version, on a source whose line endings and NUL characters pass 0 has already normalised (§3 step 0).

**Invocation.** `Parser::new_ext(src′, OPTIONS)`, where `src′` is the output of §3 step 0, with no broken-link callback.

| Flag | Bit | lfm/1 | Effect |
|---|---|---|---|
| `ENABLE_TABLES` | `1<<1` | **on** | GFM tables |
| `ENABLE_FOOTNOTES` | `1<<2` | **on** | GFM-style footnotes (`[^x]` and `[^x]: …`). A reference with no definition stays literal text. Labels match case-insensitively (`UniCase`). |
| `ENABLE_STRIKETHROUGH` | `1<<3` | **on** | `~x~` and `~~x~~`. Subscript is off, so a single `~` means strikethrough. |
| `ENABLE_TASKLISTS` | `1<<4` | **on** | `- [ ]` and `- [x]`, rendered read-only |
| `ENABLE_SMART_PUNCTUATION` | `1<<5` | off | |
| `ENABLE_HEADING_ATTRIBUTES` | `1<<6` | off | `# h {#id .c}` stays literal heading text |
| `ENABLE_YAML_STYLE_METADATA_BLOCKS` | `1<<7` | off | `---` keeps its CommonMark meaning |
| `ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS` | `1<<8` | off | `+++` is literal |
| `ENABLE_OLD_FOOTNOTES` | `(1<<9)\|(1<<2)` | off (bit 9 clear) | |
| `ENABLE_MATH` | `1<<10` | **on** | `$…$` → `InlineMath`, `$$…$$` → `DisplayMath` |
| `ENABLE_GFM` | `1<<11` | **on** | Alerts `> [!NOTE\|TIP\|IMPORTANT\|WARNING\|CAUTION]`: ASCII case-insensitive, with the marker alone on the quote's first line |
| `ENABLE_DEFINITION_LIST` | `1<<12` | off | |
| `ENABLE_SUPERSCRIPT` | `1<<13` | off | `^x^` is literal |
| `ENABLE_SUBSCRIPT` | `1<<14` | off | |
| `ENABLE_WIKILINKS` | `1<<15` | off | `[[x]]` is literal |

**`OPTIONS` = `0x0000_0C1E`.** It MUST be built as the union of the six named "on" flags. `Options::all()` and `from_bits_truncate` of any other value are forbidden, so any flag a future version adds stays off.

**Upgrades.** An upgrade of pulldown-cmark MUST pass every `lfm1` vector unchanged. If it does not, the upgrade is `lfm/2`.

**Unicode data.** Unicode tables are part of the `lfm/1` semantics (P, slugs, anchors, excerpts, the host test) and of the terminal layout, so these crates are exact pins in the workspace (`Cargo.toml`), and upgrading one is reviewed like a parser upgrade:

| Crate | Pin | Used for |
|---|---|---|
| `unicode-normalization` | `=0.1.25` | NFC (P, slugs, quotes), NFKC (§5.5) |
| `unicode-segmentation` | `=1.13.3` | extended grapheme clusters (§10.4, §9.1) |
| `unicode-width` | `=0.2.2` | display width (§9.4) |
| `unicase` | `=2.10.0` | footnote label matching, the same folding pulldown-cmark uses (§3 step 5, §4.6) |
| `unicode-general-category` | `=1.1.0` (Unicode 16.0) | slugs (§4.6), anchors (§5.1), the host test (§5.5) |

Lowercasing (`str::to_lowercase`, §4.6, §5.1) and White_Space come from the Rust standard library, whose Unicode version follows the toolchain. Slugs and anchors therefore keep only the code points the pinned General_Category table classifies as letters or numbers, so a code point assigned later is dropped whatever the toolchain maps it to; a unit test requires a toolchain whose Unicode version is not older than that table.

**Events.** These are the events 0.13.4 can emit under `OPTIONS`:
- `Start`/`End` of: `Paragraph`, `Heading`, `BlockQuote(kind)`, `CodeBlock(Indented|Fenced(info))`, `HtmlBlock`, `List(start)`, `Item`, `FootnoteDefinition(label)`, `Table(aligns)`, `TableHead`, `TableRow`, `TableCell`, `Emphasis`, `Strong`, `Strikethrough`, `Link{link_type, dest_url, title, id}`, `Image{…}`;
- `Text`, `Code`, `InlineMath`, `DisplayMath`, `Html`, `InlineHtml`, `FootnoteReference(label)`, `SoftBreak`, `HardBreak`, `Rule`, `TaskListMarker(bool)`.

Pass 0 handles any other tag defensively (§3).

---

## 3. Pass 0: normalisation

Pass 0 turns the parser's event stream E into the stream S0. Its steps run in this order.

**0. Source normalisation.** Before parsing, replace `"\r\n"` in `src` with `"\n"`, then every remaining `"\r"` with `"\n"`, and every U+0000 with U+FFFD (CommonMark 0.31.2 §2.3). CommonMark treats the three line endings alike, but pulldown-cmark 0.13.4 does not: given CRLF it turns a line ending inside a code span into two spaces, and given lone CRs it keeps the indentation of continuation lines in indented code (`normalise.json#crlf-in-inline-code`, `#cr-in-indented-code`). It also passes U+0000 through.

**1. Event cap.** Pass 0 pulls at most 2^20 = 1,048,576 events from the parser. If the parser would yield more:
- stop pulling and set `flags.truncated`;
- if a raw-HTML block is open, flush its buffer (step 2);
- then emit `End` for every open tag, innermost first.

The cap counts parser events, before any mapping.

**2. Raw HTML becomes literal text.** Raw HTML is never interpreted. It is shown, escaped, as written.

| Parser event | S0 events |
|---|---|
| `Start(HtmlBlock)` … `End(HtmlBlock)` | `Start(Paragraph)`, `lines(B)`, `End(Paragraph)`. B is the concatenation of the block's `Html(t)` payloads, minus one trailing `"\n"`. |
| `InlineHtml(t)` | `lines(t)` |
| `Start`/`End` of `Superscript`, `Subscript`, `DefinitionList*` or `MetadataBlock`, and any `Link`/`Image` with `LinkType::WikiLink` (none can occur under `OPTIONS`) | The tag is dropped and its content kept. A dropped `Link`/`Image` keeps its inner events. |

`lines(s)` splits `s` (after step 3) on `"\n"`. It emits `SoftBreak` between consecutive pieces, and `Literal(piece)` for each non-empty piece.

`Literal` is an LFM-internal event:
- It renders exactly like `Text`.
- No post-pass ever examines it: no autolinks, no markers.

**3. Line endings.** In every text payload, replace `"\r\n"` with `"\n"`, then every remaining `"\r"` with `"\n"`, and every U+0000 with U+FFFD. Text payloads are `Text`, `Literal`, `Code`, `InlineMath`, `DisplayMath`, raw HTML, link and image `dest_url` and `title`, footnote labels, and the code-block info string. After step 0 only character references can still produce these characters (`&#13;`).

**4. Coalescing.** Merge every maximal run of adjacent `Text` events into one `Text`, and every maximal run of adjacent `Literal` events into one `Literal`. `Text` and `Literal` are never merged with each other.

**5. Dangling footnote references.** After truncation, a reference's definition may be missing. A `FootnoteReference(l)` that matches no `FootnoteDefinition` in S0 (matching by `UniCase`) is replaced by `Text("[^" + l + "]")`, and step 4 is reapplied.

---

## 4. Post-passes

Passes 1–6 run in order, each over the output of the one before it.

The passes add these internal events:
- `Start`/`End` of `Spoiler`, `Details`, `Summary` and `Figure{caption}`;
- an annotation on every `Link` and `Image`: its source `link_type`, `ExtAutolink`, and after pass 5 its resolution.

### 4.1 Pass 1: GFM extended autolinks

**Scope.** Every `Text` event that is not inside a `Link`, an `Image` or a `CodeBlock`. `Code`, `Literal` and math are never scanned. Email and xmpp autolinks are not recognised. (`<a@b.c>` CommonMark autolinks are handled in §4.5.)

```
autolink(t):                                   // t = one Text payload; indexes are code-point positions
  out = []; last = 0; i = 0
  while i < len(t):
    if boundary(t, i) and (P = prefix_at(t, i)) ≠ none:     // ASCII case-insensitive: "www." | "http://" | "https://"
      d0 = (P == "www.") ? i : i + len(P)                  // a www domain includes the "www."
      d1 = d0; while d1 < len(t) and t[d1] ∈ [A-Za-z0-9_.-]: d1 += 1
      d1′ = d1; while d1′ > d0 and t[d1′ − 1] ∈ {'.', '_'}: d1′ −= 1   // what trim_trailing would drop
      if d1 − d0 ≤ 253 and valid_domain(t[d0..d1′]):
        e = d1; while e < len(t) and t[e] ∉ White_Space and t[e] ≠ '<': e += 1
        e = i + len(trim_trailing(t[i..e]))
        if e > d0 and valid_domain(t[d0..min(d1, e)]):
          out += Text(t[last..i]) if i > last
          out += Start(Link{ExtAutolink, dest}), Text(t[i..e]), End(Link)
                 // dest = "https://" + t[i..e] for "www." ; t[i..e] otherwise
          last = i = e; continue
    i += 1
  out += Text(t[last..]) if last < len(t)

boundary(t, i)   = i == 0 or t[i−1] ∈ White_Space or t[i−1] ∈ {'*', '_', '~', '('}
valid_domain(D)  = D splits on '.' into ≥ 2 segments; every segment is non-empty and matches [A-Za-z0-9_-]+;
                   the last two segments contain no '_'
trim_trailing(s):                              // cmark-gfm autolink_delim, linear form
  opens = count('(' in s); closes = count(')' in s)
  loop:
    c = last char of s (stop if s is empty)
    if c ∈ {'?','!','.',',',':','*','_','~','\'','"'}: drop c; continue
    if c == ';': if s ends with /&[A-Za-z]+;/ drop that whole suffix, else drop c; continue
    if c == ')' and closes > opens: drop c; closes −= 1; continue
    break
```

Position 0 of every `Text` event counts as a boundary. Prefixes are recognised whatever their case (`HTTPS://`, `WWW.`). The link text is the matched literal. URL normalisation happens later, in `safe_href` (§5.1).

- The 253 cap applies to the whole domain run, trailing dots included; an implementation may stop scanning after 254 characters.
- The first `valid_domain` check ignores the trailing `.` and `_` that `trim_trailing` drops, so a domain at the end of a sentence links, as in GFM: `Visit www.example.com.` links `www.example.com` (`autolink.json#trailing-punctuation`). The second check still matters: a domain followed by a path may not end in `.` (`www.a.com./x` is not linked).

### 4.2 Pass 2: spoiler and details blocks

The pass evaluates every `Start(BlockQuote(None))`, in document order and each one independently, so nesting is allowed. Alerts (`BlockQuote(Some(_))`) are never converted.

```
for each Start(BlockQuote(None)) at index b, with matching End at index e:
  i = b + 1
  require S[i] == Start(Paragraph) and S[i+1] == Text(t)            // a Literal never matches
  require m = /^\[!(spoiler|details)\]/  (ASCII case-insensitive) at the start of t
  r = t[len(m)..]
  spoiler: require r consists only of U+0020/U+0009, and S[i+2] ∈ {SoftBreak, HardBreak, End(Paragraph)}
  details: require r == "" or r starts with U+0020/U+0009
  k = index of the first SoftBreak or HardBreak at inline depth 0 after i+1, or of End(Paragraph)
  summary = details only: [Text(trim_start(r)) if non-empty] ++ S[i+2 .. k]; if the last summary event is
            a Text, trailing White_Space is trimmed from it; then empty Text events are dropped
            // trim_start and the trailing trim both use White_Space (§9.1), so U+00A0 and U+3000 are trimmed
  delete S[i+1 .. k], and S[k] too if it is a SoftBreak or HardBreak
  if S[i] is now immediately followed by End(Paragraph): delete both
  S[b] := Start(Spoiler)  |  Start(Details), Start(Summary), summary…, End(Summary)
  S[e] := End(Spoiler)    |  End(Details)
```

- An empty details summary renders the fixed string `Details` (§8.6). Spoilers always render the fixed summary `Spoiler`.
- Only the marker line is removed. Everything else stays in the block.

### 4.3 Pass 3: figures

A `Paragraph` becomes a `Figure{caption}` when all of these hold:
- its direct children are exactly one `Image` subtree (`Start(Image)` … `End(Image)`), with nothing before or after it;
- that image's `title`, trimmed of White_Space, is non-empty.

The caption is the trimmed `title`, as plain text: it is never parsed as markdown. The image keeps its own events.

An image inside a link (`[![a](x "t")](y)`) is never a figure. Neither is a lone image in a *tight* list item: pulldown-cmark emits a tight item's content without a `Paragraph` (`figure.json#figure-in-list`); in a loose list the item's paragraph can become a figure (`#figure-in-loose-list`).

### 4.4 Pass 4: mentions

A `Link` is a **mention** when both of these hold:
- `safe_href(dest_url, Markdown)` returns `Profile(did)` (§5.2);
- the first event inside the link is a `Text` whose first character is `@`.

The pass marks it `Mention(did)` and does nothing else here.
- The writers (§8, §9) replace its inner content with `"@" + short_did(did)`. Display layers later substitute the verified handle (§5.4).
- P always uses the link's source inner text (§10.1). So P does not depend on handle resolution.

`[@x](https://…)` and `[@x](at://<handle>)` are never mentions (F9).

### 4.5 Pass 5: URL policy

**Links.**

| Destination, after `mailto:` is prefixed to `LinkType::Email` destinations | `safe_href` result (§5.1) | Rendered as |
|---|---|---|
| `https:`, `http:` | `External` | external link with `rel` (§8.1) |
| `mailto:` | `Mailto` | external link with `rel` |
| `at://did/nsid/rkey` | `Record` | internal route plus hover preview (§5.3) |
| `at://did`, marked as a mention by pass 4 | `Profile` | mention |
| `at://did`, otherwise | `Profile` | profile link |
| `#frag` | `Anchor` | in-document anchor (`data-anchor`; no `href`) |
| anything else: relative, `//host`, `javascript:`, `data:`, `file:`, handle-authority `at://`, invalid | `None` | **link dropped**: its inner content is rendered without a link |

The link `title` is never rendered.

**Images.**

| Destination | Rendered as |
|---|---|
| `cid:<cid>` | media resolution (§6.1–6.2) |
| `https:` or `http:` accepted by `safe_href` as `External` | Never fetched by the renderer. The link `<a class="ext-image">` with text `"image: " + (alt ≠ "" ? alt : host)`. Inside a link, the same text without its own `<a>`. |
| anything else | the alt text, as plain text |

`alt` = `plain(inner events of the image)` (§10.1 rules, no normalisation).

**No nested anchors.** A link inside a link cannot occur in CommonMark. Even so, the writers MUST NOT emit `<a>` inside `<a>`. Any construct that would do so (an external image, a footnote reference) renders its anchor-free form inside a link: the label text, or `<sup>N</sup>`. With pulldown-cmark 0.13.4 a footnote reference never ends up inside a link: a `[^x]` in link text prevents the link (`footnotes.json#reference-in-link-text-prevents-link`), so the `<sup>N</sup>` form is defensive.

### 4.6 Pass 6: ids

**Heading ids** are `md-h-` + `slug`, deduplicated over every heading in S3 event order (including headings inside containers and footnote definitions):

```
slug(h):  s = NFC(proj(h))                  // proj: §10.1 rules over the heading's inline events, without collapse or trim
          s = to_lowercase(s)               // Unicode full lowercase mapping (Rust str::to_lowercase)
          s = keep c iff c ∈ {' ', '-', '_'} or General_Category(c) ∈ {Lu, Ll, Lt, Lm, Lo, Nd, Nl, No}
          s = replace every ' ' (U+0020) with '-'
dedup (github-slugger):  seen = {}, n = {}
          r = s; while r ∈ seen: n[s] += 1 (from 0); r = s + "-" + n[s]
          seen += r; id = "md-h-" + r
```

An empty slug is allowed: the first such heading gets `md-h-`, the next `md-h--1`.

**Footnotes.** Labels match by `UniCase` (§2), like pulldown-cmark's own matching.
- **Definitions.** For each label, only the first definition in event order can be rendered; any later duplicate never is. A definition nested inside another definition is a definition of its own: its events belong to it, not to the enclosing one.
- **Rendered definitions** are the smallest set R such that the first definition of every label referenced from the body (outside every definition) is in R, and the first definition of every label referenced from inside a definition in R is in R. References inside a definition outside R do not count: `[^u]: u[^v]` with `u` unreferenced renders neither `u` nor `v` (`footnotes.json#reference-only-from-unrendered-definition`). Unrendered definitions still contribute to P (§10.1).
- **Counting references** are the references in the body and those inside rendered definitions. Only they are rendered; a reference inside an unrendered definition gets no ordinal and no K.
- **Ordinals.** Each rendered definition's label gets ordinal N = 1, 2, … in the order of its first counting reference in S6 event order (a reference inside a definition counts at its source position, so a definition written before the body can take ordinal 1: `footnotes.json#definition-before-body-reference-order`).
- **Ids.** The K-th counting reference (K = 1, 2, …, in S6 event order) to ordinal N gets id `md-fnref-N-K`. Definition N gets id `md-fn-N`.

**In-content anchors.** For `Anchor(v)`, the value v is already decoded, lowercased and charset-checked (§5.1). It is emitted as `data-anchor="v"`.

**Resolving an anchor.** The SPA resolves `data-anchor` **inside the same rendered root**, never in the whole document:
- if v starts with `md-`, look for the element with id v;
- otherwise look for id `md-h-` + v.

It scrolls with `scrollIntoView()`. Content never assigns `location.hash`.

**Id uniqueness.** Ids are unique within one rendered root, not across the roots on a page. Consumers MUST NOT use document-wide id lookup for content ids.

---

## 5. URLs, internal links, mentions and previews

### 5.1 `safe_href`

`safe_href(raw, ctx) -> Option<Href>` is the only gate for every URL that becomes clickable anywhere: markdown, record fields, the TUI's `xdg-open` and OSC 8.
- `ctx` is `Markdown` or `Field`.
- Core exports it through wasm as `safeHref`. The SPA's `<SafeLink>` uses it, and a lint rule bans raw `href={…}` (F7).
- `Field` covers `defs#link.uri`, `defs#externalLink.uri`, `forum.reportContact` and `#endpoint.url`.

```
Href = External{url, host} | Mailto{url, host} | Record{uri: AtUri} | Profile{did} | ProfileHandle{handle} | Anchor{id}

safe_href(raw, ctx):
  1. reject if raw is empty or longer than 3000 bytes
  2. reject if raw contains: any code point ≤ U+0020, U+007F, U+0080–U+009F, a bidi control
     (U+061C U+200E U+200F U+202A–U+202E U+2066–U+2069), U+2028, U+2029, U+200B–U+200D, U+2060, U+FEFF, or '\'
  3. if raw starts with '#':
       ctx must be Markdown; v = percent_decode_utf8(raw[1..]) (invalid UTF-8 → reject); v = to_lowercase(v)
       v must be non-empty, ≤ 512 bytes, and every char ∈ {'-','_'} ∪ GC{L*, N*}       → Anchor{v}
  4. scheme = longest match of ^[A-Za-z][A-Za-z0-9+.-]*: at the start; none → reject (relative or "//host")
     s = ascii_lowercase(scheme without ':'); rest = what follows the ':'
  5. s ∈ {"https","http"}:
       rest must start with "//"; authority = rest[2 .. first of '/','?','#' or end]
       reject if authority contains '@' (userinfo)
       host = "[" a "]" where a is an IPv6 address (RFC 4291 §2.2 text form, below), or ascii_lowercase of the part before the first ':'
       host must match ^[a-z0-9._-]+$ (or the IPv6 form) and contain a char other than '.'   // non-ASCII hosts are rejected
       a host that is not an IPv6 literal must pass host_ok (below)        // what the WHATWG URL parser accepts and keeps
       port, if present, ^[0-9]{1,5}$ and ≤ 65535
       url = s + "://" + host + (":" + port)? + pct(rest after authority)                  → External{url, host}
     s == "mailto":
       rest non-empty and not starting with '/'
       addr = rest up to the first '?'; query = the rest after it, if there is a '?'
       a = percent_decode_utf8(addr) (invalid UTF-8 → reject); split a at its first '@' into local and domain (no '@' → reject)
       local must be non-empty and contain none of '@' ',' ';' '<' '>' nor a code point rejected by step 2
       domain = ascii_lowercase(domain); it must match ^[a-z0-9._-]+$ and contain a char other than '.'
       if query is present: reject if any '&'-separated field's name (the part before its first '='),
         percent_decode_utf8'd, is invalid UTF-8 or equals "to", "cc" or "bcc" (ASCII case-insensitive)
       url = "mailto:" + pct(rest); host = domain                                         → Mailto{url, host}
     s == "at":  parse_at_link(rest, ctx) (§5.2)                                            → Record | Profile | ProfileHandle
     otherwise → reject
pct(x): each UTF-8 byte of x that is not in [A-Za-z0-9] or "-._~:/?#[]@!$&'()*+,;=%" becomes "%XX" (uppercase hex)
percent_decode_utf8(x): each "%" followed by two hex digits becomes that byte; any other "%" stays; the bytes must be UTF-8
IPv6 text form: eight groups of 1–4 hex digits joined by ':'; one "::" may stand for one or more zero groups; the last two
  groups may be written as an IPv4 dotted quad (decimal 0–255, no leading zeros). No zone id.
host_ok(h):
  if h ends in a number (its last label, after removing one trailing '.', is non-empty and all ASCII digits,
    or is "0x" followed by hex digits): h must be an IPv4 dotted quad: four decimal numbers 0–255 joined by '.',
    without leading zeros
  otherwise every label of h that starts with "xn--" must be well-formed Punycode: the rest p is non-empty, does not
    end in '-', is ≤ 2000 bytes, and decodes (RFC 3492 §6.2; digits a–z, 0–9; a '-' in first position is not a
    delimiter; no overflow; no surrogate or code point above U+10FFFF) to at most 1000 code points, the first of them
    not U+200C or U+200D
```

A `mailto:` URL must address exactly one mailbox, and its query may not add recipients. The host shown next to the link (§5.5, §9.4) is then the only domain the mail goes to: "the part after the last `@`" would show `google.com` for `mailto:a@evil.com?cc=b@google.com`, and an empty address (`mailto:?to=…`) would show no host at all (`links.json#mailto-*`, `safe-href.json#mailto-*`). Like `http` hosts, internationalised mail domains are rejected.

Consequences:
- `javascript:`, `data:`, `file:`, `vbscript:`, relative and protocol-relative URLs are never accepted.
- Non-ASCII in the path, query or fragment is percent-encoded, and any `%` already present is kept as is.
- A backslash is rejected everywhere, because WHATWG treats it like `/` in special schemes, which would make our parsing and the browser's disagree.
- Every `External` host is one the WHATWG URL parser accepts and keeps as written (`host_ok`). That parser rejects some hosts the grammar allows: `256.1.1.1`, `a.123`, `[1.2.3.4]` and malformed Punycode (`xn--`, `xn--abc-`). It rewrites others to another address than the one shown: `0x7f.1`, `1.2.3` and `2130706433` all become an IPv4 address. A link to any of these would open nothing, or open a host the reader was not shown, and ammonia would strip its `href` (§8.5). `host_ok` needs no Unicode tables. A Punycode label is checked by decoding it, but it is never decoded for display (threat model ID-3) (`links.json#punycode-*`, `#ipv4-dotted-quad-only`, `#ipv6-must-parse`, `safe-href.json#ipv4-*`, `#punycode-*`).

**Opening links outside the app.** Only `External` and `Mailto` may be opened outside the app. The URL is passed as a single argv entry and never through a shell.

### 5.2 `at://` links

```
parse_at_link(rest, ctx):                  // rest = text after "at:"
  require rest starts with "//"; body = rest[2..]; reject if body contains '?' or '#'
  segs = body.split('/')
  segs[0] is a DID:  did:plc → ^did:plc:[a-z2-7]{24}$ ;  did:web → ^did:web:H$ where H is a valid atproto
                     handle written in lower case (≥ 2 labels, top-level label not starting with a digit;
                     so no port, path or percent-encoding) ;  any other method → reject
    len(segs) == 1 → Profile{did}
    len(segs) == 3 → segs[1] is a valid NSID whose authority (every segment but the last) is lower case,
                     and segs[2] a valid record key (atproto syntax, lessdugin-atp)
                     → Record{uri = "at://" + did + "/" + nsid + "/" + rkey}       // the canonical AtUri
    otherwise (including a trailing '/') → reject
  segs[0] is a valid atproto handle (lowercased), len(segs) == 1, and ctx == Field → ProfileHandle{handle}
  otherwise → reject
```

**Markdown never accepts handle authorities.** Writers MUST rewrite `at://<handle>/…` destinations to DIDs before publishing (§12). A `ProfileHandle` from a record field (for example `reportContact`) is shown as `@handle`. It becomes navigable only after the handle verifies in both directions (`sync.md` §3.1); until then it is plain text.

**Routes:**
- `Record{at://U}` → `href="#/at/U"`, plus `data-at-uri="at://U"` (the canonical URI: scheme `at`, lower-case NSID authority).
- `Profile{d}` and mentions → `href="#/u/d"`, plus `data-did="d"`.

These two attributes are all a hover preview (§5.3) needs: `data-at-uri` names the record, from which the client derives eligibility, author and forum; `data-did` names the user of a card. Before use, the SPA MUST re-parse `data-at-uri` and `data-did` with the core's parser (F5).

### 5.3 Hover previews

| Rule | |
|---|---|
| Eligible | `a[data-at-uri]` whose NSID is one of `NS.post`, `NS.comment`, `NS.sequence`, `NS.wikiRevision` or `NS.forum` (`preview_eligible(uri, NS)`; the markdown crate takes the build's `NS` as a parameter, since it does not depend on the lexicon crate); and `a[data-did]` (a user card, mentions included). Other collections, including foreign ones like `app.bsky.feed.post` and our collections under the other build's `NS`, still navigate to `#/at/…` but get no preview. |
| Depth | 1. Inside a preview container, hover and focus handlers MUST NOT open another preview. |
| Trigger | Pointer hover or keyboard focus, which implementations SHOULD debounce. At most one preview fetch per pointer is in flight. TUI and CLI have no hover; a TUI MAY show the same preview on an explicit key, under the same rules. |
| Content source | Only `pds` or `proven` records (never hints). If the record is not cached, it is point-fetched from the author's PDS. DID cards resolve the identity lazily, at hover time, and never before (F16). |
| Moderation (fail closed) | The preview shows the body only if the target is in a forum whose fold is available locally, and it resolves as `Visible` or `Collapsed` (collapsed shows the collapse notice first). In every other case the preview is a metadata card only: kind, author identity, forum. It offers "open", which navigates to the route and so runs the normal forum open and sync. |
| Rendering | The same renderer, the same render cache, the same Trusted Types policy and the same DOMPurify configuration (F5). Media inside a preview follows the autoload policy (`sync.md`, synthesis §3.9), treating the preview's target as the item. User cards show the profile `bio` only as its excerpt (§10.4), as text. |

### 5.4 Mention display

**Placeholder.** The renderer emits `@` + `short_did(did)`:
- `did:plc:…` → `did:plc:` + the first 8 characters of the identifier + `…`;
- `did:web:…` → the whole DID if it is at most 40 characters, otherwise its first 39 characters + `…`.

**Substitution.** At display time:
- If the identity cache holds a handle for that DID that is verified in both directions, the SPA, TUI and CLI replace the text with `"@" + handle`. The SPA uses `textContent`; terminals apply `term_sanitize(Name)`.
- Otherwise the placeholder stays.

**Resolution budget per body.**
- Eager resolution, when the body is displayed, covers at most the first 16 distinct mention DIDs in document order, and only those that are participants of the forum (synthesis §5.3, F16).
- Every other mention resolves only on hover, focus or click.

**Styling.** Mention styling is applied only to mentions as defined in §4.4. A link whose text merely starts with `@` gets no special styling.

### 5.5 Showing the real host (F9)

**HTML.** For an `External`/`Mailto` link, including `ext-image`, compute `t` from the link's visible text `v`: `plain(inner)` for a link, the alt for an `ext-image` (never the `image: …` label, which is not host-like). The test runs on the text as it is displayed, so invisible and compatibility characters cannot hide a host:
1. remove every Default_Ignorable_Code_Point (U+00AD, U+034F, U+061C, U+115F–U+1160, U+17B4–U+17B5, U+180B–U+180F, U+200B–U+200F, U+202A–U+202E, U+2060–U+206F, U+3164, U+FE00–U+FE0F, U+FEFF, U+FFA0, U+FFF0–U+FFF8, U+1BCA0–U+1BCA3, U+1D173–U+1D17A, U+E0000–U+E0FFF);
2. NFKC (fullwidth `ｇｏｏｇｌｅ．ｃｏｍ` becomes `google.com`), then map U+3002 and U+FF61 (ideographic full stops, which IDNA treats as dots) to `.`;
3. trim White_Space and ASCII-lowercase;
4. strip one leading `@`; if `t` still contains `@`, keep only the part after the last `@`;
5. strip one `http://`, `https://` or `mailto:` prefix, then one `www.` prefix.

**Host-like test.** A *name character* is a code point of General_Category L*, M*, Nd, Nl or No, or `_` or `-`; lookalikes of any script count (`ɡoogle.com`, Cyrillic `gооgle.com`). `t` is host-like if its first code point is a name character and the code point after its first `.` is a name character, whatever follows (`google.com!` and `not.a host` are host-like, `a..b` is not).

Let `m` be the longest prefix of `t` made of name characters and `.`, with its trailing `.` removed. If `t` is host-like and `m ≠ host` (with one `www.` stripped from `host` too), the writer appends `<span class="link-host">host</span>` inside the `<a>`, after its content. So the span is left out only when the text starts with exactly the link's host followed by something other than a name character or a dot (`links.json#host-display-*`).

**Terminal.** The rule is F1's: see §9.4.

---

## 6. Media

### 6.1 `cid:` resolution

A `cid:` image URL resolves as follows:
1. Strip `cid:`, matched ASCII case-insensitively, to get `c`.
2. `c` MUST match `^b[a-z2-7]{1,127}$`; otherwise the image is a **miss**.
3. It resolves to the **lowest** index i where `media[i].blob.ref.$link == c`, compared byte for byte.

It never resolves against anything outside this body's `media` array. No arbitrary CID is ever fetched. If nothing matches, it is a **miss**. Only images resolve: a `cid:` link destination is not a URL `safe_href` accepts, so the link is dropped (`images.json#cid-link-not-image-dropped`).

### 6.2 Rendering

| Case | HTML | Terminal placeholder (§9.3) |
|---|---|---|
| Hit; `media[i].blob.mimeType` ∈ {`image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/avif`} | `<img data-cid="c" alt="A" width="W" height="H">` | `[image N: A]` |
| Hit; any other type | `<span class="media" data-cid="c">L</span>` | `[video N: L]`, `[audio N: L]` (by `video/*` or `audio/*`), else `[file N: L]` |
| Miss | `<span class="media-missing">alt</span>` | `[missing media: alt]` |

The declared `mimeType` is compared exactly as written, like the table: `IMAGE/PNG`, `image/png; x=y` and ` image/png` are files, not images, and `Video/MP4` is a file (`images.json#mime-exact-match`). Writers normalise MIME types before upload (`data-model.md` §8.8).

The symbols in the table:
- **A** (alt): the markdown alt (§4.5), trimmed, if it is non-empty; else `media[i].alt`, trimmed, but only on the first reference to item i whose markdown alt is empty, in render order (the body, then the rendered footnotes in ordinal order; an image inside another image's alt is never rendered and does not count); else `""`. The `alt` attribute is always emitted. Without the first-reference rule, `![](cid:x) ` repeated over a 100 KB body would repeat a 20,000-byte `media.alt` in every reference (176 MB of HTML; `images.json#cid-alt-fallback-*`, `#media-alt-amplification-*`). Trimming removes White_Space and U+FEFF from both ends, a superset of ECMAScript `String.prototype.trim`, which DOMPurify applies to attribute values (`images.json#alt-trimmed`).
- **W, H**: present only if `media[i].aspectRatio` is present. Start from `(w, h) = (width, height)`; if `w > 20h`, set `w = 20h`; if `h > 20w`, set `h = 20w`. They are a ratio hint only, and the SPA's CSS sets `max-width: 100%; height: auto`.
- **L** (label): A if non-empty; else `media[i].name`, if present and acceptable under `data-model.md` §4 (no `/`, `\`, C0, C1, U+2028 or U+2029); else `""`.
- **N**: 1-based, as `i + 1`. This is the numbering that `media get|open --index` uses.

When L or A is empty, the placeholder drops the `: …` part.

The HTML never contains `src`. The SPA alone does all of the following, after sanitisation:
- verifies the bytes against the CID (streaming `Host::fetch_blob`);
- checks that the declared and sniffed types agree and are on the allowlist;
- applies the autoload policy;
- sets `src` to an object URL, or replaces `span.media` with a player or download card, using DOM APIs.

### 6.3 `media_used` and gallery

- **`media_used`** = every index i whose `blob.ref.$link` equals the CID of a resolved `cid:` image in the rendered content of S6, in ascending order. The rendered content is the body and the rendered footnote definitions (§4.6), without the events inside an image (a nested image is part of its parent's alt and never shown). Duplicate entries of a referenced CID count as used. An image referenced only from an unreferenced footnote definition is therefore in the gallery, so every media item is shown somewhere (`gallery.json#image-only-in-unreferenced-footnote`).
- **The gallery** holds every other index, in array order. It renders after the footnotes as `<div class="gallery">`, with one placeholder per item (as in §6.2, with A = `media[i].alt`, trimmed). An item whose CID does not match `^b[a-z2-7]{1,127}$` renders as `<span class="media-missing">A</span>` (`gallery.json#gallery-invalid-cid`). In the terminal, the gallery is `Media` blocks.
- If every index is used, there is no gallery element.

### 6.4 External images (opt-in)

External images are never fetched by default, and the HTML is the same whatever the viewer's settings are. A client MAY offer a per-forum opt-in, off by default. When the user has opted in for that forum, the client may replace `a.ext-image` with an image, under all of these rules:
- `credentials: 'omit'` and `referrerPolicy: 'no-referrer'` (natively, through the SSRF guard);
- the response is ≤ 5 MB;
- the sniffed type is on the inline image allowlist;
- the image is shown through a `blob:` object URL. CSP `img-src 'self' blob:` forbids remote `src`.

---

## 7. Math

### 7.1 Acceptance (part of `lfm/1`)

The TeX source is the payload of `InlineMath` or `DisplayMath`, without delimiters.

Before any engine sees an expression, the pre-scan runs. If any check fails, the expression is a **math error**:

1. **Size.** At most 4,096 bytes.
2. **Group depth.** At most 64, counting all of `{` … `}`, `\left` … `\right` and `\begin{…}` … `\end{…}`.
   - `\{`, `\}` and `\\` are skipped, and are not groups.
   - A depth that goes negative is an error.
3. **Disabled control words.** Scan for control words: `\` followed by a maximal run of `[A-Za-z]`. The expression is an error if it contains:
   - any of `\href \url \includegraphics \class \style \def \gdef \edef \xdef \let \futurelet \newcommand \renewcommand \providecommand \newenvironment \renewenvironment \DeclareMathOperator \DeclareRobustCommand \catcode \input \include \label \eqref \ref`;
   - or any control word starting with `\html`.

The pre-scan runs on the raw text, so a disabled word inside `\text{}` or after `%` still counts. A control word is the whole run of letters: `\labelx`, `\refs` and `\rfloor` are not disabled, and `\\ref` is a row break followed by the text `ref` (`math.json#double-backslash-*`).

`\label`, `\eqref` and `\ref` are disabled by ADR 0002: math-core turns them into `id` and `href` attributes, which would collide with the document's own anchors (§8.2) and link into other posts on the same page. Equation numbers through `\tag` inside `equation` stay available (`math.json#disabled-label-eqref-ref`, `#normalise-tag-no-id`).

### 7.2 Engine (default mode)

**Engine.** `math-core`, pinned exactly at `=0.9.0` (ADR 0002, spike S10). It converts each expression independently with `convert_with_local_state`, under this configuration:
- `MathCoreConfig { pretty_print: Never, xml_namespace: false, ignore_unknown_commands: false, annotation: false, allow_unreliable_rendering: false, global_group: false, unicode_substitution: Conventional, max_expansions: 1000, ..Default }`;
- no custom macros;
- display mode is `Inline` for `InlineMath` and `Block` for `DisplayMath`.

**Errors.** An engine error, including an unknown command, is a math error.

**Guard (presentation).** math-core 0.9 parses recursively without a depth limit, and its operator inference re-reads the rest of a row at every opening delimiter that has no closing partner, doubling its work each time (`$((((…(x$` with 28 `(` takes about ten seconds). Before the engine runs, a guard refuses, as a math error:
- an expression whose parser nesting exceeds 96, estimated from the argument counts of math-core's commands, groups and environments (`\sqrt\sqrt…x` and `\frac a\frac a…` would otherwise exhaust a 1 MiB wasm stack within 4,096 bytes; `math.json#engine-nesting-guard`);
- an expression with more than 8 unmatched opening delimiters (`math.json#guard-unmatched-*`, `#guard-closing-arguments-cancel-nothing`, `#guard-row-break-between`). The count is an upper bound: every token math-core can make an opening delimiter counts (`(`, `[`, `\{`, `\langle`, `\lvert`, `\bigl(`, `\mathopen{…}`, …), also as a command argument, a script or the only content of a group, and a closing delimiter cancels one only when both sit directly in the same row, with no row break (`&`, `\\`, `\newline`, `\over` and the other infix fractions) between them;
- an expression with more than 64 opening delimiters open at once within one row (`math.json#guard-delimiter-nesting-65`).

The guard bounds one expression, to about 45 ms natively for the worst 4,096 bytes. **Engine budget:** an expression of *n* bytes that the guard accepts with *u* unmatched opening delimiters costs *n* · 2^*u*, charged before the engine runs, in the order the HTML writer renders expressions; a document may spend 4,194,304 (2^22). An expression that costs more than what is left is a math error, so once the budget is spent every further expression is one (`math.json#engine-document-budget`). A body (at most 1,000,000 bytes) whose expressions each have at most 2 unmatched delimiters can never reach it; without the budget, a 1 MB body of 4,096-byte expressions with 8 unmatched delimiters each took 7.8 s natively, with it 0.17 s, no more than a balanced body (about 0.45 s).

**Normalisation.** The engine's output is never inserted as is. It is parsed into a tree (iteratively), normalised and filtered (§7.3), and re-serialised by our serialiser (§8.3).

### 7.3 MathML normaliser (presentation)

math-core writes part of its presentation as inline `style` attributes, which CSP `style-src 'self'` does not apply. Following ADR 0002, the normaliser maps each of them to a class from a closed set, or to `mathcolor`, and filters everything else; a static stylesheet styles the classes. The output never has a `style` attribute.

**Allowed elements.** `math mrow mi mn mo ms mtext mspace msub msup msubsup munder mover munderover mmultiscripts mprescripts none mfrac msqrt mroot mtable mtr mtd mpadded mphantom mstyle merror menclose`.

**Other elements:**
- `semantics` is replaced by its first child element.
- `annotation`, `annotation-xml` and `maction` are removed together with their subtrees.
- The HTML elements math-core emits inside `mtext`, `a span b strong i em code s u`, are unwrapped: their children are kept (their styles become classes, below).
- Token elements (`mi mn mo ms mtext`) keep only text: any other element inside one is removed with its subtree. `mspace`, `none` and `mprescripts` are always empty.
- Every other element is removed with its subtree.

**Styles.** Each `style` attribute is split at `;` into declarations, written `property:value` with the whitespace around `:` removed and each run of whitespace in the value collapsed to one space. A declaration in this table adds its class to the element; math-core emits the declarations that share a class together.

| Declarations | Class |
|---|---|
| `text-align:left`, `justify-items:start` | `mc-al` |
| `text-align:right`, `justify-items:end` | `mc-ar` |
| `padding-left:0` | `mc-pl0` |
| `padding-right:0` | `mc-pr0` |
| `padding-top:0`, `padding-bottom:0` | `mc-py0` |
| `padding-left:0.1em`, `padding-right:0.1em` | `mc-px01` |
| `padding-left:1em` | `mc-pl1` |
| `padding-left:0.1667em`; `0.2222em`; `0.2778em` | `mc-pl3mu`; `mc-pl4mu`; `mc-pl5mu` |
| `padding-right:0.1667em`; `0.2222em`; `0.2778em` | `mc-pr3mu`; `mc-pr4mu`; `mc-pr5mu` |
| `padding-top:0.1667em` | `mc-pt3mu` |
| `width:7.5%`; `50%`; `100%` | `mc-w7-5`; `mc-w50`; `mc-w100` |
| `border-top:0.05em solid currentcolor`; `…dashed…` | `mc-bt-solid`; `mc-bt-dashed` |
| `border-right:0.05em solid currentcolor`; `…dashed…` | `mc-br-solid`; `mc-br-dashed` |
| `border-left:0.05em solid currentcolor`; `…dashed…` | `mc-bl-solid`; `mc-bl-dashed` |
| `margin-left:-0.16666667em`; `-0.22222222em`; `-0.2777778em` | `mc-negthin`; `mc-negmed`; `mc-negthick` |
| `math-shift:compact` | `mc-cramped` |
| `font-variant-caps:small-caps` | `mc-tsc` |
| `font-size:N%`, N ∈ {50, 60, 70, 80, 90, 100, 120, 140, 170, 200, 250} | `mc-fsN` |

Other declarations:
- `color:#` and six hexadecimal digits becomes `mathcolor` (a MathML Core global attribute) with the digits in lower case. math-core accepts only named colours (`\color{red}`, `\textcolor{Periwinkle}{…}`) and writes them in hexadecimal (`math.json#normalise-color-mathcolor`, `xss/mathml.json#math-color-*`).
- On `mspace`, a `width` equal to its `width` attribute (math-core repeats a `rem` width as a style for Firefox) and any other negative `margin-left` are dropped as known, with no class.
- Any other declaration is dropped and its element kept (fail closed). The vector runner asserts that no expression of the `mathml/` vectors produces one, so a math-core upgrade that emits a new style shows up in review.

**Text styles.** math-core writes `\textbf`, `\textit`, `\emph`, `\texttt`, `\sout`, `\underline`, `\textsf` and `\textrm` as HTML wrappers inside `mtext`. When the content of a token element is one chain of wrappers around text (the only shape math-core 0.9 emits), each wrapper adds a class to the token element, and a wrapper's own `style` is mapped as above (`\textsc` gives `mc-tsc`, `\small` an `mc-fsN`; `math.json#normalise-text-style-classes`):

| Wrapper | Class | Its declaration in the stylesheet |
|---|---|---|
| `b`, `strong` | `mc-tb` | `font-weight: bold` |
| `i`, `em` | `mc-ti` | `font-style: italic` |
| `code` | `mc-tt` | `font-family: monospace` |
| `s` | `mc-ts` | `text-decoration: line-through` |
| `u` | `mc-tu` | `text-decoration: underline` |
| `span class="math-core-sans-serif-font"` | `mc-tsf` | `font-family: sans-serif` |
| `span class="math-core-serif-font"` | `mc-trm` | `font-family: serif` |

**Engine classes.** Of math-core's own `class` tokens, `menclose-updiagonalstrike`, `menclose-downdiagonalstrike` and `menclose-phasorangle` (the `menclose` polyfill of `mathmlfixes.css`, for `\cancel`, `\bcancel`, `\xcancel` and `\phase`) are kept; every other token is dropped (`math.json#normalise-menclose-class`).

**Allowed attributes.** Everything else is dropped, including `style` (after the mapping above), `id`, `href`, `src`, `xlink:*`, `xmlns*`, `on*`, `mathbackground`, `data-*`, and any `class` or `mathcolor` attribute of the engine's own (`math.json#normalise-tag-no-id`, `xss/mathml.json#math-eqref-ref-href`, `#math-raw-class-style-mathcolor-literal`).

| Attribute | On | Value |
|---|---|---|
| `display` | `math` | `block` (emitted only for display math) |
| `displaystyle`, `fence`, `separator`, `stretchy`, `symmetric`, `largeop`, `movablelimits`, `accent`, `accentunder` | any | `true` \| `false` |
| `scriptlevel` | any | `^[+-]?[0-9]{1,2}$` |
| `mathvariant` | any | `normal` |
| `form` | `mo` | `prefix` \| `infix` \| `postfix` |
| `lspace`, `rspace`, `minsize`, `maxsize`, `linethickness`, `width`, `height`, `depth`, `voffset` | any | ≤ 32 bytes, `^[+-]?[0-9]*\.?[0-9]+([a-z]{1,3}\|%)?$` |
| `columnspan`, `rowspan` | `mtd` | `^[1-9][0-9]{0,2}$` |
| `notation` | `menclose` | `^[a-z ]{1,128}$` |
| `class` | any | one or more tokens of the closed set (the 46 `mc-*` classes of the two tables above and the three engine classes), in ascending ASCII order, without duplicates, separated by one space |
| `mathcolor` | any | `^#[0-9a-f]{6}$` |

**Serialisation.**
- Attributes are sorted by name (ASCII), `class` and `mathcolor` among them.
- Every element has an explicit end tag; there are no self-closing forms.
- Text is escaped as in §8.3.

**Limits.** After filtering, if the element depth inside `math` (where `math` is 1) exceeds 64, or the serialised `math` element exceeds 65,536 bytes, the expression is a math error. The serialised `math` elements of one document total at most 16 MiB (16 · 2^20 bytes): an expression that would pass that is a math error (`math.json#mathml-document-budget`).

**Stylesheet.** `crates/markdown/assets/mathml.css` (CC0-1.0) holds one rule `math .mc-x.mc-x` per class, with exactly the declarations of the two tables above: specificity (0,2,1) outranks every rule of math-core's `mathmlfixes.css` that can match the same element, without `!important`. It is loaded after `mathmlfixes.css` (MIT, shipped next to it with its licence notice: `mtd` padding, the `menclose` polyfill, accent margins). `lessdugin_markdown::math::MATHML_CSS` is the two files concatenated, for exports; the SPA imports both (`web/src/styles/mathml.css`). A test checks that every class has a rule.

**Remaining degradation.** Negative spaces other than `\!`, `\negmedspace` and `\negthickspace` (`\mkern-6mu`, `\kern-2pt`) are lost, and so on Firefox is a space in absolute units (`\hspace{2pt}`), which math-core writes in `rem` (`mathml/math-core-0.9.json#norm-negative-space-other-dropped`, `#norm-rem-space-width-style`). Colours other than named ones cannot be written. The `mathml/` vectors record which S10 snippets and probes the engine renders (§13).

### 7.4 Math errors

A math error renders as `<code class="math-error">TEX</code>`, for inline and display math alike, with the source escaped. It increments `flags.math_errors`. Terminals and P always use the TeX source (§9.3, §10.1), so they do not depend on the engine.

### 7.5 Temml fallback mode

S10 accepted math-core (ADR 0002), so v1 releases use the default mode. Temml mode is the documented contingency: if a math-core upgrade regresses the `mathml` vectors and cannot be pinned around, the release is built with `MATH_MODE = Temml`. This is a release-wide constant included in `RENDERER_VERSION`, and every client of a release MUST use the same mode.

**What the renderer emits.** Expressions that pass §7.1 render as `<span data-tex="inline">TEX</span>` or `<span data-tex="block">TEX</span>`. Errors render as in §7.4.

**What the SPA does.**
1. Lazily loads Temml.
2. Renders the element's `textContent` with:
   - `displayMode` taken from `data-tex`;
   - `trust: false`, `throwOnError: true`, `maxExpand: 1000`, and no `macros`.
3. Sanitises the result with DOMPurify's MathML profile, generated from §7.3.
4. Replaces the span.

On a Temml error, the span stays as source text.

**Font.** The STIX Two Math subset font is **Reserved (deferred)**, per the completeness review. The stylesheets of §7.3 set no font: system math fonts are used. ADR 0002 suggests evaluating math-core's patched fonts (`math-core-fonts`) instead.

---

## 8. HTML output contract

### 8.1 Construct mapping

The root is `<div class="lfm" dir="auto">` + `"\n"` + body + footnotes + gallery + `</div>`.

`N`, `K`, `S` and `L` stand for computed values. Newlines are shown where the writer emits them (§8.3).

| Construct (SL) | HTML |
|---|---|
| Paragraph | `<p dir="auto">…</p>` |
| Heading n | `<hn id="md-h-S" dir="auto">…</hn>` |
| BlockQuote(None) | `<blockquote>` … `</blockquote>` |
| Alert kind | `<div class="alert-note\|alert-tip\|alert-important\|alert-warning\|alert-caution">` … `</div>`. No title text is emitted; the SPA's CSS supplies the label. |
| Spoiler | `<details class="spoiler">` `<summary dir="auto">Spoiler</summary>` … `</details>` |
| Details | `<details>` `<summary dir="auto">SUMMARY or Details</summary>` … `</details>` |
| Figure | `<figure>` *media* `<figcaption dir="auto">caption</figcaption>` `</figure>` |
| CodeBlock | `<pre><code class="language-L">text</code></pre>`. L = the ASCII-lowercased first token of a fenced info string split on ASCII whitespace (U+0009, U+000A, U+000C, U+000D, U+0020, as HTML splits `class`; so `a b` with U+00A0 is one invalid token); the class is emitted only if L matches `^[a-z0-9_+-]{1,32}$`. Indented code and invalid L get `<pre><code>`. |
| List | `<ul>`, or `<ol>`, or `<ol start="n">` when n ≠ 1 |
| Item | `<li dir="auto">…</li>` |
| TaskListMarker(b) | `<input type="checkbox" disabled="">` (b false) or `<input type="checkbox" checked="" disabled="">` (b true), followed by one U+0020 |
| Table | `<table>`, `<thead><tr>` + cells + `</tr></thead>`, then `<tbody>` … `</tbody>` only if there are body rows, then `</table>` |
| TableCell | `<th class="ta-l\|ta-c\|ta-r" dir="auto">` in the head, `<td …>` in the body. The class is omitted for `Alignment::None`. |
| Rule / HardBreak / SoftBreak | `<hr>` / `<br>` / `"\n"` |
| Emphasis / Strong / Strikethrough | `<em>` / `<strong>` / `<del>` |
| Code | `<code>text</code>` |
| Text, Literal | escaped text |
| InlineMath / DisplayMath | `<math>…</math>` / `<math display="block">…</math>`, or §7.4, or §7.5 |
| External link | `<a href="URL" rel="nofollow ugc noopener noreferrer">…</a>`; when §5.5 applies, `<span class="link-host">host</span>` is the last child of the `a` |
| External image | `<a href="URL" class="ext-image" rel="nofollow ugc noopener noreferrer">image: …</a>` |
| Record link | `<a href="#/at/U" data-at-uri="at://U" class="at-link">…</a>` |
| Profile link | `<a href="#/u/D" data-did="D" class="at-link">…</a>` |
| Mention | `<a href="#/u/D" data-did="D" class="mention">@short_did</a>` |
| Anchor link | `<a data-anchor="v">…</a>` |
| Dropped link | inner content only |
| FootnoteReference | `<sup id="md-fnref-N-K"><a data-anchor="md-fn-N">N</a></sup>`. Inside a link: `<sup id="md-fnref-N-K">N</sup>`. |
| Footnotes (after the body, only if any definition is rendered) | `<section class="footnotes">`, `<ol>`, then for each N in order: `<li id="md-fn-N" dir="auto">` + content + one back-link per reference K, separated by U+0020 (`<a data-anchor="md-fnref-N-K" class="fn-back">↩</a>`, where the text is `↩` for K = 1 and `↩K` for K > 1) + `</li>`, then `</ol></section>` |
| Footnote definition written inside a spoiler (its `Start` lies between a spoiler's `Start` and `End`) | It keeps its place in the footnotes section, but its content is wrapped as in a spoiler: `<li id="md-fn-N" dir="auto">` `<details class="spoiler">` `<summary dir="auto">Spoiler</summary>` content `</details>` + back-links + `</li>`, so the footnotes section does not reveal it (`footnotes.json#definition-inside-spoiler`) |
| Media | §6.2 |
| Gallery | `<div class="gallery">` + one item per line + `</div>` |
| Truncation (`flags.truncated`) | `<p class="notice" dir="auto">[content truncated: too many elements]</p>`, after the body and before the footnotes (a `p`, so it gets `dir="auto"` like every other) |

**Links.** External links always carry `rel="nofollow ugc noopener noreferrer"`. Removing `nofollow` for high-karma authors (a ForumMagnum feature) is not planned (Never, `docs/plan.md` §6.5): it would make rendering depend on the fold. Internal, anchor and mention links have no `rel`.

**Direction.** The root gets `dir="auto"`, and so does every `p`, `h1`–`h6`, `li`, `th`, `td`, `summary` and `figcaption`. The SPA stylesheet sets `unicode-bidi: isolate` on them, and wraps names in `<bdi>` outside rendered content.

**Sidenotes.** The footnote markup carries everything a client needs to show footnotes as sidenotes: reference K of footnote N is `sup#md-fnref-N-K`, definition N is `li#md-fn-N`, and its back-links are the `a.fn-back` children of that `li`. Every rendered definition's first reference `md-fnref-N-1` is in the HTML, also when the limit stage flattened the block around it (§8.4 keeps references), except when it sat inside a flattened spoiler or after an output-budget cut. On wide screens the SPA MAY copy each definition's content, without its back-links and ids, next to `sup#md-fnref-N-1` (a runtime DOM change, §8.2) and hide the footnotes it copied; a definition with no `md-fnref-N-1` in the root MUST stay visible in the footnotes section.

### 8.2 Allowlist

Nothing outside this table is ever emitted. Every `class` attribute of an HTML element holds exactly one token; a MathML `class` holds the sorted token list of §7.3.

| Element | Attributes, in serialisation order | Allowed values |
|---|---|---|
| `div` | `class`, `dir` | `lfm` (with `dir="auto"`), `alert-note`, `alert-tip`, `alert-important`, `alert-warning`, `alert-caution`, `gallery` |
| `p` | `class`, `dir` | `class="notice"` (optional); `dir="auto"` |
| `h1`…`h6` | `id`, `dir` | `md-h-[\p{L}\p{N}_-]*`; `auto` |
| `li` | `id`, `dir` | `md-fn-[1-9][0-9]*` (footnotes only); `auto` |
| `ol` | `start` | `0` or `[1-9][0-9]{0,9}` |
| `blockquote` `ul` `table` `thead` `tbody` `tr` `figure` `pre` `em` `strong` `del` `hr` `br` | – | |
| `th`, `td` | `class`, `dir` | `ta-l`, `ta-c`, `ta-r` (optional); `auto` |
| `input` | `type`, `checked`, `disabled` | `checkbox`; `""` (optional); `""` (required) |
| `code` | `class` | `language-[a-z0-9_+-]{1,32}` (only as the child of `pre`) or `math-error` |
| `sup` | `id` | `md-fnref-[1-9][0-9]*-[1-9][0-9]*` |
| `a` | `href`, `data-at-uri`, `data-did`, `data-anchor`, `class`, `rel` | Exactly the shapes in §8.1. `href` matches `^(https?://\|mailto:\|#/at/\|#/u/)`. `class` ∈ {`at-link`, `mention`, `ext-image`, `fn-back`}. `rel` = `nofollow ugc noopener noreferrer`. `data-anchor` matches `[\p{L}\p{N}_-]{1,512}`. |
| `details` | `class` | `spoiler` (optional) |
| `summary`, `figcaption` | `dir` | `auto` |
| `img` | `data-cid`, `alt`, `width`, `height` | `b[a-z2-7]{1,127}`; text; `[1-9][0-9]{0,7}` |
| `span` | `class`, `data-cid`, `data-tex` | `media` (with `data-cid`), `media-missing`, `link-host`; `data-tex` ∈ {`inline`, `block`} only with no `class` (Temml mode) |
| `section` | `class` | `footnotes` |
| MathML | §7.3 | The elements, attributes and values of §7.3, including `class` (the closed `mc-*` and engine tokens) and `mathcolor` (`#` and six lower-case hexadecimal digits) |

**Never emitted:** `style`, `src`, `srcset`, `target`, `title`, `name`, `form`, `on*`, `xmlns*`, `xlink:*`, and the elements `svg`, `iframe`, `video`, `audio`, `source`, `object`, `embed`, `script`, `style`, `template`, `base`, `meta`, `link` and `mark`.

Runtime DOM changes the SPA makes after sanitisation are not renderer output. These are setting `src` on a verified `img`, replacing `span.media`, the Temml rendering, substituting verified handles into mentions (§5.4), copying footnotes as sidenotes (§8.1), and accessibility attributes on anchor links and task checkboxes.

### 8.3 Serialisation

The serialisation is chosen so that `DOMPurify.sanitize(html, LFM_CONFIG) === html` and `ammonia(html) == html`. Both are asserted over every vector.

**Escaping:**
- **Text:** `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, U+00A0→`&nbsp;`.
- **Attribute values:** always double-quoted, with `&`→`&amp;`, `"`→`&quot;`, `<`→`&lt;`, `>`→`&gt;`, U+00A0→`&nbsp;`. This follows the current HTML serialiser; the vector runner pins the browser and jsdom versions.
- **Both:** `"\r\n"` and a lone `"\r"` → `"\n"`, U+0000 → U+FFFD. Content never carries them after pass 0, but record fields (`media.alt`, `media.name`) may, and the HTML parser's input preprocessing would rewrite them (`serialise.json#media-alt-cr-nul-normalised`).
- **`alt` values** are trimmed (§6.2), because DOMPurify trims attribute values.

**Tags:**
- Boolean attributes are written `disabled=""` and `checked=""`.
- Void elements `br`, `hr`, `img` and `input` have no `/`.
- Element and attribute names are lowercase.
- Attribute order is as in §8.2 (MathML: sorted).

**Newlines.** The writer emits exactly one `"\n"` in each of these places, and none elsewhere (apart from `SoftBreak` and text content):
- after the start tag of `div`, `blockquote`, `details`, `ul`, `ol`, `section`, `table`, `thead`, `tbody`, `tr` and `figure`;
- after the end tag of `p`, `h1`–`h6`, `blockquote`, `details`, `summary`, `div`, `ul`, `ol`, `li`, `pre`, `section`, `table`, `thead`, `tbody`, `tr`, `th`, `td`, `figure` and `figcaption`;
- after `<hr>` and `<br>`;
- after each figure's media element;
- after each gallery item.

### 8.4 Limit stage: nesting depth and leaves

The limit stage turns S6 into SL. Both writers consume SL, so HTML and `StyledDoc` flatten identically.

**Depth.** d is the number of open HTML elements around the current position, not counting the root `div`. d MUST NOT exceed 64.

**Cost** is the number of elements a tag opens:

| Tag | Cost |
|---|---|
| Paragraph, Heading, BlockQuote/Alert/Spoiler/Details, List, Item, TableHead (`tr`), TableRow, TableCell, Emphasis, Strong, Strikethrough, Link, Figure, Summary | 1 |
| CodeBlock (`pre` + `code`) | 2 |
| Table (`table` + `thead`/`tbody`) | 2 |
| Link dropped by the URL policy (it opens no element, so it is never dropped by this stage either) | 0 |
| Image | a leaf: its subtree (the alt events) is copied unchanged and never depth-checked |

**Reserve** is the room a container needs for its required children:

| Tag | Reserve |
|---|---|
| List | 1 (`li`) |
| Table | 2 (`tr` + cell) |
| Figure | 1 (`figcaption`) |
| Details/Spoiler | 1 (`summary`) |
| everything else | 0 |

Footnote-definition content starts at d = 3 (`section`, `ol`, `li`), wherever the definition appears in the source, and at d = 4 for a definition written inside a spoiler (its `details` wrapper, §8.1). The stage runs once over the body and once over each rendered definition.

```
on Start(tag) at depth d:
  if d + cost + reserve ≤ 64: admit (d += cost)
  elif tag is inline (Emphasis, Strong, Strikethrough, Link):
      drop the Start and its matching End; the content stays (a dropped Link renders as a dropped link)
  elif tag ∈ {Item, TableHead, TableRow, TableCell, Summary}: admit   // always fits: the parent reserved room
  else:                                 // block: Paragraph, Heading, CodeBlock, BlockQuote kinds, List, Table, Figure
      replace the whole subtree with flat(subtree); if flat(subtree) is empty, emit nothing;
      otherwise wrap it in Paragraph if d + 1 ≤ 64, else emit it bare
      (the parent is then always a flow container: blockquote, div, details, li or the root)
      set flags.depth_limited
flat(subtree) = in order: its Text, Literal and Code payloads as Text, and its math as Text(TeX source);
                its images and FootnoteReferences unchanged (leaves: used media and every footnote's
                references stay visible); a figure's caption as Text after its image;
                SoftBreak at every block boundary, SoftBreak, HardBreak and Rule, with no repeats and
                none at either end; TaskListMarker dropped;
                a Spoiler contributes only the fixed summary Text("Spoiler"), between block boundaries,
                and nothing of its content: flat markup could not hide it (`limits.json#spoiler-flattened-to-summary`)
```

**Leaves** are exempt from the limit: `img`, `input`, `br`, `hr`, inline `code`, `sup>a`, `span.*`, `a.ext-image` (an `a` that may contain one `span`), and `math`. A leaf adds at most 2 levels; MathML adds at most 64 (§7.3). So the DOM depth stays below 132, well under browser parser limits.

**Event cap.** §3 step 1.

**Output budget.** The input is capped, but some constructs still expand it many times: escaping (`&` → `&amp;`), fixed markup around short syntax (`[^1]` becomes a `sup` and a back-link), and record fields such as `media.alt`. After the limit stage, an upper-bound estimate of the HTML of each SL event is summed over the body and then over each rendered footnote (the gallery is reserved first). Where the sum would pass 48 MiB, that flow is cut: the event and everything after it are dropped, every open tag is closed innermost first, the remaining footnotes are dropped, and `flags.truncated` is set, exactly as when the event cap truncates (§3 step 1). Both writers consume the cut SL. The estimate is presentation: it MUST bound the HTML the writer produces (a unit test checks this), and real documents stay far below it (1 MB of plain prose is estimated at about 6 MB). MathML is budgeted separately by the HTML writer (§7). The budget never changes an `lfm/1` value.

### 8.5 Sanitiser obligations

- **SPA.** Rendered HTML is inserted only through the Trusted Types policy `lessdugin-md`, which runs DOMPurify. The DOMPurify configuration is **generated from §8.2 and §7.3** (`allowlist::dompurify_config_json()`) and contains nothing else:
  - `ALLOWED_TAGS` and `ALLOWED_ATTR` exactly as listed;
  - `ALLOW_DATA_ATTR: false` and `ALLOW_ARIA_ATTR: false`;
  - `ADD_URI_SAFE_ATTR`: every allowlisted attribute except `href`, so that DOMPurify's URI check does not drop `dir`, `rel`, `id`, `alt` and the like;
  - `SAFE_FOR_XML: false`: its pattern removes any attribute whose value contains `-->`, `]]>` or `</title` (and similar end tags), which an `alt` legitimately may. The renderer escapes `<` and `>` in every attribute value, so the mXSS that option guards against cannot arise (`xss/html.json#*-safe-for-xml-patterns`);
  - `ALLOWED_URI_REGEXP` = `^(?:https?:|mailto:|#/at/|#/u/)`;
  - a hook that enforces the class tokens and value grammars.
- **Native.** HTML output (`read --html`) also runs ammonia (feature `ammonia`), configured from the same tables; it allows relative URLs only with the `#/at/` and `#/u/` prefixes the renderer emits.
- **Tests.** The vectors assert equality for both sanitisers; any difference is a renderer bug. `ammonia(html) == html` runs in the Rust runner (`--features ammonia`). DOMPurify runs over every `html` and `html_temml` of `lfm1/`, `xss/` and `mathml/` twice: in jsdom (`web/src/lib/sanitize.vectors.test.ts`), compared with jsdom's own re-serialisation because jsdom writes `<` and `>` in attribute values unescaped, and in Chromium with exact equality (`web/e2e/vectors.spec.ts`).
- **Poison pills.** Native code wraps each record's render in `catch_unwind`. The SPA worker records `(uri, cid, RENDERER_VERSION)` before it renders, and quarantines the record as "failed to render" after a crash (synthesis §5.2 F8).

### 8.6 Fixed strings (English in v1; presentation layer; never part of P)

| Key | Text |
|---|---|
| Spoiler summary | `Spoiler` |
| Details default summary | `Details` |
| External image label prefix | `image: ` |
| Footnote back-link | `↩` (U+21A9), plus the decimal K for K > 1 |
| Truncation notice | `[content truncated: too many elements]` |
| `BodyError::TooLarge` | `[body too large to render]` |
| `BodyError::Encoding` | `[unreadable body: not valid UTF-8]` |
| Terminal: spoiler hidden | `[spoiler hidden]` |
| Terminal placeholders | `[image N: A]`, `[video N: L]`, `[audio N: L]`, `[file N: L]`, `[missing media: A]`, `[image: A ⟨host⟩]` |

---

## 9. Terminal contract

### 9.1 `TermText` and `term_sanitize`

**Type.** `TermText` is a newtype in `lessdugin-markdown`, a sequence of runs `{text, kind: Plain | Escaped}`. Only `term_sanitize` can construct it.

It is the only text type accepted by:
- `StyledDoc` spans;
- the ratatui adapters;
- the CLI printers;
- the sanitising `tracing` writer;
- error, `resolve` and `doctor` output.

Nothing relies on filtering done by ratatui or `less` (F1).

`term_sanitize(input, ctx)` has three contexts:
- **Prose**: text, literal HTML, details summaries, error messages.
- **Code**: code blocks, inline code, math TeX, `--raw` lines.
- **Name**: display names, handles, titles, forum names, tag names, react labels, `media.name`.

Callers split multi-line input into lines on `"\n"` (after normalising CRLF and CR) before they call it.

| Input code point | Prose | Code | Name |
|---|---|---|---|
| White_Space (U+0009–U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000–U+200A, U+2028, U+2029, U+202F, U+205F, U+3000) | TAB, LF → U+0020. CR → `^M` (Escaped). VT, FF → `^K`, `^L` (Escaped, as other C0). U+0085 → `\u{85}` (Escaped). U+2028/U+2029 → U+0020. The rest (U+0020, U+00A0, U+1680, U+2000–U+200A, U+202F, U+205F, U+3000) are kept. | TAB → U+0020 up to the next column that is a multiple of 4 (column = display width so far on the line). LF → U+0020. CR, VT, FF, U+0085, U+2028, U+2029 as Prose. | Every one → U+0020; then runs collapse to one U+0020 and the result is trimmed. |
| Other C0 (U+0000–U+001F) | `^` + chr(c + 0x40) (Escaped; ESC → `^[`) | same | same |
| U+007F | `^?` (Escaped) | same | same |
| C1 (U+0080–U+009F) | `\u{xx}`, lowercase hex (Escaped) | same | same |
| Bidi controls (U+061C ALM, U+200E LRM, U+200F RLM, U+202A LRE, U+202B RLE, U+202C PDF, U+202D LRO, U+202E RLO, U+2066 LRI, U+2067 RLI, U+2068 FSI, U+2069 PDI) | removed | `⟨NAME⟩` (Escaped) | removed |
| Zero-width (U+200B ZWSP, U+200C ZWNJ, U+200D ZWJ, U+2060 WJ, U+FEFF BOM) | kept | `⟨NAME⟩` (Escaped) | removed |
| `BADGE_GLYPHS` = {U+2611, U+2612, U+2705, U+2713, U+2714, U+2715, U+2716, U+2717, U+2718, U+26A0} | kept | kept | removed |
| Grapheme cap | – | – | `Name{max}`: if longer than `max` extended grapheme clusters, keep `max − 1` and append `…` |

The resulting invariant: a `TermText` never contains a code point in C0 (TAB and LF included), U+007F, C1, U+2028, U+2029 or the bidi set (`term-sanitize.json`).

**JSON form** (vectors, `--json`): a plain string when every run is `Plain`, otherwise an array `[{"plain": text} | {"escaped": text}, …]`.

**Badge rendering.** Clients MUST render badges (§1.2 of the synthesis) only in the fixed badge column and style, and only as the ASCII bracketed words or glyphs from `BADGE_GLYPHS`.

### 9.2 `StyledDoc`

```rust
pub struct StyledDoc { pub blocks: Vec<SBlock> }                // flat, document order
pub struct SBlock    { pub gutters: Vec<Gutter>, pub kind: SKind } // gutters: outermost first, ≤ 64 (from §8.4)
pub enum Gutter { Quote, Alert(AlertKind), Spoiler(u32), Details(u32),
                  Item { ordinal: Option<u64>, first: bool }, Footnote(u32) }
pub enum SKind {
  Para(Vec<SLine>),                      // prose; adapters re-wrap each SLine to the width
  Heading { level: u8, id: String, line: SLine },
  Code { lang: Option<TermText>, lines: Vec<TermText> },   // never re-wrapped by the builder
  Math(Vec<TermText>),                   // display-math TeX lines
  Table { aligns: Vec<Align>, head: Vec<SLine>, rows: Vec<Vec<SLine>> },
  Rule,
  Summary { id: u32, spoiler: bool, line: SLine },
  Media { index: u16, line: SLine },     // block-level media (figure, lone image, gallery item)
  Caption(SLine),
  Notice(TermText),
}
pub type SLine = Vec<Span>;
pub struct Span { pub text: TermText, pub style: Style, pub link: Option<SLink> }
pub enum SLink { External { url: String, host: String }, Record(AtUri), Profile(Did),
                 Mention(Did), Anchor(String), Media(u16) }
bitflags Style { EMPH, STRONG, STRIKE, CODE, MATH, LINK, MENTION, FOOTREF, TASK, PLACEHOLDER, NOTICE }
```

`Gutter::Footnote(N)` is on every block of footnote N and carries no `first` flag: a printer that marks a footnote's first block (`[N] `, §9.4) treats the first block with that gutter, after a block without it or with another N, as the first.

### 9.3 Construct mapping

| Construct | `StyledDoc` |
|---|---|
| Paragraph | `Para`. A `HardBreak` starts a new `SLine`; a `SoftBreak` becomes a `" "` span. |
| Heading | `Heading{level, id, line}` |
| BlockQuote / Alert | the `Quote` or `Alert(kind)` gutter on the contained blocks |
| Spoiler / Details | a `Summary{id, spoiler}` block (text `Spoiler`, or the summary, or `Details`), then the content blocks with the `Spoiler(id)` or `Details(id)` gutter. Ids are 1, 2, … in document order, one counter shared by spoilers and details. |
| List / Item | the `Item{ordinal, first}` gutter. `first` is true only on the item's first block. `ordinal` is the list start + the item index, or `None` for bullets. An empty item emits one empty `Para`, so that it still shows its bullet. |
| TaskListMarker | span `[x] ` or `[ ] ` with `TASK` |
| CodeBlock | `Code`. The text is split on `"\n"`, a final empty line is dropped, and each line is `term_sanitize(Code)`. `lang` is the class token of §8.1 (none when the HTML has no class). |
| Code | span `CODE`, Code context |
| InlineMath | span `MATH` with the text `"$" + tex + "$"`, Code context |
| DisplayMath | closes the current `Para`, emits `Math(lines of tex)` (leading and trailing empty lines dropped), and continues the paragraph in a new `Para`; a `SoftBreak` next to it adds no space. Inside a heading, summary or table cell, which are single lines, it is an inline span `MATH` with the text `"$$" + tex + "$$"`. |
| Text / Literal | spans, Prose context |
| HardBreak | in a heading, summary or table cell: a `" "` span |
| Emphasis / Strong / Strikethrough | style flags, combined |
| Link kinds | spans with `LINK` and `SLink`. A dropped link becomes plain spans. A mention is the span `@short_did` with `MENTION`. |
| FootnoteReference | span `[N]` with `FOOTREF` and `Anchor("md-fn-N")` (inside a link, the link's `SLink`) |
| Footnote definitions | after the body: blocks with the `Footnote(N)` gutter, in ordinal order. A definition written inside a spoiler (§8.1) starts with a spoiler `Summary` block, and its content blocks carry `Footnote(N)` then `Spoiler(id)`, so neither the TUI nor the printer reveals it. |
| Image, inline | span with `PLACEHOLDER` (§8.6) and `SLink::Media` or `External`. A standalone external image is `[image: A ⟨host⟩]` (`[image ⟨host⟩]` without alt); inside a link it is `[image: A]` carrying the outer link, without the image host, as in HTML. |
| Figure, a paragraph containing only one `cid:` image, a gallery item | `Media{index, line}` (+ `Caption`). `media.name` in a label uses the Name context. |
| Table | `Table` |
| Rule | `Rule` |
| Truncation / `BodyError` | `Notice` |

### 9.4 Printer obligations

**Escape sequences.** Our printers emit only two kinds of escape sequence:
- **SGR**, `ESC [ p(;p)* m`, with every p in {0, 1, 2, 3, 4, 7, 9, 22, 23, 24, 27, 29, 39, 30–37, 90–97};
- **OSC 8**, `ESC ] 8 ; ; URI ESC \`.

They never emit title sequences (OSC 0/1/2), OSC 52 or cursor movement inside content. `Escaped` runs print in reverse video (SGR 7).

**OSC 8.**
- Emitted only for `SLink::External` (`External` and `Mailto` from `safe_href`), and only when color is enabled and stdout is a TTY.
- In URI, every byte outside 0x21–0x7E, and `;`, is percent-encoded.
- If the encoded URI is longer than 2,048 bytes, there is no OSC 8 (the text is still printed).
- SGR and OSC 8 open once per run of equally styled text on an output line and close at its end, never per word or grapheme (`terminal/escapes.json#osc8-once-per-line`, `#osc8-and-sgr-per-line`).

**Real host (F1).** After the link text, print a dimmed ` ⟨host⟩` (U+27E8, U+27E9) unless `norm(text) == norm(url)`. `norm` strips one `https://`, `http://` or `mailto:` prefix, one `www.`, and one trailing `/`, then ASCII-lowercases. Under `--color never`, links print as `text ⟨host⟩` (plain). A link split into several styled spans gets one suffix, after its last span; there is none when the link text already contains exactly `⟨host⟩` (the standalone external-image placeholder, §9.3).

**Mentions.** Shown as in §5.4.

**Spoilers.**
- Spoiler content is not displayed until the user explicitly reveals it.
- TUI: hidden behind its `Summary` line, which a toggle key reveals.
- CLI: one line `[spoiler hidden]` replaces the summary and the content, unless the user passed an explicit reveal option (defined in the CLI spec).
- Details: TUI collapsed by default; CLI expanded.

**Gutters** (plain stdout): `│ ` for a quote or alert, `┆ ` for spoiler content, two spaces for details content, `• ` or `N. ` on an item's first block (spaces of the same width on its later blocks), and `[N] ` on a footnote's first block. At most 32 columns of gutters print: a deeper block shows `… ` followed by its innermost gutters (`terminal/escapes.json#deep-gutters-elided`).

**Bounds.** Printed output stays proportionate to the input:
- a table column is at most 40 columns wide (or the terminal width, if smaller); longer cells wrap within the column, and when padding would still exceed 8 × the content + 4,096 bytes, the rows print unaligned (`#table-*`);
- OSC 8 wrappers stop after 1 MiB of OSC 8 bytes per document; the text still prints (`#osc8-*`);
- a `media.alt` fallback prints once per item, as in HTML (§6.2; `#media-alt-amplification-*`).

**Images.**
- TUI: a `Media` block MAY be drawn with ratatui-image if all of these hold:
  - the type is an inline image type;
  - the bytes are CID-verified;
  - the autoload policy allows it, or the user clicked through;
  - decode limits hold: 64 MB, 8192² pixels, 200 GIF frames, decoded off the UI thread.
- Otherwise the placeholder is shown.
- CLI: always the placeholder.

**Layout.** Display width is `unicode-width` 0.2 `width()` (not CJK width). Prose wraps at U+0020, or at a grapheme boundary when one word is longer than the line. Snapshot vectors use 80×24 and 160×48.

**`--json` and `--raw` on a TTY:**
- `--json`: every C1 code point, U+2028, U+2029 and bidi control is written as `\uXXXX`. serde_json already escapes C0.
- `--raw`: each line passes through `term_sanitize(Code)`.
- When piped (not a TTY), both are byte-exact.

---

## 10. Plain-text projection P and derived text

### 10.1 Event table

P is computed on **S3**: after pass 0 and passes 1–3, before mentions and the URL policy. So P does not depend on handle resolution, the URL policy, rendering limits, the math engine or viewer settings.

| S3 event | Appended to the buffer |
|---|---|
| `Text(t)`, `Literal(t)`, `Code(t)`, `InlineMath(t)`, `DisplayMath(t)` | t (the TeX source for math, without delimiters) |
| `Start`/`End(Link)` of any kind, mentions included | nothing (the link's source inner events contribute; URLs never do) |
| `Start`/`End(Image)` | nothing (inner events, the alt text, contribute) |
| `End(Figure{caption})` | `"\n"` + caption + `"\n"` |
| `Start` and `End` of `Paragraph`, `Heading`, `BlockQuote` (any kind), `Spoiler`, `Details`, `Summary`, `CodeBlock`, `List`, `Item`, `FootnoteDefinition`, `Table`, `TableHead`, `TableRow`; `Start(Figure)`; `Rule` | `"\n"` |
| `Start`/`End(TableCell)` | `" "` |
| `SoftBreak` / `HardBreak` | `" "` / `"\n"` |
| `Start`/`End` of `Emphasis`, `Strong`, `Strikethrough` | nothing |
| `FootnoteReference`, `TaskListMarker` | nothing |

The table also defines `plain(events)`: the same rules over a sub-sequence, with no normalisation. It is used for alt text, the host test and heading text.

Footnote definitions contribute at their source position, even when unreferenced. Removed spoiler and details markers contribute nothing, because pass 2 deleted them.

### 10.2 Normalisation

1. Remove controls: every code point of general category `Cc` that is not White_Space, and the bidi controls U+061C, U+200E, U+200F, U+202A–U+202E and U+2066–U+2069. They would otherwise reach excerpts, search and quote keys, where they can reorder or hide text.
2. Unicode NFC (`unicode-normalization`; vectors pin the Unicode version).
3. Replace every maximal run of White_Space code points (the 25-code-point set in §9.1) with one U+0020.
4. Strip one leading and one trailing U+0020.

**Offsets** into P are counted in Unicode scalar values (code points), never in UTF-16 units or bytes.

### 10.3 P_x (projection for excerpts)

P_x is P computed with every event strictly between `Start(Spoiler)` and its matching `End(Spoiler)` contributing nothing. The spoiler's own `Start` and `End` still contribute `"\n"`. Excerpts therefore never reveal spoilers.

### 10.4 Excerpt

```
excerpt(X = P_x):
  if graphemes(X) ≤ 500 and bytes(X) ≤ 5000: return X
  head = longest prefix of whole grapheme clusters with ≤ 499 clusters and ≤ 4997 bytes
  k = index of the last U+0020 in head
  if k exists and k > 0: head = head[..k]
  return head + "…"            // U+2026; the result is ≤ 500 graphemes and ≤ 5000 bytes (fits post.excerpt)
```

Graphemes are extended grapheme clusters (`unicode-segmentation`).

Writers MUST fill `post.excerpt` with this value when the body is spilled (`data-model.md` §5.3). Readers display a stored excerpt as plain text (`TermText` Prose, or React text).

### 10.5 Word count and read time

- `word_count` = the number of maximal runs of non-U+0020 code points in P: 0 if P is empty, otherwise (the number of U+0020 in P) + 1. It is informational only.
- `read_minutes` = 0 if `word_count` = 0, otherwise `max(1, (word_count + 125) / 250)`, using integer division (250 words per minute).

### 10.6 Heading text and ToC

- Heading text = the normalisation of §10.2 applied to `plain(heading inline events)`.
- `toc` lists, in document order, every heading of S6 that is not inside a `BlockQuote` (alerts included), `Spoiler`, `Details`, `List`, `Table` or `FootnoteDefinition`, as `{level, id, text}`, where `id` is the heading id of §4.6. Clients choose how many levels to show, and link each entry to its heading with the anchor rule of §4.6.
- Slugs use the un-collapsed form (§4.6).
- The ToC is `lfm/1`: the limit stage and the output budget never change it. A top-level heading sits at depth 1, so the limit stage never flattens it; after an output-budget cut (§8.4), entries past the cut have no element in the HTML, and resolving their anchor does nothing.

---

## 11. Quote resolution

This covers `comment.quote` and `#react.quote`. It is deterministic. No fuzzy matching ever happens in `lfm/1`: fuzzy or historical re-anchoring would be `lfm/2` or a display-only lens.

**Target:**
- for a comment: its `parent` if present, otherwise its `root`;
- for a react: `opinion.subject`.

**Text.** T = P of the target's **current** version's body.
- A post's title is not included.
- A target with no body has T = `""`.
- A target whose body is pending (§1.3) gives `Pending`.

```
resolve(quote, T) -> Anchored{start, end} | Orphaned | Pending          // code-point offsets in T
  q  = norm(trim_ws(quote.exact))  ; norm(x) = remove §10.2 controls, NFC, then White_Space runs → U+0020
  pf = norm(quote.prefix or "")    ; sf = norm(quote.suffix or "")    // not trimmed
  if q == "" or bytes(q) > 2000: return Orphaned
  T' = longest prefix of T of ≤ 200,000 bytes that ends on a code-point boundary
  O  = every offset o (overlapping allowed) with T'[o .. o+len(q)] == q, ascending; keep only the first 1,000
  if O is empty: return Orphaned
  if |O| == 1: o = O[0]
  else:
    score(o) = lcs_suffix(pf, T'[..o]) + lcp_prefix(sf, T'[o+len(q)..])     // in code points
    o = argmax score; tie → min |o − quote.start| (skipped if start is absent); tie → min o
  return Anchored{start: o, end: o + len(q)}
lcs_suffix(a, b) = length of the longest common suffix of a and b
lcp_prefix(a, b) = length of the longest common prefix of a and b
```

**Search.** Both strings are NFC UTF-8, so a byte-level substring search (for example `memchr::memmem`) finds exactly the code-point matches. Implementations MUST run in O(|T'| + |q| + |O|·(|pf| + |sf|)).

**Display.** `Orphaned` is shown as "quoted text no longer present". `Pending` is shown as loading. Neither is an error.

**Reacts** (`social.md` §9.2):
- `qkey(quote) = norm(trim_ws(quote.exact))`, in that order, or `""` when there is no quote. Reacts group by `(react, qkey)`, never by a resolved range, so the fold never depends on whether a body blob has been fetched.
- Anchors are display-only: the creator's quote of each group is resolved as above, and `Anchored{start, end}` highlights that range; `Orphaned` and `Pending` show as described under **Display**.

**Writers** (`data-model.md` W8):
1. Compute T from the version the user saw.
2. Take the user's selection, trimmed to start and end on a non-space, as `exact`. It must be 1–2,000 bytes.
3. `prefix` = up to 32 grapheme clusters of T immediately before `exact`. `suffix` = up to 32 immediately after. Drop clusters from the far end until each is ≤ 640 bytes. Both are verbatim substrings of T.
4. `start` = the code-point offset of `exact`.

**CLI `--quote "…"`:** normalise the text as q; if it has no occurrence in T, fail; otherwise use the first occurrence.

---

## 12. Derived metadata and writer obligations

**Scope.** `links` and `mentions` are collected from the **rendered content** of S6: the body, then the rendered footnote definitions in ordinal order (§4.6), skipping the events inside an image (a link in an alt is never shown as a link). A link or mention in an unreferenced or duplicate footnote definition is never shown, so it must not notify anyone or count as a pingback (`at-links.json#unreferenced-footnote-not-derived`, `#link-in-image-alt-not-derived`). Being `lfm/1` values, they do not depend on the limit stage or the output budget: a link flattened by the limit stage still shows its text and still counts (`at-links.json#depth-flattened-still-derived`). Links inside a spoiler count, since the spoiler can be revealed.

**`links`.**
- The canonical at-uris of every `Record` link in the rendered content, of any collection, deduplicated, in order of first occurrence, capped at 256.
- Profile links and mentions are not included.

**`mentions`.**
- The DIDs of every mention link (§4.4) in the rendered content, deduplicated, in order of first occurrence, capped at 128.

**`anchors`** (presentation).
- Every `id` emitted in the HTML, in document order: so after the limit stage (a flattened heading has no id) and the output budget. The HTML writer yields it.

**Writers.**
- `markdown.mentions` = `Rendered.mentions`, and `markdown.links` = `Rendered.links`, both rendered from the exact text being published (`derive_metadata`, §1.6, computes them without the HTML writer).
- Before publishing, writers rewrite handle-authority `at://` link destinations to DIDs. The editor's mention autocomplete inserts `[@handle](at://did)`.
- Writers set `profile: "lfm/1"`.
- They spill per `data-model.md` §5.3, and fill the excerpt per §10.4.
- Draft-local image paths become `cid:` URLs at publish time (CLI spec; F3).

**Readers** never use the record's `mentions` or `links` fields for rendering, notifications or the fold.
- **Mention notifications** and **pingbacks** are computed from `Rendered.mentions` and `Rendered.links` of synced (`pds` or `proven`) records.
- Pingbacks consider only targets in our collections.
- External backlink indexes, and the indexer's `listBacklinks`, are hints only.

**Search.** The core's in-memory search index and the indexer index P. Snippets shown to readers are plain text cut from P_x (`derive_metadata(..).text_x`), with highlight ranges in P_x offsets, so that they never reveal a spoiler.

---

## 13. Test vectors (`testdata/vectors/lfm1/`)

**Format.** Each file has the common vector-file shape of `docs/spec/README.md` §5.1 (`{behaviour: "lfm/1", kind, description, vectors}`); each vector is `{name, input, expected}`. `testdata/vectors/README.md` documents every `kind`, its `input` and `expected` shapes, and the derived fields.
- `input` is `{text | gen, media?, profile?, encoding_error?}` for render vectors (`gen` = `prefix + repeat × times + suffix`, for large inputs), plus `print` for `terminal` vectors, or the arguments of the function under test.
- `expected` holds only the fields the vector asserts: any `Rendered` field (`html`, `styled`, `text` (P), `excerpt`, `word_count`, `read_minutes`, `toc`, `links`, `mentions`, `media_used`, `anchors`, `flags`, `error`), or a derived field: `html_temml`, `text_x` (P_x), `html_head`, `html_tail`, `html_rle`, `html_bytes`, and for `terminal` vectors `printed`, `printed_bytes`, `printed_rle`.

**Runners.** One Rust runner, `crates/markdown/tests/vectors.rs`, runs every file natively (`cargo test -p lessdugin-markdown`, and `--features ammonia` for the ammonia equality) and on wasm32 under node (`cargo xtask test-wasm`), where it embeds the same files; both runs MUST pass, which proves native = wasm. The SPA's DOMPurify runner is described in §8.5. Inputs are generated by `testdata/vectors/lfm1/gen_inputs.py` (which keeps the expected values of unchanged inputs); `LESSDUGIN_BLESS=1 cargo test -p lessdugin-markdown --test vectors` rewrites the expected values (`LESSDUGIN_BLESS_ONLY=dir/file.json,…` limits it to some files), and the diff is reviewed. Vectors are never edited by hand. M4 requires at least 150 `lfm1` vectors; the runner fails below that.

| File | Covers |
|---|---|
| `options.json` | each enabled extension; each disabled one rendering literally (smart punctuation, heading attributes, YAML and `+++` blocks, definition lists, `^sup^`, `~sub~` as strikethrough, `[[wikilinks]]`, old-footnote syntax) |
| `raw-html.json` | HTML blocks of every CommonMark type, inline HTML, comments, `<script>`/`<style>`, entities, multi-line inline HTML, autolinks not applied to literals |
| `normalise.json` | CR, CRLF and NUL in the source (step 0) and from character references (step 3) in every payload kind, text coalescing, the event cap (exactly 2^20 events, and 2^20 + 1 with tag closing), dangling footnote references after truncation |
| `autolink.json` | `www.`/`http://`/`https://`, letter case, boundaries (`*_~(`, start of event), trailing punctuation (including a sentence-final `.` and trailing dots before a path), `;` entities, unbalanced `)`, `<`, `_` in the last two segments, no period, a domain longer than 253, inside links, images, code and literals |
| `spoiler-details.json` | marker case, a rest of line that is not blank, a marker as a whole paragraph, an empty summary, an inline-formatted summary, nesting (also inside alerts and list items), alerts untouched, a marker made into a link by a reference definition, P and P_x |
| `figure.json` | title and no title, extra text, two images, an image in a link, a whitespace-only title, tight and loose list items, a `cid:` miss and a non-image media item as the figure's media |
| `mention.json` | did:plc, did:web, other methods, handle authority, text not starting with `@`, formatted inner text, the inner text replaced in HTML but kept in P, the 128 cap, mentions in summaries and spoilers, `short_did` |
| `links.json` | every `safe_href` branch: schemes, relative, `//host`, userinfo, backslash, controls, bidi, non-ASCII host, hosts the WHATWG URL parser rejects or rewrites (IPv4 forms, malformed IPv6, Punycode), path percent-encoding, a port, IPv6, the 3,000-byte cap, `mailto`, email autolinks, fragments (charset, lowercase, decoding, `#/x`), the host-display rule |
| `at-links.json` | record, profile, collection only, query or fragment, trailing slash, foreign NSIDs, `.temp` NSIDs, invalid rkeys, the canonical `data-at-uri`, the 256 cap, the scope of §12 (unreferenced footnotes, image alts, flattened blocks) |
| `images.json` | `cid:` hit, miss, malformed, duplicate and case; `cid:` as a link destination; each mime class and exact MIME matching; alt fallback (first reference only) and trimming; aspect clamp; external images in and out of links; other schemes |
| `gallery.json` | `media_used` (including images only in unreferenced footnotes or inside an alt), gallery order, duplicates, invalid CIDs, no gallery |
| `ids.json` | slugs (Unicode, punctuation, emoji, NFC, empty), deduplication including a literal `foo-1` collision, ids in containers and footnotes, `anchors` |
| `footnotes.json` | ordinals, multiple references, undefined labels, duplicate definitions, unreferenced definitions and the rendered set, references inside definitions and inside links, nested definitions, case-insensitive labels (`UniCase`), empty definitions, definitions inside spoilers and other containers, references in headings and tables, back-link numbering |
| `blocks.json` | alerts (all kinds, nested, with block content, empty, lazy continuation, in lists), tables (alignment, escaped pipes, a header with no body), task lists, code-language classes, `ol start`, tight and loose lists |
| `math.json` | inline and display math; each disabled control word, including inside `\text{}` and comments; the 4,096-byte and depth-64 boundaries; unknown commands; MathML filtering (style, class, id, href, `semantics`); the output depth and size caps; `html_temml` variants; currency-style `$` |
| `limits.json` | depth flattening for each container kind and the inline drop at the boundary; what `flat()` keeps (images, footnote references, captions) and drops (spoiler content); leaf exemption; footnote base depth (3, and 4 inside a spoiler); `flags`; `TooLarge`; `Encoding` |
| `serialise.json` | escaping (`&<>"`, U+00A0), boolean attributes, attribute order, the newline placement, `dir` |
| `projection.json` | each row of §10.1, NFC, all 25 White_Space code points, trimming, astral-plane offsets, P vs P_x |
| `excerpt.json` | ≤ 500 graphemes, exactly 500 and 501, the byte cap with long clusters, no space, a leading spoiler, `word_count`, `read_minutes`, `toc` (top level only, no headings inside spoilers, details, alerts or tables, text normalisation) |
| `quote.json` | 0, 1 and many occurrences; overlapping occurrences; prefix and suffix scoring; the `start` tie-break with and without `start`; normalisation; the 2,000-byte and 200,000-byte limits; the 1,000-occurrence cap; `Pending`; `qkey` |
| `safe-href.json` | direct `safe_href(raw, ctx)` → `Href` or null, including the `Field`-only `ProfileHandle` and `Anchor` refused in `Field` |
| `term-sanitize.json` | direct `term_sanitize(input, ctx)` for every row of §9.1 (including VT and FF), tab stops, the grapheme cap |
| `styled.json` | `StyledDoc` JSON snapshots for each construct in §9.3 |

**Invariants checked over every render vector** (and `terminal` vector), whatever `expected` lists:
- `DOMPurify(html) === html` in pinned Chromium and jsdom (§8.5), for `html` and `html_temml`;
- `ammonia(html) == html` (feature `ammonia`);
- every `class` token and attribute matches §8.2 or §7.3 (`allowlist::check_html`);
- native output equals wasm output (the same runner on wasm32);
- rendering is idempotent: rendering the same input twice gives byte-equal output;
- `derive_metadata` (§1.6) equals the corresponding `render` fields;
- P and the excerpt contain no whitespace other than U+0020 and no C0, DEL, C1, U+2028/U+2029 or bidi control;
- no `StyledDoc` text contains those characters either;
- printer output, with and without color, contains no escape sequence other than the §9.4 SGR and OSC 8 forms, and none at all without color.

**Related directories:**
- `terminal/` holds the escape corpus and printer snapshots, and asserts that no ESC appears other than the SGR and OSC 8 forms of §9.4.
- `xss/` holds the HTML and MathML XSS and mXSS corpora.
- `mathml/` holds the engine-specific MathML snapshots (`math-core-0.9.json`) and the ADR 0002 S10 corpus and probes (`s10-corpus.json`, `s10-probes.json`), each expression's output or math error recorded, so that a math-core upgrade that changes coverage shows up in review (`gen_mathml.py`). Their runner also asserts that the normaliser met no unknown style (§7.3).

**Fuzzing.** The `render` target in `fuzz/` (cargo-fuzz; run it with `cargo xtask fuzz render`, see `fuzz/README.md`) feeds arbitrary bodies of up to 32 KiB, on a 1 MiB stack. Every input in the directories above is a seed. It checks the invariants above, except DOMPurify, native = wasm and idempotence (the vector runners cover those). It also checks two more properties:
- a quote built from a selection of P resolves to that selection (§11);
- no `term_sanitize` run contains a terminal control (§9.1).

The M4 bar is 10 minutes with no failure, and `.github/workflows/fuzz.yml` repeats the run every night. A failure is fixed in the crate and gets a vector. NOTES bug 22 was found this way.

---

## 14. Decisions made while writing

These resolve ambiguities in the design. Each takes the simplest safe option.

1. **Two layers.** `lfm/1` freezes only data-relevant semantics: syntax, URL policy, P, excerpts, quotes and derived links. The HTML/`StyledDoc` presentation is versioned by `RENDERER_VERSION`, so styling fixes do not need `lfm/2`.
2. **Pass 0** was added before the six synthesis post-passes. It covers source normalisation (line endings and NUL, so that pulldown-cmark's CR quirks never apply), the event cap, raw HTML as `Literal` (block → paragraph), CR normalisation of payloads (needed for DOMPurify round-trip stability), text coalescing, and dangling footnote references.
3. **Autolinks.** `www.` links get `https://`, not GFM's `http://`. Prefixes are case-insensitive. A domain needs a period for `http(s)://` too, and is capped at 253 characters (to bound scan cost). The trailing-punctuation set and the `;`/`)` rules follow cmark-gfm, written in linear form; the domain is validated without the trailing dots that rule drops, so a sentence-final `www.example.com.` links. Every `Text` event start counts as a boundary.
4. **Spoilers** render as `<details class="spoiler">` with the fixed summary `Spoiler`. This is accessible and needs no JS. `[!spoiler]` must be alone on its line, like GFM alerts. `[!details]` takes an optional same-line summary, defaulting to `Details`.
5. **P is computed on S3,** so mentions use their source text and P is independent of handles and settings. **P_x omits spoiler content,** so excerpts never leak spoilers.
6. **Mentions** bake `@short_did` into the HTML. The verified handle is substituted at display time, which keeps the `(cid, RENDERER_VERSION)` cache sound. Only did:plc and did:web are accepted, matching `data-model.md`.
7. **`at://` links in markdown require a DID authority.** Handle authorities drop the link, and writers rewrite them to DIDs. Only `at://did` and `at://did/nsid/rkey` are accepted. `ProfileHandle` is allowed only in `Field` context, for `forum.reportContact`, and becomes navigable after bidirectional verification.
8. **`safe_href`** rejects whitespace, controls, bidi and zero-width characters, backslashes, userinfo and non-ASCII hosts; caps length at 3,000 bytes (the lexicon `url` limit); and percent-encodes non-ASCII elsewhere. Rejecting IDN hosts avoids a punycode dependency and homograph risk; the link text stays visible. Hosts are also limited to those the WHATWG URL parser accepts and keeps (`host_ok`, M4, found by fuzzing). IPv4 is accepted only as a dotted quad and IPv6 only as a valid address. An `xn--` label must be well-formed Punycode: a small decoder checks it, with no Unicode tables, and the label is shown as written. Otherwise a link could open a host other than the one shown (`0x7f.1` is 127.0.0.1), or ammonia would strip its `href`.
9. **External images** always render as the same link in HTML. The per-forum opt-in is a client-side replacement after sanitisation (blob URL, CSP-compatible), so HTML never depends on viewer settings.
10. **Non-image media and misses** use `span.media[data-cid]` and `span.media-missing`. The SPA chooses the player or card from the verified record, so the allowlist stays free of `video` and `audio`. `span` was added to the allowlist; the synthesis already implied it with `span[data-tex]`.
11. **Showing the real host.** HTML uses F9's host-like test, because always appending the host would be noisy. Terminals use F1's rule: show the host whenever the text differs from the URL.
12. **`dir="auto"`** goes on the root, as in the synthesis, and also on text blocks, following F5 ("every container"). This is cheap and better for mixed-direction threads.
13. **Alerts** carry no title text; CSS supplies it. Fixed English strings are confined to the §8.6 table.
14. **Footnotes.** Ordinals follow first reference. The first definition wins. Unreferenced definitions are not rendered (as on GitHub) but stay in P; "referenced" means reachable from the body through rendered definitions (§4.6). Back-links are `↩`/`↩K`. Inside a link, references render without an anchor, because nested `<a>` is never emitted. A definition written inside a spoiler stays hidden in the footnotes section.
15. **`input[checked]`** was added to the allowlist; the synthesis listed only `disabled`.
16. **The depth limit** is defined on output DOM depth, with cost and reserve per tag. Flattening keeps structures valid, so the HTML parser never foster-parents (which would break DOMPurify equality). Leaves are exempt. Markdown depth is enforced on the event stream rather than before parsing, because pulldown-cmark itself is iterative; fuzzing covers it.
17. **Math.**
    - The disabled list is extended to the whole macro-definition family, plus `\catcode`, `\input` and `\include`.
    - Group depth counts `\left` and `\begin` as well as braces.
    - math-core output is re-parsed and filtered (no `style`, `class`, `id` or `href`), with attributes sorted, MathML depth ≤ 64, and output ≤ 65,536 bytes.
    - In Temml mode the HTML is `<span data-tex="inline|block">TEX</span>`: the attribute holds the mode and the content holds the source.
18. **Excerpt** = 499 graphemes + `…`, with a byte cap of 4,997 + 3. That fits `post.excerpt` exactly (500 graphemes, 5,000 bytes); "500 graphemes plus `…`" would overflow it by one.
19. **`read_minutes`** = 0 for an empty body, otherwise `max(1, round(wc/250))` (the ForumMagnum rate).
20. **Quote limits.** "KB" is read as 1,000 bytes: the target is ≤ 200,000 bytes of P (the tail is not searched) and the quote ≤ 2,000 bytes. At most 1,000 occurrences are scored, to bound poison-pill cost. Overlapping matches count. `Pending` is distinct from `Orphaned` (unknown is not absent).
21. **`term_sanitize` contexts.**
    - Name: collapses whitespace, strips zero-width characters and badge glyphs, and caps graphemes.
    - Code: shows zero-width and bidi characters as visible markers (Trojan Source), and expands tabs to 4-column stops.
    - CR is shown as `^M` outside Name.
    - `BADGE_GLYPHS` is a fixed 10-code-point set.
22. **The ToC** lists only top-level headings. `anchors` lists every emitted id (presentation: after the limit stage and the output budget).
23. **`links`** excludes profile links and mentions, matching `data-model.md`, where `markdown.links` holds record uris.
24. **Code-language classes** are lowercased. A token outside `[a-z0-9_+-]{1,32}` (for example `c#`) gets no class.
25. **Media alt** falls back from the markdown alt to `media.alt`, on the first reference to each item only (an alt of up to 20,000 bytes must not repeat per reference). The P projection uses only the markdown alt.
26. **Derived values come from the rendered content of S6** (§12, §6.3): the body and the rendered footnote definitions, outside image alts. What is never shown (an unreferenced footnote, a link in an alt) neither notifies, nor counts as a pingback, nor keeps media out of the gallery. They are `lfm/1` values, so they never depend on the limit stage or the output budget: otherwise two releases with different presentation could publish different `markdown.mentions` for one text.
27. **Flattening keeps references.** `flat()` (§8.4) keeps images and footnote references as the leaves they are, and a figure's caption as text, so a flattened block still shows its media and every rendered footnote keeps its reference (and its sidenote). It drops a spoiler's content, which flat markup could not hide.
28. **Output budget.** A 48 MiB estimate-based cut of SL (§8.4) bounds what fixed markup and record fields can expand a 1 MB body into; it is presentation, so it may change with `RENDERER_VERSION`, and it never changes an `lfm/1` value.
29. **`mailto:` addresses one mailbox** (§5.1): the host shown next to a link must be the only domain the mail can go to.
30. **The real-host test runs on the displayed text** (§5.5): invisible characters removed, NFKC, names of any script. An ASCII-only test missed `gооgle.com` (Cyrillic) and `google.com&#8203;`.
31. **Sanitiser configuration** (§8.5): `SAFE_FOR_XML: false` and `ADD_URI_SAFE_ATTR` are needed for `DOMPurify(html) === html`; neither weakens the allowlist, because the renderer escapes `<` and `>` in every attribute and emits only allowlisted attributes.

**Open questions**
- **math-core version.** *(Resolved by ADR 0002: math-core `=0.9.0`, Temml only as a contingency.)* ADR 0002's MathML normaliser and its extra disabled words `\label`, `\eqref`, `\ref` are folded into §7.1–§7.3 and the vectors, which closes `README.md` §1.4 K3.
- **General_Category data.** *(Resolved: `unicode-general-category =1.1.0`, Unicode 16.0, an exact workspace pin; see §2 "Unicode data".)*
- **HTML serialiser drift.** *(Resolved for now.)* Escaping `<` and `>` in attributes is recent browser behaviour, which jsdom does not have yet: the jsdom runner compares DOMPurify's output with jsdom's own re-serialisation, and Chromium checks exact equality (§8.5).
- **Shared definitions.** `short_did` (§5.4) and the badge set (§9.1) should be shared with `glossary.md` and the TUI and CLI specs once those exist.
- **CLI spoiler reveal.** The name of the CLI option that reveals spoilers is left to the CLI spec.
