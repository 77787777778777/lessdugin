<!-- SPDX-License-Identifier: CC0-1.0 -->

# Shared test vectors

Language-neutral vectors for lessdugin behaviours. The Rust runners (`cargo test`) and the
TypeScript runner (vitest over the `vectors` wasm build) read the same files, and must produce
identical results (synthesis §7, `docs/spec/lfm-1.md` §13). Vector files are CC0-1.0.

## File format

Each `*.json` file is one object:

```json
{
  "behaviour": "lfm/1",
  "kind": "render",
  "description": "what the file covers and how to run it",
  "vectors": [
    { "name": "unique-within-file", "input": { … }, "expected": { … } }
  ]
}
```

- `behaviour` is the versioned behaviour ID the vectors pin (`lfm/1`, `fk/1`, `sk/1`, …).
- `kind` selects the function under test; the `description` documents its `input` and
  `expected` shapes.
- `expected` holds **only** the fields a vector asserts. Runners compare each listed field
  exactly (JSON equality) and ignore fields that are not listed.
- Names are unique within a file.
- Strings may contain any Unicode. Invisible or confusable code points (controls, format
  characters such as bidi overrides, combining marks, non-ASCII spaces) are written as `\uXXXX`
  escapes so that reviewers see them.

## Directories

| Directory | Behaviour | Runner |
|---|---|---|
| `keys/` | `fk/1`, `sk/1` record keys (`docs/spec/data-model.md` §3) | see `keys/gen_keys.py` |
| `limits/` | `limits.md` §21 vectors; `graphemes.json`: grapheme counting for `maxGraphemes` (`data-model.md` §2.2) | `graphemes.json`: `crates/lexicon/tests/graphemes.rs` |
| `lfm1/` | `lfm/1` markdown (`docs/spec/lfm-1.md` §13) | `crates/markdown/tests/vectors.rs` |
| `xss/` | HTML and MathML XSS / mXSS corpora (`lfm/1`, kind `render`) | same |
| `terminal/` | terminal escape corpus and printer snapshots (kind `terminal`) | same |
| `mathml/` | engine-specific MathML snapshots (kind `mathml`, math-core 0.9) | same |

## `lfm/1` kinds

| `kind` | `input` | `expected` fields |
|---|---|---|
| `render` | `text` (or `gen`), `media?` (`defs#media` JSON), `profile?`, `encoding_error?` | any `Rendered` field (`html`, `styled`, `text`, `excerpt`, `toc`, `word_count`, `read_minutes`, `links`, `mentions`, `media_used`, `anchors`, `flags`, `error`) and the derived fields below |
| `terminal` | as `render`, plus `print: {color, hyperlinks, reveal_spoilers, width}` | as `render`, plus `printed` (the plain-stdout printer output) |
| `safe_href` | `raw` (or `raw_gen`), `ctx`: `markdown` \| `field` | `href`: the `Href` as `{type, …}`, or `null` |
| `quote` | `quote: {exact, prefix?, suffix?, start?}`, `target` (or `target_gen`; absent = pending body) | `result`, `qkey`, `first` |
| `term_sanitize` | `text`, `ctx`: `prose` \| `code` \| `name`, `max?` | `out`: a string if every run is plain, else `[{"plain"\|"escaped": text}, …]` |
| `short_did` | `did` | `out` |
| `mathml` | `tex`, `display` | `html` (document HTML of `$tex$` / `$$tex$$`), `math_errors` |

**Generated inputs.** Large inputs are given as `gen: {prefix, repeat, times, suffix}`, meaning
`prefix + repeat × times + suffix` (also `raw_gen`, `target_gen`).

**Derived `render` fields:**

- `html_temml`: the HTML in Temml mode (`lfm-1.md` §7.5), checked in the same run;
- `text_x`: P_x (§10.3);
- `html_head` / `html_tail`: the first / last 400 characters of `html` (for huge outputs);
- `html_rle`: `html` split on `"\n"`, each run of N > 1 identical consecutive lines written once
  as `"<line> ×N"` (U+00D7), for deep-nesting vectors.

**Invariants** checked over every `render` and `terminal` vector, whatever `expected` lists:
the §8.2/§7.3 allowlist checker accepts `html` and `html_temml`; rendering twice is
byte-identical; P and the excerpt contain no whitespace other than U+0020; no `StyledDoc`
text contains C0, DEL, C1, U+2028/U+2029 or a bidi control; printer output contains no escape
sequence other than the §9.4 SGR and OSC 8 forms (and none at all without color); with the
`ammonia` feature, `ammonia(html) == html`.

## Workflow

```sh
cargo test -p lessdugin-markdown --test vectors                       # check
LESSDUGIN_BLESS=1 cargo test -p lessdugin-markdown --test vectors     # rewrite expected values
python3 -I testdata/vectors/lfm1/gen_inputs.py                        # (re)generate lfm/1 inputs
```

Bless mode rewrites every expected field with the current output (and normalises the file
formatting); review the diff before committing. `gen_inputs.py` keeps the expected values of
vectors whose input did not change, so adding vectors is: edit the generator, run it, bless,
review.
