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
2. **Derived metadata scope (§1.5, §6.3, §12).** `links`, `mentions` and `media_used` are
   computed over all of S6, as §1.5 says, including unrendered footnote definitions and
   subtrees the limit stage later flattens.
3. **`mailto` host (§5.1).** "Part after the last `@`, up to `?`" is the domain of the address
   part (before the first `?`), so `mailto:a@b.com?cc=c@d.com` has host `b.com`.
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
10. **P keeps controls (§10.2).** Only White_Space is collapsed, so P (and the excerpt) may
    contain other C0/C1 controls such as U+0001 (`projection.json#control-chars-kept`). The
    tested invariant is therefore "no whitespace other than U+0020" (see bug 9).

### Presentation (`RENDERER_VERSION` layer)

11. **Truncation notice (§8.1).** Written `<p class="notice" dir="auto">`: the Direction rule
    gives every `p` `dir="auto"`, although the §8.1 row shows the notice without it.
12. **Anchors (§12 vs §1.5).** `anchors` lists the ids actually emitted in the HTML, i.e. after
    the limit stage (a flattened heading has no id), as §12 says; §1.5 lists S6 as the source.
13. **Limit-stage costs (§8.4).** A link dropped by the URL policy opens no element, so it
    costs 0 and is never dropped; images are leaves (their alt events are not depth-checked);
    `flat()` that yields nothing emits nothing (no empty paragraph).
14. **Real host (§5.5).** For `ext-image` the visible text used by the host test is the alt
    (`plain(inner)`), not `"image: …"` (which is never host-like). No `link-host` span is
    emitted when the host is empty (`mailto:` without `@`).
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
18. **`RENDERER_VERSION`.** `1`; builds without the `math` feature (Temml mode, §7.5) set bit 31,
    so the mode is part of the cache key.
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
    into several styled spans gets one OSC 8 wrapper per word and one suffix. Gutters print as
    `│ ` (quote, alert), `┆ ` (spoiler), two spaces (details), `• `/`N. ` (items) and `[N] `
    (first block of a footnote).
23. **`TermText` JSON.** A plain string when every run is plain, else
    `[{"plain": …} | {"escaped": …}, …]`.
24. **Hover previews (§5.3).** `preview_eligible(uri, ns)` takes the build's `NS` (the markdown
    crate does not depend on the lexicon crate).

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
4. **§8.4 flattening reveals spoilers.** `flat()` of a spoiler deeper than the limit puts its
   content in a plain paragraph (`limits.json#spoiler-flattened-reveals`); P_x and excerpts are
   unaffected. Suggest `flat(Spoiler)` = the fixed summary only. `flat()` also drops a figure's
   caption, which P keeps.
5. **§7.3 MathML filter vs math-core 0.9.** The spec says math-core emits only `a` and `span`
   inside `mtext`; 0.9 also emits `b i em code s u` (and `span class/style`) for `\textbf`,
   `\texttt`, `\emph`, … Read literally, the filter would delete their text (decision 16).
6. **§7.2 engine recursion.** Unbounded recursion in math-core is a crash vector (decision 17);
   the spec should either define a nesting limit or require one from the engine (S10).
7. **§1.5 vs §12 `anchors`** disagree on the source (decision 12); **§8.1 notice `dir`**
   disagrees with the Direction rule (decision 11).
8. **§5.5 homoglyphs.** The host-like test is ASCII-only, so `[gооgle.com](https://evil.com)`
   with Cyrillic `о` is not host-like and gets no `link-host` span in HTML (terminals show the
   host anyway). Consider treating dotted text with non-ASCII letters as host-like.
9. **§10.2 controls in P.** P keeps non-whitespace C0/C1 controls, which then reach
   `post.excerpt` and search indexes. Consider removing `Cc` code points in §10.2 (an `lfm/1`
   semantic, so before the freeze).
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
16. **Dependencies.** The workspace pins `math-core = "0.9"` (resolved to 0.9.0; the spec wants an
    exact pin from S10). `unicode-general-category 1.1` (the open question in §14) and
    `unicase 2.10` (the `UniCase` footnote-label matching pulldown-cmark uses) were added to
    `crates/markdown/Cargo.toml` directly rather than to `[workspace.dependencies]`.
