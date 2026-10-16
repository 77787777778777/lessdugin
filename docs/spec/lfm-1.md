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
| **`lfm/1` semantics** | The parser profile (§2). Pass 0 and post-passes 1–6 (§3, §4). `safe_href` and the URL policy (§5). `cid:` resolution (§6.1). Math acceptance: delimiters, pre-scan and disabled macros (§7.1). P, P_x, the excerpt, word count and read time (§10). Quote resolution (§11). Derived links and mentions (§12). | Never changes. Any change requires `lfm/2`. |
| **Presentation**, versioned by `RENDERER_VERSION` | Exact HTML serialisation, classes and fixed strings (§8). MathML filtering details (§7.3). The `StyledDoc` shape and printer styling (§9). | May change in any release, provided all three of these happen in the same commit: bump `RENDERER_VERSION` (a `u32` constant, starting at 1, that is part of the render-cache key `(cid, RENDERER_VERSION)`); re-bless the vectors; regenerate the DOMPurify and ammonia configurations from §8.2. Adding anything to the §8.2 allowlist needs a security review. |

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
- `html` = the root element (§8.1) containing a single `p.notice` with the matching fixed string (§8.6);
- `text` = `""`;
- every list empty.

**`RenderOpts`** only selects which outputs to compute: any subset of `{html, styled, text}`. It never changes their content. So a rendered output depends only on `(src, media, RENDERER_VERSION)`, and the render cache key `(cid, RENDERER_VERSION)` is sound.

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
  anchors:       Vec<String>         // every element id emitted, in document order
  flags:         { truncated, depth_limited, unknown_profile: bool, math_errors: u32 }
}
```

### 1.5 Pipeline

```
src ─parse (§2)─► E ─pass 0 (§3)─► S0 ─pass 1 autolinks─► S1 ─pass 2 spoiler/details─► S2 ─pass 3 figures─► S3
S3 ──► P, P_x, heading text, slugs                                        (§10; quote targets §11)
S3 ─pass 4 mentions─► S4 ─pass 5 URL policy and media─► S5 ─pass 6 ids─► S6 ─limit stage (§8.4)─► SL
SL ──► HTML writer (§8)    SL ──► StyledDoc builder (§9)    S6 ──► links, mentions, media_used, anchors (§12)
```

Every stage MUST be one or more forward iterations over an event vector, with explicit stacks. No code path may recurse in proportion to input nesting (F8). This covers every pass, both writers, P, and the MathML filter and serialiser.

---

## 2. Parser profile

**Crate.** `pulldown-cmark = { version = "=0.13.4", default-features = false }`. The `html`, `getopts` and `simd` features are off, because lessdugin has its own writers.

**Base syntax.** CommonMark 0.31.2, as implemented by that version.

**Invocation.** `Parser::new_ext(src, OPTIONS)`, with no broken-link callback.

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

**Events.** These are the events 0.13.4 can emit under `OPTIONS`:
- `Start`/`End` of: `Paragraph`, `Heading`, `BlockQuote(kind)`, `CodeBlock(Indented|Fenced(info))`, `HtmlBlock`, `List(start)`, `Item`, `FootnoteDefinition(label)`, `Table(aligns)`, `TableHead`, `TableRow`, `TableCell`, `Emphasis`, `Strong`, `Strikethrough`, `Link{link_type, dest_url, title, id}`, `Image{…}`;
- `Text`, `Code`, `InlineMath`, `DisplayMath`, `Html`, `InlineHtml`, `FootnoteReference(label)`, `SoftBreak`, `HardBreak`, `Rule`, `TaskListMarker(bool)`.

Pass 0 handles any other tag defensively (§3).

---

## 3. Pass 0: normalisation

Pass 0 turns the parser's event stream E into the stream S0. Its steps run in this order.

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

**3. Line endings.** In every text payload, replace `"\r\n"` with `"\n"`, then every remaining `"\r"` with `"\n"`. Text payloads are `Text`, `Literal`, `Code`, `InlineMath`, `DisplayMath`, raw HTML, link and image `dest_url` and `title`, and the code-block info string.

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
      if d1 − d0 ≤ 253 and valid_domain(t[d0..d1]):
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
  summary = details only: [Text(trim_start(r)) if non-empty] ++ S[i+2 .. k], with trailing White_Space
            trimmed from the last Text and empty Text events dropped
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

An image inside a link (`[![a](x "t")](y)`) is never a figure.

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

**No nested anchors.** A link inside a link cannot occur in CommonMark. Even so, the writers MUST NOT emit `<a>` inside `<a>`. Any construct that would do so (an external image, a footnote reference) renders its anchor-free form inside a link: the label text, or `<sup>N</sup>`.

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

**Footnotes.**
- **Ordinals.** A definition label gets ordinal N = 1, 2, … in the order of its first `FootnoteReference` in S6 event order (references inside definitions count at their source position).
- **Definitions.** For each label, the first definition in event order is used. A definition that is never referenced, and any later duplicate, is not rendered. It still contributes to P (§10.1).
- **Ids.** The K-th reference (K = 1, 2, …) to ordinal N gets id `md-fnref-N-K`. Definition N gets id `md-fn-N`.

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
       host = "[" hex/':'/'.' "]" (IPv6 literal) or ascii_lowercase of the part before the first ':'
       host must match ^[a-z0-9._-]+$ (or the IPv6 form) and contain a char other than '.'   // non-ASCII hosts are rejected
       port, if present, ^[0-9]{1,5}$ and ≤ 65535
       url = s + "://" + host + (":" + port)? + pct(rest after authority)                  → External{url, host}
     s == "mailto":
       rest non-empty and not starting with '/'; url = "mailto:" + pct(rest)
       host = ascii_lowercase(part after the last '@', up to '?') or ""                 → Mailto{url, host}
     s == "at":  parse_at_link(rest, ctx) (§5.2)                                            → Record | Profile | ProfileHandle
     otherwise → reject
pct(x): each UTF-8 byte of x that is not in [A-Za-z0-9] or "-._~:/?#[]@!$&'()*+,;=%" becomes "%XX" (uppercase hex)
```

Consequences:
- `javascript:`, `data:`, `file:`, `vbscript:`, relative and protocol-relative URLs are never accepted.
- Non-ASCII in the path, query or fragment is percent-encoded, and any `%` already present is kept as is.
- A backslash is rejected everywhere, because WHATWG treats it like `/` in special schemes, which would make our parsing and the browser's disagree.

**Opening links outside the app.** Only `External` and `Mailto` may be opened outside the app. The URL is passed as a single argv entry and never through a shell.

### 5.2 `at://` links

```
parse_at_link(rest, ctx):                  // rest = text after "at:"
  require rest starts with "//"; body = rest[2..]; reject if body contains '?' or '#'
  segs = body.split('/')
  segs[0] is a DID:  did:plc → ^did:plc:[a-z2-7]{24}$ ;  did:web → ^did:web:<lowercase hostname, no port, no path>$ ;
                     any other method → reject
    len(segs) == 1 → Profile{did}
    len(segs) == 3 → segs[1] is a valid NSID and segs[2] a valid record key (atproto syntax, lessdugin-atp)
                     → Record{uri = "at://" + did + "/" + nsid + "/" + rkey}
    otherwise (including a trailing '/') → reject
  segs[0] is a valid atproto handle (lowercased), len(segs) == 1, and ctx == Field → ProfileHandle{handle}
  otherwise → reject
```

**Markdown never accepts handle authorities.** Writers MUST rewrite `at://<handle>/…` destinations to DIDs before publishing (§12). A `ProfileHandle` from a record field (for example `reportContact`) is shown as `@handle`. It becomes navigable only after the handle verifies in both directions (`sync.md` §3.1); until then it is plain text.

**Routes:**
- `Record{at://U}` → `href="#/at/U"`, plus `data-at-uri="at://U"`.
- `Profile{d}` and mentions → `href="#/u/d"`, plus `data-did="d"`.

Before use, the SPA MUST re-parse `data-at-uri` and `data-did` with the core's parser (F5).

### 5.3 Hover previews

| Rule | |
|---|---|
| Eligible | `a[data-at-uri]` whose NSID is one of `NS.post`, `NS.comment`, `NS.sequence`, `NS.wikiRevision` or `NS.forum`; and `a[data-did]` (a user card). Other collections, including foreign ones like `app.bsky.feed.post`, still navigate to `#/at/…` but get no preview. |
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

**HTML.** For an `External`/`Mailto` link, including `ext-image`, compute `t` from the link's visible text:
1. `t = ascii_lowercase(trim_ws(NFC(plain(inner))))`;
2. strip one leading `@`;
3. if `t` contains `@`, keep only the part after the last `@`;
4. strip one `http://`, `https://` or `mailto:` prefix;
5. strip one `www.` prefix.

**Host-like test.** `t` is host-like if it starts with a dotted name `m` matching `[a-z0-9_-]+(\.[a-z0-9_-]+)+` that is followed by end-of-string, `/`, `:`, `?` or `#`.

If `t` is host-like and `m ≠ host` (with one `www.` stripped from `host` too), the writer appends `<span class="link-host">host</span>` inside the `<a>`, after its content.

**Terminal.** The rule is F1's: see §9.4.

---

## 6. Media

### 6.1 `cid:` resolution

A `cid:` image URL resolves as follows:
1. Strip `cid:`, matched ASCII case-insensitively, to get `c`.
2. `c` MUST match `^b[a-z2-7]{1,127}$`.
3. It resolves to the **lowest** index i where `media[i].blob.ref.$link == c`, compared byte for byte.

It never resolves against anything outside this body's `media` array. No arbitrary CID is ever fetched. If nothing matches, it is a **miss**.

### 6.2 Rendering

| Case | HTML | Terminal placeholder (§9.3) |
|---|---|---|
| Hit; `media[i].blob.mimeType` ∈ {`image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/avif`} | `<img data-cid="c" alt="A" width="W" height="H">` | `[image N: A]` |
| Hit; any other type | `<span class="media" data-cid="c">L</span>` | `[video N: L]`, `[audio N: L]` (by `video/*` or `audio/*`), else `[file N: L]` |
| Miss | `<span class="media-missing">alt</span>` | `[missing media: alt]` |

The symbols in the table:
- **A** (alt): the markdown alt (§4.5), if it is non-empty after trimming; else `media[i].alt`; else `""`. The `alt` attribute is always emitted.
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

- **`media_used`** = every index i whose `blob.ref.$link` equals some resolved `cid:` URL in the body, in ascending order. Duplicate entries of a referenced CID count as used.
- **The gallery** holds every other index, in array order. It renders after the footnotes as `<div class="gallery">`, with one placeholder per item (as in §6.2, with A = `media[i].alt`). In the terminal, the gallery is `Media` blocks.
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
   - any of `\href \url \includegraphics \class \style \def \gdef \edef \xdef \let \futurelet \newcommand \renewcommand \providecommand \newenvironment \renewenvironment \DeclareMathOperator \DeclareRobustCommand \catcode \input \include`;
   - or any control word starting with `\html`.

The pre-scan runs on the raw text, so a disabled word inside `\text{}` or after `%` still counts.

### 7.2 Engine (default mode)

**Engine.** `math-core`, pinned exactly at `=0.9.0` (ADR 0002, spike S10). ADR 0002 also specifies a style-to-class MathML normaliser and three more disabled control words that §7.1 and §7.3 do not yet contain; until they are folded in at M4, see `README.md` §1.4 K3. It converts each expression independently with `convert_with_local_state`, under this configuration:
- `MathCoreConfig { pretty_print: Never, xml_namespace: false, ignore_unknown_commands: false, annotation: false, allow_unreliable_rendering: false, global_group: false, unicode_substitution: Conventional, max_expansions: 1000, ..Default }`;
- no custom macros;
- display mode is `Inline` for `InlineMath` and `Block` for `DisplayMath`.

**Errors.** An engine error, including an unknown command, is a math error.

**Filtering.** The engine's output is never inserted as is. It is parsed into a tree (iteratively), filtered (§7.3), and re-serialised by our serialiser (§8.3).

### 7.3 MathML filter (presentation)

**Allowed elements.** `math mrow mi mn mo ms mtext mspace msub msup msubsup munder mover munderover mmultiscripts mprescripts none mfrac msqrt mroot mtable mtr mtd mpadded mphantom mstyle merror menclose`.

**Other elements:**
- `semantics` is replaced by its first child.
- `annotation`, `annotation-xml` and `maction` are removed together with their subtrees.
- HTML `a` and `span` (math-core emits them inside `mtext`) are unwrapped: their children are kept.
- Every other element is removed with its subtree.

**Allowed attributes.** Everything else is dropped, including `style`, `class`, `id`, `href`, `src`, `xlink:*`, `xmlns*`, `on*`, `mathcolor`, `mathbackground` and `data-*`.

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

**Serialisation.**
- Attributes are sorted by name (ASCII).
- Every element has an explicit end tag; there are no self-closing forms.
- Text is escaped as in §8.3.

**Limits.** After filtering, if the element depth inside `math` (where `math` is 1) exceeds 64, or the serialised `math` element exceeds 65,536 bytes, the expression is a math error.

**Known degradation.** Stripping `style` and `class` loses colours, some spacing, equation-table widths and menclose CSS helpers. S10 records which constructs degrade. This is accepted under CSP `style-src 'self'`.

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

**Font.** The STIX Two Math subset font is **Reserved (deferred)**, per the completeness review. `mathml.css` uses system math fonts.

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
| CodeBlock | `<pre><code class="language-L">text</code></pre>`. L = ASCII-lowercased first whitespace-delimited token of a fenced info string; the class is emitted only if L matches `^[a-z0-9_+-]{1,32}$`. Indented code and invalid L get `<pre><code>`. |
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
| Media | §6.2 |
| Gallery | `<div class="gallery">` + one item per line + `</div>` |
| Truncation (`flags.truncated`) | `<p class="notice">[content truncated: too many elements]</p>`, after the body and before the footnotes |

**Links.** External links always carry `rel="nofollow ugc noopener noreferrer"`. Removing `nofollow` for high-karma authors (a ForumMagnum feature) is not planned (Never, `docs/plan.md` §6.5): it would make rendering depend on the fold. Internal, anchor and mention links have no `rel`.

**Direction.** The root gets `dir="auto"`, and so does every `p`, `h1`–`h6`, `li`, `th`, `td`, `summary` and `figcaption`. The SPA stylesheet sets `unicode-bidi: isolate` on them, and wraps names in `<bdi>` outside rendered content.

### 8.2 Allowlist

Nothing outside this table is ever emitted. Every `class` attribute holds exactly one token.

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
| MathML | §7.3 | |

**Never emitted:** `style`, `src`, `srcset`, `target`, `title`, `name`, `form`, `on*`, `xmlns*`, `xlink:*`, and the elements `svg`, `iframe`, `video`, `audio`, `source`, `object`, `embed`, `script`, `style`, `template`, `base`, `meta`, `link` and `mark`.

Runtime DOM changes the SPA makes after sanitisation are not renderer output. These are setting `src` on a verified `img`, replacing `span.media`, and the Temml rendering.

### 8.3 Serialisation

The serialisation is chosen so that `DOMPurify.sanitize(html, LFM_CONFIG) === html` and `ammonia(html) == html`. Both are asserted over every vector.

**Escaping:**
- **Text:** `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, U+00A0→`&nbsp;`.
- **Attribute values:** always double-quoted, with `&`→`&amp;`, `"`→`&quot;`, `<`→`&lt;`, `>`→`&gt;`, U+00A0→`&nbsp;`. This follows the current HTML serialiser; the vector runner pins the browser and jsdom versions.

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

**Reserve** is the room a container needs for its required children:

| Tag | Reserve |
|---|---|
| List | 1 (`li`) |
| Table | 2 (`tr` + cell) |
| Figure | 1 (`figcaption`) |
| Details/Spoiler | 1 (`summary`) |
| everything else | 0 |

Footnote-definition content starts at d = 3 (`section`, `ol`, `li`), wherever the definition appears in the source.

```
on Start(tag) at depth d:
  if d + cost + reserve ≤ 64: admit (d += cost)
  elif tag is inline (Emphasis, Strong, Strikethrough, Link):
      drop the Start and its matching End; the content stays (a dropped Link renders as a dropped link)
  elif tag ∈ {Item, TableHead, TableRow, TableCell, Summary}: admit   // always fits: the parent reserved room
  else:                                 // block: Paragraph, Heading, CodeBlock, BlockQuote kinds, List, Table, Figure
      replace the whole subtree with flat(subtree), wrapped in Paragraph if d + 1 ≤ 64, else emitted bare
      (the parent is then always a flow container: blockquote, div, details, li or the root)
      set flags.depth_limited
flat(subtree) = its Text, Literal and Code payloads as Text, and its math as Text(TeX source), in order;
                image alts as Text; SoftBreak at every block boundary, SoftBreak and HardBreak, with no
                repeats and none at either end; FootnoteReference and TaskListMarker dropped
```

**Leaves** are exempt from the limit: `img`, `input`, `br`, `hr`, inline `code`, `sup>a`, `span.*`, and `math`. A leaf adds at most 2 levels; MathML adds at most 64 (§7.3). So the DOM depth stays below 132, well under browser parser limits.

**Event cap.** §3 step 1.

### 8.5 Sanitiser obligations

- **SPA.** Rendered HTML is inserted only through the Trusted Types policy `lessdugin-md`, which runs DOMPurify. The DOMPurify configuration is **generated from §8.2 and §7.3** and contains nothing else:
  - `ALLOWED_TAGS` and `ALLOWED_ATTR` exactly as listed;
  - `ALLOW_DATA_ATTR: false`;
  - `ALLOWED_URI_REGEXP` = `^(?:https?:|mailto:|#/at/|#/u/)`;
  - a hook that enforces the class tokens and value grammars.
- **Native.** HTML output (`read --html`) also runs ammonia (feature `ammonia`), configured from the same tables.
- **Tests.** The vectors assert equality for both sanitisers. Any difference is a renderer bug.
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
| White_Space (U+0009–U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000–U+200A, U+2028, U+2029, U+202F, U+205F, U+3000) | TAB, LF → U+0020. CR → `^M` (Escaped). U+0085 → `\u{85}` (Escaped). U+2028/U+2029 → U+0020. The rest are kept. | TAB → U+0020 up to the next column that is a multiple of 4 (column = display width so far on the line). LF → U+0020. CR, U+0085, U+2028, U+2029 as Prose. | Every one → U+0020; then runs collapse to one U+0020 and the result is trimmed. |
| Other C0 (U+0000–U+001F) | `^` + chr(c + 0x40) (Escaped; ESC → `^[`) | same | same |
| U+007F | `^?` (Escaped) | same | same |
| C1 (U+0080–U+009F) | `\u{xx}`, lowercase hex (Escaped) | same | same |
| Bidi controls (U+061C ALM, U+200E LRM, U+200F RLM, U+202A LRE, U+202B RLE, U+202C PDF, U+202D LRO, U+202E RLO, U+2066 LRI, U+2067 RLI, U+2068 FSI, U+2069 PDI) | removed | `⟨NAME⟩` (Escaped) | removed |
| Zero-width (U+200B ZWSP, U+200C ZWNJ, U+200D ZWJ, U+2060 WJ, U+FEFF BOM) | kept | `⟨NAME⟩` (Escaped) | removed |
| `BADGE_GLYPHS` = {U+2611, U+2612, U+2705, U+2713, U+2714, U+2715, U+2716, U+2717, U+2718, U+26A0} | kept | kept | removed |
| Grapheme cap | – | – | `Name{max}`: if longer than `max` extended grapheme clusters, keep `max − 1` and append `…` |

The resulting invariant: a `TermText` never contains a code point in C0, U+007F, C1, U+2028, U+2029 or the bidi set.

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

### 9.3 Construct mapping

| Construct | `StyledDoc` |
|---|---|
| Paragraph | `Para`. A `HardBreak` starts a new `SLine`; a `SoftBreak` becomes a `" "` span. |
| Heading | `Heading{level, id, line}` |
| BlockQuote / Alert | the `Quote` or `Alert(kind)` gutter on the contained blocks |
| Spoiler / Details | a `Summary{id, spoiler}` block (text `Spoiler`, or the summary, or `Details`), then the content blocks with the `Spoiler(id)` or `Details(id)` gutter. Ids are 1, 2, … in document order. |
| List / Item | the `Item{ordinal, first}` gutter. `first` is true only on the item's first block. `ordinal` is the list start + the item index, or `None` for bullets. |
| TaskListMarker | span `[x] ` or `[ ] ` with `TASK` |
| CodeBlock | `Code`. The text is split on `"\n"`, a final empty line is dropped, and each line is `term_sanitize(Code)`. |
| Code | span `CODE`, Code context |
| InlineMath | span `MATH` with the text `"$" + tex + "$"`, Code context |
| DisplayMath | closes the current `Para`, emits `Math(lines of tex)`, and continues the paragraph in a new `Para` |
| Text / Literal | spans, Prose context |
| Emphasis / Strong / Strikethrough | style flags, combined |
| Link kinds | spans with `LINK` and `SLink`. A dropped link becomes plain spans. A mention is the span `@short_did` with `MENTION`. |
| FootnoteReference | span `[N]` with `FOOTREF` and `Anchor("md-fn-N")` |
| Footnote definitions | after the body: blocks with the `Footnote(N)` gutter, in ordinal order |
| Image, inline | span with `PLACEHOLDER` (§8.6) and `SLink::Media` or `External` |
| Figure, a paragraph containing only one `cid:` image, a gallery item | `Media{index, line}` (+ `Caption`) |
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

**Real host (F1).** After the link text, print a dimmed ` ⟨host⟩` (U+27E8, U+27E9) unless `norm(text) == norm(url)`. `norm` strips one `https://`, `http://` or `mailto:` prefix, one `www.`, and one trailing `/`, then ASCII-lowercases. Under `--color never`, links print as `text ⟨host⟩` (plain).

**Mentions.** Shown as in §5.4.

**Spoilers.**
- Spoiler content is not displayed until the user explicitly reveals it.
- TUI: hidden behind its `Summary` line, which a toggle key reveals.
- CLI: one line `[spoiler hidden]` replaces the content, unless the user passed an explicit reveal option (defined in the CLI spec).
- Details: TUI collapsed by default; CLI expanded.

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
- `toc` lists, in document order, every heading that is not inside a `BlockQuote`, `Spoiler`, `Details`, `List`, `Table` or `FootnoteDefinition`, as `{level, id, text}`. Clients choose how many levels to show.
- Slugs use the un-collapsed form (§4.6).

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

**React anchors** (`social.md`):
- `qkey(quote) = norm(trim_ws(quote.exact))`, or `""` when there is no quote;
- `anchor = range` if anchored, otherwise `"orphan:" + qkey`.

**Writers** (`data-model.md` W8):
1. Compute T from the version the user saw.
2. Take the user's selection, trimmed to start and end on a non-space, as `exact`. It must be 1–2,000 bytes.
3. `prefix` = up to 32 grapheme clusters of T immediately before `exact`. `suffix` = up to 32 immediately after. Drop clusters from the far end until each is ≤ 640 bytes. Both are verbatim substrings of T.
4. `start` = the code-point offset of `exact`.

**CLI `--quote "…"`:** normalise the text as q; if it has no occurrence in T, fail; otherwise use the first occurrence.

---

## 12. Derived metadata and writer obligations

**`links`.**
- The canonical at-uris of every `Record` link in S6, of any collection, deduplicated, in order of first occurrence, capped at 256.
- Profile links and mentions are not included.

**`mentions`.**
- The DIDs of every mention link (§4.4), deduplicated, in order of first occurrence, capped at 128.

**`anchors`.**
- Every `id` emitted in the HTML, in document order.

**Writers.**
- `markdown.mentions` = `Rendered.mentions`, and `markdown.links` = `Rendered.links`, both rendered from the exact text being published.
- Before publishing, writers rewrite handle-authority `at://` link destinations to DIDs. The editor's mention autocomplete inserts `[@handle](at://did)`.
- Writers set `profile: "lfm/1"`.
- They spill per `data-model.md` §5.3, and fill the excerpt per §10.4.
- Draft-local image paths become `cid:` URLs at publish time (CLI spec; F3).

**Readers** never use the record's `mentions` or `links` fields for rendering, notifications or the fold.
- **Mention notifications** and **pingbacks** are computed from `Rendered.mentions` and `Rendered.links` of synced (`pds` or `proven`) records.
- Pingbacks consider only targets in our collections.
- External backlink indexes, and the indexer's `listBacklinks`, are hints only.

**Search.** The core's in-memory search index and the indexer index P. Indexer snippets are plain text with highlight ranges in P offsets.

---

## 13. Test vectors (`testdata/vectors/lfm1/`)

**Format.** Each file has the common vector-file shape of `README.md` §5.1 (`{behaviour: "lfm/1", kind, vectors}`); each vector is `{name, input, expected}`.
- `input` is `{text, media?, profile?}` for render vectors, or the arguments of the function under test.
- `expected` holds only the fields the category asserts: `html`, `html_temml`, `styled`, `text` (P), `text_x` (P_x), `excerpt`, `word_count`, `read_minutes`, `toc`, `links`, `mentions`, `media_used`, `anchors` or `flags`.

**Runners.** The Rust runner (testkit) and the TS runner (vitest, `vectors` wasm build) MUST produce identical results. `xtask vectors --bless` regenerates the expected values, and the diff is reviewed. M4 requires at least 150 vectors in total.

| File | Covers |
|---|---|
| `options.json` | each enabled extension; each disabled one rendering literally (smart punctuation, heading attributes, YAML and `+++` blocks, definition lists, `^sup^`, `~sub~` as strikethrough, `[[wikilinks]]`, old-footnote syntax) |
| `raw-html.json` | HTML blocks of every CommonMark type, inline HTML, comments, `<script>`/`<style>`, entities, multi-line inline HTML, autolinks not applied to literals |
| `normalise.json` | CR and CRLF in every payload kind, text coalescing, the event cap (exactly 2^20 events, and 2^20 + 1 with tag closing), dangling footnote references after truncation |
| `autolink.json` | `www.`/`http://`/`https://`, letter case, boundaries (`*_~(`, start of event), trailing punctuation, `;` entities, unbalanced `)`, `<`, `_` in the last two segments, no period, a domain longer than 253, inside links, images, code and literals |
| `spoiler-details.json` | marker case, a rest of line that is not blank, a marker as a whole paragraph, an empty summary, an inline-formatted summary, nesting, alerts untouched, a marker made into a link by a reference definition, P and P_x |
| `figure.json` | title and no title, extra text, two images, an image in a link, a whitespace-only title, a figure at the depth limit |
| `mention.json` | did:plc, did:web, other methods, handle authority, text not starting with `@`, formatted inner text, `short_did` |
| `links.json` | every `safe_href` branch: schemes, relative, `//host`, userinfo, backslash, controls, bidi, non-ASCII host, path percent-encoding, a port, IPv6, the 3,000-byte cap, `mailto`, email autolinks, fragments (charset, lowercase, decoding, `#/x`), the host-display rule |
| `at-links.json` | record, profile, collection only, query or fragment, trailing slash, foreign NSIDs, `.temp` NSIDs, invalid rkeys |
| `images.json` | `cid:` hit, miss, duplicate and case; each mime class; alt fallback; aspect clamp; external images in and out of links; other schemes |
| `gallery.json` | `media_used`, gallery order, duplicates, no gallery |
| `ids.json` | slugs (Unicode, punctuation, emoji, NFC, empty), deduplication including a literal `foo-1` collision, ids in containers and footnotes, `anchors` |
| `footnotes.json` | ordinals, multiple references, undefined labels, duplicate definitions, unreferenced definitions, references inside definitions and inside links, case-insensitive labels |
| `blocks.json` | alerts, tables (alignment, escaped pipes, a header with no body), task lists, code-language classes, `ol start`, tight and loose lists |
| `math.json` | inline and display math; each disabled control word, including inside `\text{}` and comments; the 4,096-byte and depth-64 boundaries; unknown commands; MathML filtering (style, class, id, href, `semantics`); the output depth and size caps; `html_temml` variants; currency-style `$` |
| `limits.json` | depth flattening for each container kind and the inline drop at the boundary; leaf exemption; footnote base depth; `flags`; `TooLarge`; `Encoding` |
| `serialise.json` | escaping (`&<>"`, U+00A0), boolean attributes, attribute order, the newline placement, `dir` |
| `projection.json` | each row of §10.1, NFC, all 25 White_Space code points, trimming, astral-plane offsets, P vs P_x |
| `excerpt.json` | ≤ 500 graphemes, exactly 500 and 501, the byte cap with long clusters, no space, a leading spoiler, `word_count`, `read_minutes`, `toc` |
| `quote.json` | 0, 1 and many occurrences; overlapping occurrences; prefix and suffix scoring; the `start` tie-break with and without `start`; normalisation; the 2,000-byte and 200,000-byte limits; the 1,000-occurrence cap; `Pending`; `qkey` |
| `safe-href.json` | direct `safe_href(raw, ctx)` → `Href` or null, including the `Field`-only `ProfileHandle` and `Anchor` refused in `Field` |
| `term-sanitize.json` | direct `term_sanitize(input, ctx)` for every row of §9.1, tab stops, the grapheme cap |
| `styled.json` | `StyledDoc` JSON snapshots for each construct in §9.3 |

**Invariants checked over every render vector:**
- `DOMPurify(html) === html` in pinned Chromium and jsdom;
- `ammonia(html) == html`;
- every `class` token and attribute matches §8.2 or §7.3;
- native output equals wasm output;
- rendering is idempotent: rendering the same input twice gives byte-equal output.

**Related directories:**
- `terminal/` holds the escape corpus and printer snapshots, and asserts that no ESC appears other than the SGR and OSC 8 forms of §9.4.
- `xss/` holds the HTML and MathML XSS and mXSS corpora.
- `mathml/` holds engine-specific MathML snapshots.

The fuzz target is the renderer (cargo-fuzz). It feeds unbounded nesting, and the M4 bar is 10 minutes with no panics.

---

## 14. Decisions made while writing

These resolve ambiguities in the design. Each takes the simplest safe option.

1. **Two layers.** `lfm/1` freezes only data-relevant semantics: syntax, URL policy, P, excerpts, quotes and derived links. The HTML/`StyledDoc` presentation is versioned by `RENDERER_VERSION`, so styling fixes do not need `lfm/2`.
2. **Pass 0** was added before the six synthesis post-passes. It covers the event cap, raw HTML as `Literal` (block → paragraph), CR normalisation (needed for DOMPurify round-trip stability), text coalescing, and dangling footnote references.
3. **Autolinks.** `www.` links get `https://`, not GFM's `http://`. Prefixes are case-insensitive. A domain needs a period for `http(s)://` too, and is capped at 253 characters (to bound scan cost). The trailing-punctuation set and the `;`/`)` rules follow cmark-gfm, written in linear form. Every `Text` event start counts as a boundary.
4. **Spoilers** render as `<details class="spoiler">` with the fixed summary `Spoiler`. This is accessible and needs no JS. `[!spoiler]` must be alone on its line, like GFM alerts. `[!details]` takes an optional same-line summary, defaulting to `Details`.
5. **P is computed on S3,** so mentions use their source text and P is independent of handles and settings. **P_x omits spoiler content,** so excerpts never leak spoilers.
6. **Mentions** bake `@short_did` into the HTML. The verified handle is substituted at display time, which keeps the `(cid, RENDERER_VERSION)` cache sound. Only did:plc and did:web are accepted, matching `data-model.md`.
7. **`at://` links in markdown require a DID authority.** Handle authorities drop the link, and writers rewrite them to DIDs. Only `at://did` and `at://did/nsid/rkey` are accepted. `ProfileHandle` is allowed only in `Field` context, for `forum.reportContact`, and becomes navigable after bidirectional verification.
8. **`safe_href`** rejects whitespace, controls, bidi and zero-width characters, backslashes, userinfo and non-ASCII hosts; caps length at 3,000 bytes (the lexicon `url` limit); and percent-encodes non-ASCII elsewhere. Rejecting IDN hosts avoids a punycode dependency and homograph risk; the link text stays visible.
9. **External images** always render as the same link in HTML. The per-forum opt-in is a client-side replacement after sanitisation (blob URL, CSP-compatible), so HTML never depends on viewer settings.
10. **Non-image media and misses** use `span.media[data-cid]` and `span.media-missing`. The SPA chooses the player or card from the verified record, so the allowlist stays free of `video` and `audio`. `span` was added to the allowlist; the synthesis already implied it with `span[data-tex]`.
11. **Showing the real host.** HTML uses F9's host-like test, because always appending the host would be noisy. Terminals use F1's rule: show the host whenever the text differs from the URL.
12. **`dir="auto"`** goes on the root, as in the synthesis, and also on text blocks, following F5 ("every container"). This is cheap and better for mixed-direction threads.
13. **Alerts** carry no title text; CSS supplies it. Fixed English strings are confined to the §8.6 table.
14. **Footnotes.** Ordinals follow first reference. The first definition wins. Unreferenced definitions are not rendered (as on GitHub) but stay in P. Back-links are `↩`/`↩K`. Inside a link, references render without an anchor, because nested `<a>` is never emitted.
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
22. **The ToC** lists only top-level headings. `anchors` lists every emitted id.
23. **`links`** excludes profile links and mentions, matching `data-model.md`, where `markdown.links` holds record uris.
24. **Code-language classes** are lowercased. A token outside `[a-z0-9_+-]{1,32}` (for example `c#`) gets no class.
25. **Media alt** falls back from the markdown alt to `media.alt`. The P projection uses only the markdown alt.

**Open questions**
- **math-core version.** *(Resolved by ADR 0002: math-core `=0.9.0`, Temml only as a contingency.)* ADR 0002's MathML normaliser (a closed set of `style` values mapped to `mc-*` classes, `color` to `mathcolor`) and its extra disabled words `\label`, `\eqref`, `\ref` conflict with §7.1, §7.3 and the current vectors; `README.md` §1.4 K3 records the interim rule.
- **General_Category data.** The heading-slug charset needs it. The `lessdugin-markdown` dependency list (synthesis §6.2) has no such table, so a small dependency (for example `unicode-general-category`) must be added and pinned with the Unicode version.
- **HTML serialiser drift.** Escaping `<` and `>` in attributes is recent browser behaviour. The DOMPurify equality vectors must pin Chromium and jsdom, and may need a normalisation shim if jsdom lags.
- **Shared definitions.** `short_did` (§5.4) and the badge set (§9.1) should be shared with `glossary.md` and the TUI and CLI specs once those exist.
- **CLI spoiler reveal.** The name of the CLI option that reveals spoilers is left to the CLI spec.
