#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Generate LFM/1 vector inputs. Expected values are filled by the runner's bless mode and
then reviewed by hand. Existing expected values are preserved when a file is regenerated."""
import json, os, sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
DID = "did:plc:abcdefghijklmnopqrstuvwx"
DID2 = "did:plc:ccccccccccccccccccccccc2"
DIDWEB = "did:web:example.com"
NS = "online.7778777.lessdugin"
CID1 = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
CID2 = "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy"
CID3 = "bafkreiabcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst"
CID4 = "bafkreib2rxk3rh6kzwq2tzz5qsmsa6e7rqikz3uwpl3iuvbb4hgsndhzzq"


def media(cid, mime="image/png", alt=None, ar=None, name=None):
    m = {"blob": {"$type": "blob", "ref": {"$link": cid}, "mimeType": mime, "size": 1234}}
    if alt is not None:
        m["alt"] = alt
    if ar is not None:
        m["aspectRatio"] = {"width": ar[0], "height": ar[1]}
    if name is not None:
        m["name"] = name
    return m


def V(name, inp, *fields):
    return {"name": name, "input": inp, "expected": {f: None for f in fields}}


def R(name, text, *fields, media=None, profile=None):
    inp = {"text": text}
    if media is not None:
        inp["media"] = media
    if profile is not None:
        inp["profile"] = profile
    return V(name, inp, *(fields or ("html", "text")))


def G(name, gen, *fields, media=None):
    inp = {"gen": gen}
    if media is not None:
        inp["media"] = media
    return V(name, inp, *fields)


FILES = {}


KIND_DESC = {
    "render": "render(text, media, profile) vectors (docs/spec/lfm-1.md). input: {text | gen, media?, profile?, encoding_error?}; gen = prefix + repeat*times + suffix. expected holds only the asserted Rendered fields (html, styled, text, excerpt, toc, word_count, read_minutes, links, mentions, media_used, anchors, flags, error) plus derived fields: html_temml (Temml-mode HTML, sec. 7.5), text_x (P_x, sec. 10.3), html_head/html_tail (first/last 400 chars of html), html_rle (html lines, runs of identical lines as 'line \u00d7N'). Every render vector is also checked against the sec. 13 invariants (allowlist, idempotence, sanitiser equality, terminal escapes).",
    "safe_href": "safe_href(raw, ctx) vectors (docs/spec/lfm-1.md sec. 5.1-5.2). input: {raw | raw_gen, ctx: markdown|field}; expected.href is the Href as JSON ({type, ...}) or null.",
    "quote": "Quote resolution vectors (docs/spec/lfm-1.md sec. 11). input: {quote: {exact, prefix?, suffix?, start?}, target | target_gen (absent = Pending)}; expected: result ({type: anchored, start, end} in code points | orphaned | pending), qkey, first (CLI --quote first occurrence [start, end] or null).",
    "term_sanitize": "term_sanitize(text, ctx) vectors (docs/spec/lfm-1.md sec. 9.1). input: {text, ctx: prose|code|name, max? (Name grapheme cap, 0 = none)}; expected.out is the TermText: a string when every run is plain, else [{plain|escaped: text}, ...].",
    "short_did": "short_did(did) vectors (docs/spec/lfm-1.md sec. 5.4). expected.out is the placeholder without the leading @.",
    "terminal": "Terminal escape corpus (docs/spec/lfm-1.md sec. 9). input as for render plus print: {color, hyperlinks, reveal_spoilers, width} options of the plain-stdout printer; expected: styled (StyledDoc JSON) and printed (printer output). Invariants: no TermText contains C0, C1, DEL, U+2028/9 or bidi controls; printer output contains no escape sequence other than the allowed SGR and OSC 8 forms.",
    "mathml": "Engine-specific MathML snapshots (math-core 0.9, filtered by sec. 7.3). input: {tex, display}; expected: html (the document HTML of $tex$ or $$tex$$) and math_errors.",
}


COVERS = {
    "lfm1/options.json": "Parser profile (sec. 2): each enabled extension and each disabled one rendering literally.",
    "lfm1/raw-html.json": "Raw HTML as literal text (sec. 3 step 2): HTML blocks of every CommonMark type, inline HTML, comments, entities, multi-line inline HTML.",
    "lfm1/normalise.json": "Pass 0 (sec. 3): CR/CRLF in every payload kind, coalescing, the 2^20 event cap (exact, over, at a definition start) and dangling footnote references after truncation.",
    "lfm1/autolink.json": "Pass 1 GFM extended autolinks (sec. 4.1).",
    "lfm1/spoiler-details.json": "Pass 2 spoiler and details blocks (sec. 4.2), with P and P_x.",
    "lfm1/figure.json": "Pass 3 figures (sec. 4.3).",
    "lfm1/mention.json": "Pass 4 mentions (sec. 4.4) and mention display (sec. 5.4).",
    "lfm1/short-did.json": "",
    "lfm1/links.json": "Pass 5 URL policy for links (sec. 4.5) through safe_href (sec. 5.1) and the HTML real-host rule (sec. 5.5).",
    "lfm1/at-links.json": "at:// links (sec. 5.2) and derived links/mentions (sec. 12).",
    "lfm1/images.json": "Images: cid: resolution and placeholders (sec. 6.1-6.2), external images, other schemes, alt text.",
    "lfm1/gallery.json": "media_used and the gallery (sec. 6.3).",
    "lfm1/ids.json": "Heading ids, slugs and deduplication (sec. 4.6), anchors (sec. 12), ToC (sec. 10.6).",
    "lfm1/footnotes.json": "Footnote ordinals, ids, back-links and rendering (sec. 4.6, 8.1).",
    "lfm1/blocks.json": "Block constructs (sec. 8.1): alerts, tables, task lists, code classes, ol start, lists, quotes.",
    "lfm1/math.json": "Math acceptance (sec. 7.1), engine errors, MathML filtering and caps (sec. 7.3), errors (sec. 7.4) and the Temml output (sec. 7.5).",
    "lfm1/limits.json": "Limit stage (sec. 8.4), flags, TooLarge, Encoding, profiles and deep nesting.",
    "lfm1/serialise.json": "Serialisation (sec. 8.3): escaping, boolean attributes, attribute order, newline placement, dir.",
    "lfm1/projection.json": "Plain-text projection P and P_x (sec. 10.1-10.3).",
    "lfm1/excerpt.json": "Excerpt, word count, read time and ToC (sec. 10.4-10.6).",
    "lfm1/quote.json": "",
    "lfm1/safe-href.json": "",
    "lfm1/term-sanitize.json": "",
    "lfm1/styled.json": "StyledDoc snapshots for each construct of sec. 9.3.",
}


def F(path, kind, vectors, desc=None):
    desc = desc or COVERS.get(path) or None
    FILES[path] = {"behaviour": "lfm/1", "kind": kind, "description": (desc + " " if desc else "") + KIND_DESC[kind] + " Inputs generated by lfm1/gen_inputs.py; expected values blessed with LESSDUGIN_BLESS=1 and reviewed.", "vectors": vectors}


HT = ("html", "text")

# ------------------------------------------------------------------ options.json
F("lfm1/options.json", "render", [
    R("tables", "| a | b |\n|---|---|\n| 1 | 2 |\n"),
    R("footnotes", "x[^1]\n\n[^1]: note\n"),
    R("strikethrough-double", "~~gone~~ text"),
    R("strikethrough-single-tilde", "~gone~ text"),
    R("tasklists", "- [ ] open\n- [x] done\n- [X] upper\n"),
    R("math-inline-and-display", "$a+b$ and\n\n$$c$$\n"),
    R("alert-note", "> [!NOTE]\n> body\n"),
    R("alert-kinds-case-insensitive", "> [!tip]\n> a\n\n> [!Important]\n> b\n\n> [!WARNING]\n> c\n\n> [!caution]\n> d\n"),
    R("alert-marker-not-alone", "> [!NOTE] inline\n> body\n"),
    R("alert-unknown-kind", "> [!INFO]\n> body\n"),
    R("smart-punctuation-off", "\"quotes\" 'single' -- --- ...\n"),
    R("heading-attributes-off", "# Title {#custom .cls}\n", "html", "text", "anchors"),
    R("yaml-metadata-off", "---\ntitle: x\n---\nbody\n"),
    R("plus-metadata-off", "+++\ntitle = 1\n+++\n"),
    R("definition-list-off", "term\n: definition\n"),
    R("superscript-off", "x^sup^ y\n"),
    R("intraword-single-tilde-literal", "H~2~O\n"),
    R("subscript-syntax-is-strikethrough", "x ~2~ y\n"),
    R("wikilinks-off", "[[Page Name]] and [[a|b]]\n"),
    R("old-footnote-syntax-off", "x[^a]\n\n[^a]: first line\nlazy continuation\n\n    indented para\n"),
    R("commonmark-autolink", "<https://example.com/a?b=1>\n"),
    R("hard-and-soft-breaks", "a  \nb\\\nc\nd\n"),
    R("thematic-breaks", "a\n\n***\n\n---\n\n___\n"),
    R("setext-headings", "Title\n=====\n\nSub\n---\n", "html", "text", "toc"),
    R("emphasis-nesting", "***both*** *em **strong** em* _u_ __s__\n"),
    R("backslash-escapes", "\\*not em\\* \\[x\\](y) \\<b\\>\n"),
    R("entities-decoded", "&copy; &amp; &#65; &#x41; &nbsp;x &bogus;\n"),
])

# ------------------------------------------------------------------ raw-html.json
F("lfm1/raw-html.json", "render", [
    R("html-block-type1-script", "<script>\nalert(1)\n</script>\nafter\n"),
    R("html-block-type1-style", "<style>body{color:red}</style>\n"),
    R("html-block-type1-pre-blank-lines", "<pre>\na\n\nb\n</pre>\n"),
    R("html-block-type2-comment", "<!-- comment\nspanning -->\ntext\n"),
    R("html-block-type3-pi", "<?php echo 1; ?>\n"),
    R("html-block-type4-doctype", "<!DOCTYPE html>\n"),
    R("html-block-type5-cdata", "<![CDATA[\nx < y\n]]>\n"),
    R("html-block-type6-div", "<div onclick=\"x()\">\n*not md*\n</div>\n"),
    R("html-block-type7-custom", "<my-element attr=\"1\">\n\npara\n"),
    R("html-block-in-blockquote", "> <div>\n> quoted\n> </div>\n"),
    R("html-block-in-list", "- <details>\n  <summary>x</summary>\n  </details>\n"),
    R("inline-html", "a <b>bold</b> <img src=x onerror=alert(1)> c\n"),
    R("inline-html-multiline", "a <span\ntitle=\"x\">b</span> c\n"),
    R("inline-comment", "a <!-- hidden --> b\n"),
    R("inline-html-adjacent-coalesced", "<b><i>x</i></b>\n"),
    R("entity-in-html-block-kept", "<p>&amp; &lt;</p>\n"),
    R("autolink-in-text-not-in-literal", "<a href=\"https://www.example.com\">www.example.com</a>\n"),
    R("html-block-crlf", "<div>\r\nline\r\n</div>\r\n"),
    R("javascript-in-inline-html", "click <a href=\"javascript:alert(1)\">me</a>\n"),
    R("closing-tags-only", "</div></p>\n"),
])

# ------------------------------------------------------------------ normalise.json
cap_hdr = "|" + "a|" * 100 + "\n" + "|" + "-|" * 100 + "\n"
cap_row = "|" + "a|" * 100 + "\n"
F("lfm1/normalise.json", "render", [
    R("crlf-in-text", "a\r\nb\r\n\r\nc\r\n"),
    R("cr-in-text", "a\rb\r\rc\r"),
    R("crlf-in-code-block", "```\r\nx\r\ny\r\n```\r\n"),
    R("cr-in-indented-code", "    x\r    y\r"),
    R("crlf-in-inline-code", "`a\r\nb`\r\n"),
    R("crlf-in-display-math", "$$a\r\nb$$\r\n", "html", "html_temml", "text"),
    R("crlf-in-link-title-and-dest", "![alt](https://x.example/a \"t1\r\nt2\")\r\n"),
    R("crlf-in-info-string", "```rust\r\nfn x() {}\r\n```\r\n"),
    R("crlf-in-inline-html", "a <b\r\nc> d\r\n"),
    R("text-coalescing-brackets", "a [b] c [!x] d\n", "html", "text"),
    R("text-and-literal-not-merged", "a<b>c\n"),
    R("nul-replaced", "a\u0000b\n"),
    G("event-cap-exact", {"prefix": "x\n\n" * 10 + cap_hdr, "repeat": cap_row, "times": 3471}, "flags", "word_count", "anchors"),
    G("event-cap-plus-one-closes-tags", {"prefix": "x\n\n" * 7 + "*x*\n\n" * 2 + cap_hdr, "repeat": cap_row, "times": 3471}, "flags", "word_count", "html_tail"),
    G("event-cap-at-definition-start", {"prefix": "a[^n] b[^m]\n\n[^m]: kept\n\n" + "x\n\n" * 6 + cap_hdr, "repeat": cap_row, "times": 3471, "suffix": "\n[^n]: lost\n"}, "flags", "anchors", "html_tail"),
    G("event-cap-dangling-footnote", {"prefix": "a[^n] b[^m]\n\n[^m]: kept\n\n" + "x\n\n" * 8 + cap_hdr, "repeat": cap_row, "times": 3471, "suffix": "\n[^n]: lost\n"}, "flags", "anchors", "html_tail"),
])

# ------------------------------------------------------------------ autolink.json
F("lfm1/autolink.json", "render", [
    R("www", "see www.example.com now\n"),
    R("http", "see http://example.com/path now\n"),
    R("https-with-query-fragment", "https://example.com/a?b=c&d=e#frag\n"),
    R("uppercase-prefixes", "HTTPS://Example.COM/X and WWW.EXAMPLE.ORG\n"),
    R("boundary-chars", "*www.a.com* _www.b.com_ ~www.c.com~ (www.d.com)\n"),
    R("no-boundary", "xwww.a.com ahttp://b.com\n"),
    R("start-of-text-event", "**bold**www.example.com\n"),
    R("trailing-punctuation", "www.a.com. www.b.com! www.c.com? www.d.com, www.e.com: www.f.com*\n"),
    R("trailing-quotes", "\"www.a.com\" 'www.b.com'\n"),
    R("entity-suffix", "www.a.com/x&amp; www.b.com/y&hellip; www.c.com/z;\n"),
    R("unbalanced-parens", "www.a.com/x(y) www.b.com/x) (www.c.com/x(y))\n"),
    R("stops-at-lt", "www.a.com/x<y\n"),
    R("underscore-last-two-segments", "www.a_b.com www.x.a_b.com www.a_b.c.com\n"),
    R("no-period", "http://localhost/x https://x\n"),
    R("domain-over-253", "http://" + "a" * 250 + ".com x\n"),
    R("domain-exactly-253", "http://" + "a" * 249 + ".com x\n"),
    R("not-inside-link", "[www.a.com](https://b.com)\n"),
    R("not-inside-image-alt", "![www.a.com](x.png)\n"),
    R("not-inside-code", "`www.a.com` and\n\n    www.b.com\n"),
    R("not-inside-literal", "<x www.a.com>\n"),
    R("email-not-autolinked", "mail me at bob@example.com\n"),
    R("in-heading-and-table", "# www.a.com\n\n| www.b.com |\n|---|\n| http://c.com |\n", "html", "toc"),
    R("adjacent-links", "www.a.com www.b.com\n"),
    R("multibyte-before", "é www.a.com ü\n"),
])

# ------------------------------------------------------------------ spoiler-details.json
SD = ("html", "text", "text_x", "excerpt")
F("lfm1/spoiler-details.json", "render", [
    R("spoiler-basic", "> [!spoiler]\n> the butler did it\n", *SD),
    R("spoiler-case-insensitive", "> [!SPOILER]\n> x\n\n> [!Spoiler]\n> y\n", *SD),
    R("spoiler-trailing-spaces-tab", "> [!spoiler] \t\n> x\n", *SD),
    R("spoiler-rest-not-blank", "> [!spoiler] not alone\n> x\n", *SD),
    R("spoiler-marker-whole-paragraph", "> [!spoiler]\n>\n> para\n", *SD),
    R("spoiler-hard-break", "> [!spoiler]  \n> x\n", *SD),
    R("spoiler-empty", "> [!spoiler]\n", *SD),
    R("details-with-summary", "> [!details] Click me\n> body text\n", *SD),
    R("details-empty-summary", "> [!details]\n> body\n", *SD),
    R("details-inline-formatted-summary", "> [!details] A *fancy* `sum`\n> body\n", *SD),
    R("details-no-space-after-marker", "> [!details]x\n> body\n", *SD),
    R("details-summary-only", "> [!details] Only summary\n", *SD),
    R("details-summary-spans-emphasis-break", "> [!details] *a\n> b* c\n> d\n", *SD),
    R("nested-spoiler-in-details", "> [!details] Outer\n> > [!spoiler]\n> > inner\n", *SD),
    R("alert-untouched", "> [!NOTE]\n> [!spoiler]\n> x\n", *SD),
    R("marker-made-link-by-reference", "> [!spoiler]\n> x\n\n[!spoiler]: https://example.com\n", *SD),
    R("literal-never-matches", "> <b>[!spoiler]</b>\n> x\n", *SD),
    R("marker-not-first-paragraph", "> intro\n>\n> [!spoiler]\n> x\n", *SD),
    R("leading-spoiler-excerpt", "> [!spoiler]\n> secret words\n\nvisible words\n", *SD),
    R("details-summary-trailing-nbsp", "> [!details] Sum\u00a0\n> body\n", *SD),
])

# ------------------------------------------------------------------ figure.json
F("lfm1/figure.json", "render", [
    R("figure-external", "![alt](https://img.example/a.png \"A caption\")\n", "html", "text", "styled"),
    R("figure-cid", "![alt](cid:" + CID1 + " \"Cap\")\n", "html", "text", "styled", "media_used", media=[media(CID1)]),
    R("no-title-not-figure", "![alt](https://img.example/a.png)\n"),
    R("whitespace-title-not-figure", "![alt](https://img.example/a.png \"   \")\n"),
    R("extra-text-not-figure", "see ![alt](https://img.example/a.png \"t\")\n"),
    R("two-images-not-figure", "![a](https://x.example/1.png \"t\") ![b](https://x.example/2.png \"u\")\n"),
    R("image-in-link-not-figure", "[![a](https://x.example/1.png \"t\")](https://y.example)\n"),
    R("caption-trimmed-not-markdown", "![a](https://x.example/1.png \"  *not em* <b>  \")\n"),
    R("figure-alt-only-scheme", "![alt text](ftp://x.example/a \"Cap\")\n"),
    R("figure-in-list", "- ![a](https://x.example/1.png \"Cap\")\n"),
])

# ------------------------------------------------------------------ mention.json
MN = ("html", "text", "mentions", "links")
F("lfm1/mention.json", "render", [
    R("plc-mention", f"hi [@alice](at://{DID})\n", *MN),
    R("web-mention", f"hi [@bob](at://{DIDWEB})\n", *MN),
    R("other-method-dropped", "hi [@k](at://did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK)\n", *MN),
    R("handle-authority-dropped", "hi [@alice](at://alice.example.com)\n", *MN),
    R("https-never-mention", "hi [@alice](https://bsky.app/profile/alice)\n", *MN),
    R("no-at-sign-profile-link", f"see [alice](at://{DID})\n", *MN),
    R("formatted-inner-not-mention", f"see [*@alice*](at://{DID})\n", *MN),
    R("at-inside-later-text", f"see [x @alice](at://{DID})\n", *MN),
    R("dedup-and-order", f"[@a](at://{DID2}) [@b](at://{DID}) [@c](at://{DID2})\n", *MN),
    R("web-long-did", "x [@w](at://did:web:" + "a" * 40 + ".example.com)\n", *MN),
    R("mention-in-heading-toc-uses-source", f"# Hi [@alice](at://{DID})\n", "html", "toc", "mentions"),
    R("did-plc-wrong-length", "[@x](at://did:plc:abc)\n", *MN),
    R("did-plc-uppercase", "[@x](at://did:plc:ABCDEFGHIJKLMNOPQRSTUVWX)\n", *MN),
    R("did-web-port-rejected", "[@x](at://did:web:example.com%3A8080)\n", *MN),
])
F("lfm1/short-did.json", "short_did", [
    V("plc", {"did": DID}, "out"),
    V("web-short", {"did": DIDWEB}, "out"),
    V("web-exactly-40", {"did": "did:web:" + "a" * 28 + ".com"}, "out"),
    V("web-41", {"did": "did:web:" + "a" * 29 + ".com"}, "out"),
])

# ------------------------------------------------------------------ links.json
LK = ("html", "text")
F("lfm1/links.json", "render", [
    R("https", "[a](https://example.com/p)\n", *LK),
    R("http", "[a](http://example.com)\n", *LK),
    R("scheme-and-host-lowercased", "[a](HTTPS://EXAMPLE.com/Path)\n", *LK),
    R("relative-dropped", "[a](/path) [b](page.html) [c](../up)\n", *LK),
    R("protocol-relative-dropped", "[a](//example.com/x)\n", *LK),
    R("javascript-dropped", "[a](javascript:alert(1)) [b](JaVaScRiPt:alert(1))\n", *LK),
    R("data-file-vbscript-dropped", "[a](data:text/html,x) [b](file:///etc/passwd) [c](vbscript:x)\n", *LK),
    R("userinfo-dropped", "[a](https://user:pw@example.com) [b](https://user@example.com)\n", *LK),
    R("backslash-dropped", "[a](<https://example.com\\@evil.com>)\n", *LK),
    R("control-and-space-dropped", "[a](<https://exa mple.com>) [b](<https://example.com/\u0007>)\n", *LK),
    R("bidi-dropped", "[a](https://example.com/\u202ex)\n", *LK),
    R("zero-width-dropped", "[a](https://exa\u200bmple.com)\n", *LK),
    R("non-ascii-host-dropped", "[a](https://exämple.com) [b](https://xn--exmple-cua.com)\n", *LK),
    R("path-percent-encoding", "[a](https://example.com/ä/ö?q=ü#ß)\n[b](<https://example.com/a b>)\n[c](https://example.com/%41%zz)\n", *LK),
    R("port", "[a](https://example.com:8443/x) [b](https://example.com:99999/) [c](https://example.com:/)\n", *LK),
    R("ipv6", "[a](http://[2001:db8::1]:8080/x) [b](http://[zz]/)\n", *LK),
    G("length-3000-accepted", {"prefix": "[a](https://example.com/", "repeat": "a", "times": 3000 - len("https://example.com/"), "suffix": ")\n"}, "text", "anchors", "html_tail"),
    G("length-3001-dropped", {"prefix": "[a](https://example.com/", "repeat": "a", "times": 3001 - len("https://example.com/"), "suffix": ")\n"}, "html"),
    R("mailto", "[mail](mailto:Bob@Example.COM?subject=grüße)\n", *LK),
    R("mailto-space-dropped", "[mail](<mailto:bob@example.com?subject=hi there>)\n", *LK),
    R("mailto-empty-or-slash-dropped", "[a](mailto:) [b](mailto:/x)\n", *LK),
    R("email-autolink", "<bob@example.com>\n", *LK),
    R("fragment-anchor", "[a](#Section-One) [b](#md-fn-1) [c](#%C3%9Cber)\n", "html", "text", "anchors"),
    R("fragment-bad-chars-dropped", "[a](#a.b) [b](#/x) [c](#) [d](#a%20b) [e](#%FF)\n", *LK),
    R("fragment-512-boundary", "[a](#" + "a" * 512 + ") [b](#" + "a" * 513 + ")\n", *LK),
    R("title-never-rendered", "[a](https://example.com \"Title text\")\n", *LK),
    R("reference-link", "[a][ref]\n\n[ref]: https://example.com/r\n", *LK),
    R("host-display-mismatch", "[google.com](https://evil.com) [https://www.paypal.com/x](https://evil.example/x)\n", *LK),
    R("host-display-match", "[example.com](https://www.example.com) [https://Example.com/a](https://example.com/b) [Click here](https://evil.com)\n", *LK),
    R("host-display-at-handle", "[@bob.bsky.social](https://evil.com) [bob@good.com](mailto:bob@evil.com)\n", *LK),
    R("host-display-with-port-text", "[good.com:443/x](https://evil.com)\n", *LK),
    R("host-display-not-host-like", "[not.a host](https://evil.com) [a..b](https://evil.com) [x.y!](https://evil.com)\n", *LK),
    R("unknown-scheme-dropped", "[a](ftp://example.com) [b](irc://x) [c](tel:123)\n", *LK),
    R("link-text-keeps-formatting", "[**bold** `code`](https://example.com)\n", *LK),
])

# ------------------------------------------------------------------ at-links.json
AL = ("html", "links", "mentions")
P = f"at://{DID}/{NS}.post/3kabc2def"
F("lfm1/at-links.json", "render", [
    R("record", f"[post]({P})\n", *AL),
    R("profile", f"[profile](at://{DID})\n", *AL),
    R("collection-only-dropped", f"[c](at://{DID}/{NS}.post)\n", *AL),
    R("query-dropped", f"[q]({P}?x=1)\n", *AL),
    R("fragment-dropped", f"[f]({P}#x)\n", *AL),
    R("trailing-slash-dropped", f"[t]({P}/) [u](at://{DID}/)\n", *AL),
    R("foreign-nsid", f"[b](at://{DID}/app.bsky.feed.post/3kabc)\n", *AL),
    R("temp-nsid", f"[t](at://{DID}/{NS}.temp.comment/3kxyz)\n", *AL),
    R("invalid-rkey", f"[a](at://{DID}/{NS}.post/..) [b](at://{DID}/{NS}.post/a%20b)\n", *AL),
    R("invalid-nsid", f"[a](at://{DID}/notansid/x) [b](at://{DID}/1a.b.c/x)\n", *AL),
    R("uppercase-nsid-authority", f"[a](at://{DID}/Online.7778777.lessdugin.post/x)\n", *AL),
    R("handle-authority-record-dropped", f"[a](at://alice.example.com/{NS}.post/3k)\n", *AL),
    R("links-dedup-order-and-profiles-excluded", f"[a]({P}) [b](at://{DID}) [c](at://{DID2}/{NS}.comment/x) [d]({P})\n", *AL),
    R("did-web-record", f"[w](at://{DIDWEB}/{NS}.forum/self)\n", *AL),
    R("uppercase-scheme", f"[u](AT://{DID})\n", *AL),
    R("autolink-at-uri", f"<at://{DID}>\n", *AL),
])

# ------------------------------------------------------------------ images.json
IM = ("html", "text", "media_used")
M1 = media(CID1, "image/png", alt="media alt", ar=[800, 600])
F("lfm1/images.json", "render", [
    R("cid-hit", f"![shown](cid:{CID1})\n", *IM, media=[M1]),
    R("cid-hit-media-alt-fallback", f"x ![](cid:{CID1})\n", *IM, media=[M1]),
    R("cid-hit-no-alt", f"x ![ ](cid:{CID1})\n", *IM, media=[media(CID1)]),
    R("cid-miss", f"x ![gone](cid:{CID2})\n", *IM, media=[M1]),
    R("cid-malformed-miss", "x ![bad](cid:QmNotBase32) ![b2](cid:b) ![b3](cid:BAFY)\n", *IM, media=[M1]),
    R("cid-case-insensitive-prefix", f"x ![a](CID:{CID1}) ![b](Cid:{CID1})\n", *IM, media=[M1]),
    R("cid-duplicate-lowest-index", f"x ![a](cid:{CID1})\n", *IM, media=[media(CID2), media(CID1, alt="first"), media(CID1, alt="second")]),
    R("mime-video", f"x ![clip](cid:{CID1})\n", *IM, media=[media(CID1, "video/mp4", name="clip.mp4")]),
    R("mime-audio-name-label", f"x ![](cid:{CID1})\n", *IM, media=[media(CID1, "audio/mpeg", name="song.mp3")]),
    R("mime-file-bad-name-ignored", f"x ![](cid:{CID1})\n", *IM, media=[media(CID1, "application/pdf", name="../etc/passwd")]),
    R("mime-image-types", f"![a](cid:{CID1}) ![b](cid:{CID2}) ![c](cid:{CID3}) ![d](cid:{CID4})\n", *IM,
      media=[media(CID1, "image/jpeg"), media(CID2, "image/gif"), media(CID3, "image/webp"), media(CID4, "image/avif")]),
    R("mime-svg-not-image", f"x ![a](cid:{CID1})\n", *IM, media=[media(CID1, "image/svg+xml")]),
    R("aspect-clamp-wide", f"x ![a](cid:{CID1})\n", *IM, media=[media(CID1, ar=[100000, 10])]),
    R("aspect-clamp-tall", f"x ![a](cid:{CID1})\n", *IM, media=[media(CID1, ar=[3, 1000])]),
    R("external-image", "x ![a cat](https://img.example/cat.png) y\n", *IM),
    R("external-image-empty-alt-uses-host", "x ![](https://img.example/cat.png)\n", *IM),
    R("external-image-in-link", "[![a cat](https://img.example/cat.png)](https://example.com)\n", *IM),
    R("external-image-host-like-alt", "x ![google.com](https://evil.example/a.png)\n", *IM),
    R("other-scheme-alt-text", "x ![alt *em*](data:image/png;base64,AAAA) ![rel](a.png) ![js](javascript:x)\n", *IM),
    R("alt-is-plain-projection", "x ![a **b** `c` $d$](https://img.example/x.png)\n", *IM),
    R("nested-image-alt", "x ![a ![b](c.png) d](https://img.example/x.png)\n", *IM),
    R("cid-image-in-link", f"[![a](cid:{CID1})](https://example.com)\n", *IM, media=[M1]),
    R("lone-cid-image-paragraph-styled", f"![a](cid:{CID1})\n", "html", "styled", media=[M1]),
])

# ------------------------------------------------------------------ gallery.json
GA = ("html", "media_used")
F("lfm1/gallery.json", "render", [
    R("all-unreferenced", "text\n", *GA, media=[media(CID1, alt="one"), media(CID2, "video/mp4", alt="two")]),
    R("some-referenced", f"![x](cid:{CID2})\n", *GA, media=[media(CID1, alt="one"), media(CID2), media(CID3, alt="three")]),
    R("duplicate-cid-both-used", f"![x](cid:{CID1})\n", *GA, media=[media(CID1, alt="a"), media(CID2, alt="b"), media(CID1, alt="c")]),
    R("all-used-no-gallery", f"![x](cid:{CID1}) ![y](cid:{CID2})\n", *GA, media=[media(CID1), media(CID2)]),
    R("gallery-after-footnotes", "a[^1]\n\n[^1]: n\n", *GA, media=[media(CID1, alt="g")]),
    R("gallery-invalid-cid", "x\n", *GA, media=[media("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG", alt="v0")]),
    R("gallery-styled", "x\n", "styled", media=[media(CID1, alt="one"), media(CID2, "audio/ogg", name="a.ogg")]),
    R("miss-does-not-use", f"![x](cid:{CID2})\n", *GA, media=[media(CID1)]),
])

# ------------------------------------------------------------------ ids.json
ID = ("html", "anchors", "toc")
F("lfm1/ids.json", "render", [
    R("basic-slug", "# Hello, World!\n", *ID),
    R("unicode-slug", "# Ünïcödé Ελληνικά 日本語\n", *ID),
    R("punctuation-and-emoji", "# C++ & Rust 🦀 — fast!\n", *ID),
    R("nfc-before-slug", "# Cafe\u0301\n", *ID),
    R("empty-slugs", "# !!!\n\n# ???\n\n# ...\n", *ID),
    R("dedup", "# Foo\n\n# Foo\n\n# Foo\n", *ID),
    R("dedup-literal-collision", "# Foo\n\n# Foo-1\n\n# Foo\n\n# Foo-1\n", *ID),
    R("formatting-and-code", "# A *b* `c` [d](https://x.example) $e$\n", *ID),
    R("spaces-uncollapsed", "#   a   b  \n", *ID),
    R("underscore-hyphen-kept", "# snake_case and kebab-case\n", *ID),
    R("ids-in-containers", "> # Quoted\n\n- # Listed\n\n> [!details] s\n> # In details\n", *ID),
    R("ids-in-footnotes-anchor-order", "# Top\n\nref[^1]\n\n[^1]: # Note heading\n", *ID),
    R("toc-levels", "# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six\n", *ID),
    R("uppercase-unicode-lowercase", "# ΣΊΣΥΦΟΣ İstanbul\n", *ID),
    R("numbers-letter-number", "# Ⅻ ½ ²\n", *ID),
])

# ------------------------------------------------------------------ footnotes.json
FN = ("html", "anchors", "text")
F("lfm1/footnotes.json", "render", [
    R("ordinal-by-first-reference", "b[^b] a[^a]\n\n[^a]: A\n[^b]: B\n", *FN),
    R("multiple-references", "x[^n] y[^n] z[^n]\n\n[^n]: N\n", *FN),
    R("undefined-label-literal", "x[^nope]\n", *FN),
    R("duplicate-definitions-first-wins", "x[^d]\n\n[^d]: first\n\n[^d]: second\n", *FN),
    R("unreferenced-not-rendered-but-in-p", "x\n\n[^u]: unreferenced\n", *FN),
    R("reference-inside-definition", "x[^a]\n\n[^a]: see[^b]\n\n[^b]: B\n", *FN),
    R("definition-before-body-reference-order", "[^a]: A refs[^b]\n\n[^b]: B\n\nx[^a]\n", *FN),
    R("reference-in-link-text-prevents-link", "[link[^1]](https://example.com)\n\n[^1]: N\n", *FN),
    R("case-insensitive-labels", "x[^Note] y[^NOTE]\n\n[^note]: n\n", *FN),
    R("reference-only-from-unrendered-definition", "x\n\n[^u]: u[^v]\n\n[^v]: v\n", *FN),
    R("self-reference", "x[^s]\n\n[^s]: me[^s]\n", *FN),
    R("multi-paragraph-definition", "x[^m]\n\n[^m]: para one\n\n    para two\n", *FN),
    R("footnote-styled", "x[^1]\n\n[^1]: note\n", "styled"),
])

# ------------------------------------------------------------------ blocks.json
F("lfm1/blocks.json", "render", [
    R("alerts-all", "> [!NOTE]\n> n\n\n> [!TIP]\n> t\n\n> [!IMPORTANT]\n> i\n\n> [!WARNING]\n> w\n\n> [!CAUTION]\n> c\n"),
    R("table-alignment", "| l | c | r | n |\n|:--|:-:|--:|---|\n| 1 | 2 | 3 | 4 |\n"),
    R("table-escaped-pipe", "| a \\| b | `c \\| d` |\n|---|---|\n| e | f |\n"),
    R("table-header-only", "| a | b |\n|---|---|\n"),
    R("table-ragged-rows", "| a | b |\n|---|---|\n| 1 |\n| 1 | 2 | 3 |\n"),
    R("task-list-loose", "- [ ] a\n\n- [x] b\n"),
    R("code-language-classes", "```Rust\nx\n```\n\n```c#\ny\n```\n\n```  python3 extra\nz\n```\n\n```" + "a" * 33 + "\nw\n```\n\n~~~\nplain\n~~~\n"),
    R("indented-code", "    code <b>\n      more\n"),
    R("ol-start", "3. three\n4. four\n\n- x\n\n0. zero\n\n- y\n\n1. one\n"),
    R("tight-and-loose-lists", "- a\n- b\n\n* c\n\n* d\n"),
    R("nested-lists", "- a\n  - b\n    1. c\n"),
    R("blockquote-nested", "> a\n> > b\n> c\n"),
    R("list-item-empty", "- \n- x\n"),
    R("code-block-empty", "```\n```\n"),
    R("rule-in-list", "- a\n\n  ***\n"),
])

# ------------------------------------------------------------------ math.json
MA = ("html", "html_temml", "text", "flags")
F("lfm1/math.json", "render", [
    R("inline", "Euler: $e^{i\\pi}+1=0$.\n", *MA),
    R("display", "$$\\int_0^1 x\\,dx = \\frac{1}{2}$$\n", *MA),
    R("display-in-paragraph", "before $$x$$ after\n", *MA),
    R("currency-not-math", "It costs $5 and $10.\n", *MA),
    R("currency-escaped", "\\$5 and \\$10\n", *MA),
    R("disabled-href", "$\\href{javascript:alert(1)}{x}$\n", *MA),
    R("disabled-url", "$\\url{https://x}$\n", *MA),
    R("disabled-includegraphics-class-style", "$\\includegraphics{a}$ $\\class{c}{x}$ $\\style{color:red}{x}$\n", *MA),
    R("disabled-def-family", "$\\def\\a{b}$ $\\gdef\\a{b}$ $\\edef\\a{b}$ $\\xdef\\a{b}$ $\\let\\a\\b$ $\\futurelet\\a\\b$\n", *MA),
    R("disabled-newcommand-family", "$\\newcommand{\\a}{b}$ $\\renewcommand{\\a}{b}$ $\\providecommand{\\a}{b}$ $\\newenvironment{a}{b}{c}$ $\\renewenvironment{a}{b}{c}$\n", *MA),
    R("disabled-declare-catcode-input", "$\\DeclareMathOperator{\\a}{b}$ $\\DeclareRobustCommand{\\a}{b}$ $\\catcode1=1$ $\\input{x}$ $\\include{x}$\n", *MA),
    R("disabled-html-prefix", "$\\htmlClass{a}{x}$ $\\htmlId{a}{x}$ $\\html@mathml{a}{b}$\n", *MA),
    R("disabled-inside-text", "$\\text{\\href{x}{y}}$\n", *MA),
    R("disabled-after-comment", "$$x % \\def\\a{b}\n+1$$\n", *MA),
    R("double-backslash-not-control-word", "$$a \\\\href b$$\n", *MA),
    G("size-4096-accepted", {"prefix": "$", "repeat": "x", "times": 4096, "suffix": "$\n"}, "flags", "html_temml"),
    G("size-4097-error", {"prefix": "$", "repeat": "x", "times": 4097, "suffix": "$\n"}, "flags"),
    G("depth-64-accepted", {"prefix": "$" + "{" * 64 + "x", "repeat": "}", "times": 64, "suffix": "$\n"}, "html", "flags"),
    G("depth-65-error", {"prefix": "$" + "{" * 65 + "x", "repeat": "}", "times": 65, "suffix": "$\n"}, "html", "flags"),
    R("depth-left-right-begin", "$\\left(\\begin{matrix}a\\end{matrix}\\right)$\n", *MA),
    R("negative-depth-error", "$a}x{b$\n", *MA),
    R("escaped-braces-not-groups", "$\\{x\\}$\n", *MA),
    R("unknown-command-error", "$\\notacommand x$\n", *MA),
    R("engine-error-ampersand", "$a & b$\n", *MA),
    R("filter-style-color", "$\\color{red}{x}$\n", *MA),
    R("filter-class-cancel", "$\\cancel{x}$\n", *MA),
    R("filter-id-align-tag", "$$\\begin{align} a &= b \\label{eq} \\end{align}$$\n", *MA),
    R("filter-text-styles-unwrapped", "$\\textbf{bold} \\texttt{mono} \\textit{it} \\emph{e}$\n", *MA),
    R("filter-mspace-no-self-closing", "$a\\,b\\quad c$\n", *MA),
    R("filter-mathvariant-and-entities", "$\\mathrm{d}x \\sin x < y > z \\& w$\n", *MA),
    R("output-depth-64-ok", "$" + "\\left(" * 62 + "x" + "\\right)" * 62 + "$\n", "flags", "html_temml"),
    R("output-depth-cap", "$" + "\\left(" * 64 + "x" + "\\right)" * 64 + "$\n", "flags", "html_temml"),
    R("output-size-cap", "$" + "\\pmod{x}" * 512 + "$\n", "flags"),
    R("output-size-under-cap", "$" + "\\pmod{x}" * 300 + "$\n", "flags"),
    R("engine-nesting-guard", "$" + "\\sqrt" * 100 + " x$\n", "flags", "html"),
    R("math-in-heading-and-link", "# $x^2$\n\n[$y$](https://example.com)\n", *MA),
    R("matrix", "$$\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}$$\n", *MA),
    R("display-math-styled", "a $b$\n\n$$c\nd$$\n", "styled"),
])

# ------------------------------------------------------------------ limits.json
def nest(open_, close, n, inner="x"):
    return open_ * n + inner + close * n
LM = ("html", "flags")
F("lfm1/limits.json", "render", [
    G("blockquote-64-ok", {"prefix": "", "repeat": ">", "times": 63, "suffix": " x\n"}, "html_rle", "flags"),
    G("blockquote-64-paragraph-bare", {"prefix": "", "repeat": ">", "times": 64, "suffix": " x\n"}, "html_rle", "flags"),
    G("blockquote-flattened", {"prefix": "", "repeat": ">", "times": 70, "suffix": " deep *text*\n"}, "html_rle", "flags", "text"),
    G("list-at-63-flattened-wrapped", {"prefix": "", "repeat": ">", "times": 63, "suffix": " - a\n" + ">" * 63 + " - b\n"}, "html_rle", "flags"),
    G("list-flattened", {"prefix": "", "repeat": "- ", "times": 70, "suffix": "deep\n"}, "flags", "html_rle"),
    G("emphasis-dropped-at-boundary", {"prefix": "", "repeat": ">", "times": 63, "suffix": " a *b **c***\n"}, "html_rle", "flags"),
    G("table-at-boundary", {"prefix": "", "repeat": ">", "times": 61, "suffix": " | a |\n" + ">" * 61 + " |---|\n" + ">" * 61 + " | **b** |\n"}, "html_rle", "flags"),
    G("table-flattened", {"prefix": "", "repeat": ">", "times": 62, "suffix": " | a |\n" + ">" * 62 + " |---|\n" + ">" * 62 + " | b |\n"}, "html_rle", "flags"),
    G("code-block-flattened", {"prefix": "", "repeat": ">", "times": 63, "suffix": " ```\n" + ">" * 63 + " code\n" + ">" * 63 + " ```\n"}, "html_rle", "flags"),
    G("spoiler-at-62-fits", {"prefix": "", "repeat": ">", "times": 63, "suffix": " [!spoiler]\n" + ">" * 63 + " hidden\n"}, "html_rle", "flags"),
    G("spoiler-flattened-reveals", {"prefix": "", "repeat": ">", "times": 64, "suffix": " [!spoiler]\n" + ">" * 64 + " hidden\n"}, "html_rle", "flags", "text_x"),
    G("figure-at-depth-limit", {"prefix": "", "repeat": ">", "times": 63, "suffix": " ![a](https://x.example/i.png \"Cap\")\n"}, "html_rle", "flags"),
    G("heading-flattened-no-id", {"prefix": "", "repeat": ">", "times": 64, "suffix": " # H\n"}, "html_rle", "flags", "anchors", "toc"),
    G("leaf-exemption", {"prefix": "", "repeat": ">", "times": 63, "suffix": " `code` ![i](https://x.example/i.png) $x$ [^1]\n\n[^1]: n\n"}, "html_rle", "flags"),
    G("link-dropped-at-boundary", {"prefix": "", "repeat": ">", "times": 63, "suffix": " [a](https://example.com) [b](#x)\n"}, "html_rle", "flags"),
    G("footnote-base-depth", {"prefix": "x[^1]\n\n[^1]: ", "repeat": ">", "times": 61, "suffix": " deep *e*\n"}, "html_rle", "flags"),
    G("too-large-1000001", {"prefix": "", "repeat": "a", "times": 1000001}, "html", "text", "flags", "error", "styled", "anchors"),
    G("exactly-1000000-ok", {"prefix": "", "repeat": "a ", "times": 500000}, "flags", "error", "word_count", "read_minutes"),
    V("encoding-error", {"text": "", "encoding_error": True}, "html", "text", "flags", "error", "styled"),
    R("unknown-profile", "text\n", "html", "flags", profile="lfm/2"),
    R("lfm1-profile", "text\n", "flags", profile="lfm/1"),
    G("deep-brackets-fast", {"prefix": "", "repeat": "[", "times": 10000, "suffix": "x"}, "flags", "word_count"),
    G("deep-brackets-images", {"prefix": "", "repeat": "![", "times": 5000, "suffix": "x" + "](y)" * 5000}, "flags", "word_count"),
    G("deep-emphasis", {"prefix": "", "repeat": "*a ", "times": 5000, "suffix": "x" + " b*" * 5000}, "flags", "word_count"),
    G("deep-lists-and-quotes", {"prefix": "", "repeat": "> - ", "times": 3000, "suffix": "x"}, "flags", "word_count"),
])

# ------------------------------------------------------------------ serialise.json
F("lfm1/serialise.json", "render", [
    R("text-escaping", "a & b < c > d \" e ' f\n"),
    R("nbsp-text-and-attr", "a\u00a0b ![x\u00a0y](cid:" + CID1 + ")\n", "html", media=[media(CID1)]),
    R("attribute-escaping", "![a \"q\" <t> & z](cid:" + CID1 + ")\n", "html", media=[media(CID1)]),
    R("media-alt-cr-nul-normalised", f"![](cid:{CID1})\n", "html", media=[media(CID1, alt="a\r\nb\rc\u0000d")]),
    R("boolean-attributes", "- [x] y\n- [ ] n\n", "html"),
    R("attribute-order-img", f"![a](cid:{CID1})\n", "html", media=[media(CID1, ar=[4, 3])]),
    R("newline-placement", "# H\n\n> q\n\n- a\n\n| t |\n|---|\n| d |\n\n```\nc\n```\n\n---\n\na  \nb\n", "html"),
    R("dir-auto-everywhere", "p\n\n# h\n\n- li\n\n| th |\n|---|\n| td |\n\n> [!details] s\n> x\n\n![a](https://x.example/i.png \"cap\")\n", "html"),
    R("empty-document", "", "html", "text", "anchors", "styled"),
    R("whitespace-only-document", "  \n\t\n", "html", "text"),
])

# ------------------------------------------------------------------ projection.json
PJ = ("text", "text_x", "word_count")
WS25 = "".join(chr(c) for c in [0x9, 0xA, 0xB, 0xC, 0xD, 0x20, 0x85, 0xA0, 0x1680] + list(range(0x2000, 0x200B)) + [0x2028, 0x2029, 0x202F, 0x205F, 0x3000])
F("lfm1/projection.json", "render", [
    R("text-code-math", "a `b` $c$ $$d$$\n", *PJ),
    R("links-and-urls", "[text](https://url.example) <https://auto.example> www.ext.example\n", *PJ),
    R("image-alt-and-figure-caption", "![alt](https://x.example/i.png)\n\n![falt](https://x.example/i.png \"Cap\")\n", *PJ),
    R("block-boundaries", "# H\n\npara\n\n> q\n\n- a\n- b\n\n```\ncode\n```\n\n***\n\nend\n", *PJ),
    R("table-cells", "| a | b |\n|---|---|\n| c | d |\n", *PJ),
    R("soft-and-hard-breaks", "a\nb  \nc\n", *PJ),
    R("footnotes-at-source-position", "x[^1] y\n\n[^1]: note\n\nz\n", *PJ),
    R("task-markers-and-refs-nothing", "- [x] task[^1]\n\n[^1]: n\n", *PJ),
    R("raw-html-literal", "a <b>c</b> <!-- d -->\n", *PJ),
    R("nfc", "Cafe\u0301 n\u0303\n", *PJ),
    R("all-25-white-space", "a" + WS25.replace("\n", "") + "b\n", *PJ),
    R("leading-trailing-trimmed", "\u3000\u00a0a\u00a0\u3000\n", *PJ),
    R("astral-plane", "𝔘𝔫𝔦 😀 x\n", *PJ),
    R("mention-source-text", f"[@alice](at://{DID}) hi\n", *PJ),
    R("spoiler-p-vs-px", "a\n\n> [!spoiler]\n> hidden\n\nb\n", *PJ),
    R("details-marker-removed", "> [!details] Sum\n> body\n", *PJ),
    R("control-chars-kept", "a\u0001b\u007fc\u0085d\n", *PJ),
    R("unreferenced-footnote-in-p", "x\n\n[^u]: u text\n", *PJ),
])

# ------------------------------------------------------------------ excerpt.json
EX = ("excerpt", "word_count", "read_minutes")
F("lfm1/excerpt.json", "render", [
    R("short", "Hello world.\n", *EX),
    G("exactly-500-graphemes", {"prefix": "", "repeat": "ab ", "times": 166, "suffix": "ab\n"}, *EX),
    G("501-graphemes", {"prefix": "", "repeat": "ab ", "times": 166, "suffix": "abc\n"}, *EX),
    G("no-space-cut-at-499", {"prefix": "", "repeat": "x", "times": 600}, *EX),
    G("byte-cap-long-clusters", {"prefix": "", "repeat": "e" + "\u0301" * 20, "times": 200}, *EX),
    G("byte-cap-with-spaces", {"prefix": "", "repeat": "é" * 30 + " ", "times": 100}, *EX),
    R("leading-spoiler", "> [!spoiler]\n> spoiled words here\n\nreal text\n", *EX),
    R("empty", "", *EX),
    G("word-count-374-one-minute", {"prefix": "", "repeat": "w ", "times": 374}, "word_count", "read_minutes"),
    G("word-count-375-two-minutes", {"prefix": "", "repeat": "w ", "times": 375}, "word_count", "read_minutes"),
    R("toc-top-level-only", "# A\n\n> # B\n\n## C\n\n- # D\n\n[^1]: # E\n", "toc"),
    R("first-cluster-is-space-cut-rule", "a" + " b" * 300 + "\n", *EX),
])

# ------------------------------------------------------------------ quote.json
def Q(name, quote, target, *fields):
    inp = {"quote": quote}
    if target is not None:
        if isinstance(target, dict):
            inp["target_gen"] = target
        else:
            inp["target"] = target
    return V(name, inp, *(fields or ("result",)))
T1 = "the cat sat on the mat with the hat"
F("lfm1/quote.json", "quote", [
    Q("zero-occurrences", {"exact": "dog"}, T1),
    Q("one-occurrence", {"exact": "cat"}, T1),
    Q("many-lowest-offset", {"exact": "the"}, T1),
    Q("prefix-scoring", {"exact": "the", "prefix": "with "}, T1),
    Q("suffix-scoring", {"exact": "the", "suffix": " mat"}, T1),
    Q("prefix-and-suffix", {"exact": "at", "prefix": "the h", "suffix": ""}, T1),
    Q("start-tiebreak", {"exact": "the", "start": 16}, T1),
    Q("start-tiebreak-nearest", {"exact": "ab", "start": 2}, "ab_ab_ab"),
    Q("start-tiebreak-equal-distance-lowest", {"exact": "ab", "start": 2}, "ab__ab"),
    Q("score-beats-start", {"exact": "the", "suffix": " cat", "start": 30}, T1),
    Q("overlapping", {"exact": "aa", "suffix": "a!"}, "aaaa!"),
    Q("normalisation-exact-trimmed-ws", {"exact": "  cat\n sat\u00a0 "}, T1),
    Q("normalisation-nfc", {"exact": "Cafe\u0301"}, "a Café b"),
    Q("astral-offsets", {"exact": "x"}, "😀😀 x"),
    Q("empty-exact-orphaned", {"exact": "   "}, T1),
    Q("quote-2000-bytes-ok", {"exact": "a" * 2000}, "b" + "a" * 2000),
    Q("quote-2001-bytes-orphaned", {"exact": "a" * 2001}, "a" * 2001),
    Q("target-200000-boundary", {"exact": "zz"}, {"prefix": "", "repeat": "y", "times": 199998, "suffix": "zz"}),
    Q("target-beyond-200000-not-searched", {"exact": "zz"}, {"prefix": "", "repeat": "y", "times": 199999, "suffix": "zz"}),
    Q("target-cut-on-char-boundary", {"exact": "é"}, {"prefix": "", "repeat": "y", "times": 199999, "suffix": "é"}),
    Q("cap-1000-occurrences", {"exact": "a", "suffix": "b"}, {"prefix": "", "repeat": "a", "times": 1500, "suffix": "b"}),
    Q("pending", {"exact": "cat"}, None),
    Q("empty-target-orphaned", {"exact": "cat"}, ""),
    Q("qkey", {"exact": " a\n b "}, T1, "result", "qkey"),
    Q("qkey-no-quote", {}, T1, "qkey"),
    Q("cli-first-occurrence", {"exact": "the  "}, T1, "first"),
    Q("cli-no-occurrence", {"exact": "dog"}, T1, "first"),
])

# ------------------------------------------------------------------ safe-href.json
def S(name, raw, ctx="markdown"):
    return V(name, {"raw": raw, "ctx": ctx}, "href")
F("lfm1/safe-href.json", "safe_href", [
    S("https", "https://example.com/a?b#c"),
    S("http-uppercase", "HTTP://EXAMPLE.COM"),
    S("empty", ""),
    V("over-3000", {"raw_gen": {"prefix": "https://e.com/", "repeat": "a", "times": 2987}, "ctx": "markdown"}, "href"),
    V("exactly-3000", {"raw_gen": {"prefix": "https://e.com/", "repeat": "a", "times": 2986}, "ctx": "markdown"}, "href"),
    S("space", "https://e.com/a b"),
    S("tab", "https://e.com/\tx"),
    S("del", "https://e.com/\u007f"),
    S("c1", "https://e.com/\u0085"),
    S("bidi-alm", "https://e.com/\u061c"),
    S("lrm", "https://e.com/\u200e"),
    S("lri", "https://e.com/\u2066"),
    S("line-separator", "https://e.com/\u2028"),
    S("zwj", "https://e.com/\u200d"),
    S("word-joiner", "https://e.com/\u2060"),
    S("bom", "https://e.com/\ufeff"),
    S("backslash", "https:\\\\e.com"),
    S("relative", "/a/b"),
    S("protocol-relative", "//e.com"),
    S("no-scheme-colon-later", "a/b:c"),
    S("javascript", "javascript:alert(1)"),
    S("javascript-mixed-case", "jAvAsCrIpT:alert(1)"),
    S("data", "data:text/html;base64,PHNjcmlwdD4="),
    S("vbscript", "vbscript:msgbox"),
    S("file", "file:///etc/passwd"),
    S("http-no-slashes", "http:e.com"),
    S("userinfo", "https://a@e.com"),
    S("userinfo-after-path-ok", "https://e.com/a@b"),
    S("empty-host", "https:///x"),
    S("dots-only-host", "https://../x"),
    S("underscore-host", "https://a_b.e.com"),
    S("non-ascii-host", "https://ü.com"),
    S("percent-host", "https://e%2Ecom"),
    S("port-ok", "https://e.com:65535/"),
    S("port-too-big", "https://e.com:65536/"),
    S("port-six-digits", "https://e.com:000080/"),
    S("port-empty", "https://e.com:/"),
    S("ipv6", "http://[::1]/"),
    S("ipv6-port", "http://[::1]:8080/x"),
    S("ipv6-zone-id-rejected", "http://[fe80::1%25en0]:80/"),
    S("ipv6-bad-char", "http://[::g]/"),
    S("pct-path-unicode", "https://e.com/ü?ö=ä#ß"),
    S("pct-keeps-percent-and-reserved", "https://e.com/%2F%zz/[]!$&'()*+,;=:@~"),
    S("pct-encodes-quote-angle-backtick", "https://e.com/\"<>`{}|^"),
    S("mailto", "mailto:a@B.com"),
    S("mailto-query-host", "mailto:a@b.com?cc=c@d.com"),
    S("mailto-no-at", "mailto:nobody"),
    S("mailto-empty", "mailto:"),
    S("mailto-slash", "mailto:/x"),
    S("anchor", "#Intro"),
    S("anchor-decoded-lowercased", "#%C3%9Cber-Uns"),
    S("anchor-invalid-utf8", "#%C3"),
    S("anchor-slash", "#/x"),
    S("anchor-dot", "#a.b"),
    S("anchor-empty", "#"),
    S("anchor-field-refused", "#intro", "field"),
    S("at-profile", f"at://{DID}"),
    S("at-record", f"at://{DID}/{NS}.post/3k2"),
    S("at-record-field", f"at://{DID}/{NS}.post/3k2", "field"),
    S("at-collection-only", f"at://{DID}/{NS}.post"),
    S("at-trailing-slash", f"at://{DID}/"),
    S("at-four-segments", f"at://{DID}/{NS}.post/a/b"),
    S("at-query", f"at://{DID}?x"),
    S("at-did-web", f"at://{DIDWEB}"),
    S("at-did-web-uppercase", "at://did:web:Example.com"),
    S("at-did-web-single-label", "at://did:web:localhost"),
    S("at-did-key", "at://did:key:zQ3sh"),
    S("at-handle-markdown", "at://alice.bsky.social"),
    S("at-handle-field", "at://Alice.Bsky.Social", "field"),
    S("at-handle-field-with-path", f"at://alice.bsky.social/{NS}.post/x", "field"),
    S("at-handle-invalid", "at://-alice.com", "field"),
    S("at-no-slashes", f"at:{DID}"),
    S("at-uppercase-scheme", f"AT://{DID}"),
    S("unknown-scheme", "gopher://e.com"),
])

# ------------------------------------------------------------------ term-sanitize.json
def TS(name, text, ctx, mx=None):
    inp = {"text": text, "ctx": ctx}
    if mx is not None:
        inp["max"] = mx
    return V(name, inp, "out")
F("lfm1/term-sanitize.json", "term_sanitize", [
    TS("prose-plain", "hello", "prose"),
    TS("prose-tab-lf", "a\tb\nc", "prose"),
    TS("prose-cr", "a\rb", "prose"),
    TS("prose-nel", "a\u0085b", "prose"),
    TS("prose-ls-ps", "a\u2028b\u2029c", "prose"),
    TS("prose-other-ws-kept", "a\u00a0b\u3000c\u2003d", "prose"),
    TS("prose-esc", "\u001b[31mred\u001b[0m", "prose"),
    TS("prose-bel-nul", "\u0007\u0000", "prose"),
    TS("prose-del", "a\u007fb", "prose"),
    TS("prose-c1", "\u009b2J\u0090", "prose"),
    TS("prose-bidi-removed", "a\u202eb\u2066c\u061cd", "prose"),
    TS("prose-zero-width-kept", "a\u200bb\ufeffc", "prose"),
    TS("prose-badges-kept", "ok \u2705 \u2713", "prose"),
    TS("code-tab-stops", "a\tb\tcd\te", "code"),
    TS("code-tab-after-wide", "日\tx", "code"),
    TS("code-bidi-visible", "a\u202eb\u2069", "code"),
    TS("code-zero-width-visible", "a\u200bb\u200cc\u200dd\u2060e\ufefff", "code"),
    TS("code-cr-nel-ls", "a\rb\u0085c\u2028d", "code"),
    TS("name-collapse-trim", "  a \t b\u3000\u00a0c  ", "name"),
    TS("name-removes-invisibles-and-badges", "al\u200bice\u202e \u2705\u26a0\u2611 x", "name"),
    TS("name-controls-escaped", "a\u001bb\u0085c", "name"),
    TS("name-grapheme-cap", "abcdefgh", "name", 5),
    TS("name-grapheme-cap-exact", "abcde", "name", 5),
    TS("name-grapheme-cap-clusters", "e\u0301e\u0301e\u0301e\u0301", "name", 3),
])

# ------------------------------------------------------------------ styled.json
ST = ("styled",)
F("lfm1/styled.json", "render", [
    R("paragraph-breaks", "a\nb  \nc\n", *ST),
    R("heading", "## Head *x*\n", *ST),
    R("blockquote-and-alert", "> q\n\n> [!WARNING]\n> w\n", *ST),
    R("spoiler-and-details", "> [!spoiler]\n> s\n\n> [!details] Sum\n> d\n\n> [!details]\n> e\n", *ST),
    R("lists", "1. a\n2. b\n   - c\n\n5. five\n", *ST),
    R("task-list", "- [x] done\n- [ ] todo\n", *ST),
    R("code-block", "```py\nx = 1\n\ty\n```\n", *ST),
    R("inline-code-and-math", "a `b\u200b` $c$\n", *ST),
    R("display-math", "x\n$$\na\nb\n$$\ny\n", *ST),
    R("styles-combined", "***~~all~~***\n", *ST),
    R("links", f"[e](https://e.com) [r](at://{DID}/{NS}.post/x) [p](at://{DID}) [@m](at://{DID}) [a](#x) [d](/rel)\n", *ST),
    R("footnote-ref-and-def", "x[^1]\n\n[^1]: note\n", *ST),
    R("images-inline", f"a ![i](cid:{CID1}) ![m](cid:{CID2}) ![e](https://img.example/e.png) ![o](ftp://x)\n", *ST, media=[media(CID1, alt="ma")]),
    R("table", "| a | b |\n|:-|-:|\n| *1* | 2 |\n", *ST),
    R("rule", "a\n\n---\n\nb\n", *ST),
    R("figure-media-caption", f"![a](cid:{CID1} \"Cap\")\n", *ST, media=[media(CID1)]),
    R("plain-paragraph", "x\n", *ST),
    R("raw-html-and-escapes", "<b>\u001b[31m</b> \u202e\n", *ST),
])

# ------------------------------------------------------------------ xss
X = ("html",)
F("xss/html.json", "render", desc="XSS corpus: classic HTML injection, dangerous schemes (entity-, percent- and homoglyph-encoded), attribute breakouts, backslash and whitespace tricks, markdown inside raw HTML. The allowlist checker must accept every output.", vectors=[
    R("script-tag", "<script>alert(1)</script>", *X),
    R("script-inline", "a <script>alert(1)</script> b", *X),
    R("img-onerror", "<img src=x onerror=alert(1)>", *X),
    R("svg-onload", "<svg onload=alert(1)>", *X),
    R("iframe", "<iframe src=\"javascript:alert(1)\"></iframe>", *X),
    R("style-tag", "<style>*{background:url(javascript:alert(1))}</style>", *X),
    R("object-embed", "<object data=x></object><embed src=x>", *X),
    R("math-raw-html", "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>", *X),
    R("svg-foreignobject", "<svg><foreignObject><iframe srcdoc=x></iframe></foreignObject></svg>", *X),
    R("javascript-link", "[x](javascript:alert(1))", *X),
    R("javascript-entity-encoded", "[x](&#106;avascript:alert(1)) [y](&#x6A;avascript:alert(1)) [z](javascript&colon;alert(1))", *X),
    R("javascript-percent-encoded", "[x](javascript%3Aalert(1)) [y](%6Aavascript:alert(1))", *X),
    R("javascript-whitespace-tricks", "[a](<java\tscript:alert(1)>) [b](<java\nscript:x>) [c]( javascript:alert(1)) [d](<\u0001javascript:x>)", *X),
    R("javascript-backslash", "[a](<\\javascript:alert(1)>) [b](<https:\\\\evil.com>) [c](https:/\\evil.com)", *X),
    R("data-and-vbscript", "[a](data:text/html,<script>alert(1)</script>) [b](vbscript:msgbox(1)) [c](DATA:x)", *X),
    R("unicode-homoglyph-scheme", "[a](ϳavascript:alert(1)) [b](јavascript:alert(1)) [c](javascrіpt:alert(1))", *X),
    R("fullwidth-scheme", "[a](ｊａｖａｓｃｒｉｐｔ:alert(1))", *X),
    R("attribute-breakout-alt", "![\" onerror=\"alert(1)](cid:" + CID1 + ")", "html", media=[media(CID1)]),
    R("attribute-breakout-media-alt", f"![](cid:{CID1})", "html", media=[media(CID1, alt="\"><script>alert(1)</script>")]),
    R("attribute-breakout-media-name", f"![](cid:{CID1})", "html", media=[media(CID1, "video/mp4", name="\"><img src=x onerror=alert(1)>")]),
    R("attribute-breakout-href", "[x](https://e.com/\"onmouseover=\"alert(1))", *X),
    R("attribute-breakout-title", "[x](https://e.com \"\\\" onclick=alert(1)\")", *X),
    R("heading-id-injection", "# \"><script>alert(1)</script>", *X),
    R("anchor-injection", "[x](#\"><script>) [y](#x\"onclick=\"y)", *X),
    R("code-lang-injection", "```\"><script>alert(1)</script>\nx\n```", *X),
    R("nested-markdown-in-html", "<div>\n\n*md* [x](javascript:alert(1))\n\n</div>", *X),
    R("html-in-link-text", "[<img src=x onerror=alert(1)>](https://e.com)", *X),
    R("autolink-javascript", "<javascript:alert(1)>", *X),
    R("autolink-data", "<data:text/html,x>", *X),
    R("ext-autolink-quote-breakout", "www.e.com/\"onmouseover=\"x", *X),
    R("cdata-and-comment-tricks", "<!--><script>alert(1)</script>--> <![CDATA[<script>]]>", *X),
    R("noscript-template", "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript><template><script>x</script></template>", *X),
    R("textarea-title-rcdata", "<title><img src=x onerror=alert(1)></title><textarea><img src=x></textarea>", *X),
    R("base-meta-link", "<base href=//evil.com><meta http-equiv=refresh content=0;url=javascript:x><link rel=import href=x>", *X),
    R("form-input", "<form action=javascript:x><input type=submit formaction=javascript:y></form>", *X),
    R("mention-text-spoof", "[@paypal.com](at://" + DID + ")", *X),
    R("footnote-label-injection", "x[^\"><script>]\n\n[^\"><script>]: y", *X),
    R("table-cell-injection", "| <script>x</script> |\n|---|\n| <img src=x onerror=y> |", *X),
    R("spoiler-summary-injection", "> [!details] <img src=x onerror=alert(1)>\n> body", *X),
    R("figcaption-injection", "![a](https://x.example/i.png \"<script>alert(1)</script>\")", *X),
    R("ext-image-javascript", "![a](javascript:alert(1)) ![b](https://x.example/\"><svg onload=alert(1)>)", *X),
    R("nul-in-tags", "<scr\u0000ipt>alert(1)</scr\u0000ipt>", *X),
])
F("xss/mathml.json", "render", desc="MathML XSS/mXSS corpus: disabled macros, href/xlink/actiontype, annotation-xml and mglyph namespace confusion, text-mode HTML.", vectors=[
    R("math-href", "$\\href{javascript:alert(1)}{x}$", "html", "flags"),
    R("math-url", "$\\url{javascript:alert(1)}$", "html", "flags"),
    R("math-html-class-data", "$\\htmlData{onclick=alert(1)}{x}$ $\\htmlStyle{color:red}{x}$", "html", "flags"),
    R("math-text-html-injection", "$\\text{<img src=x onerror=alert(1)>}$", "html", "flags"),
    R("math-text-lt-gt", "$\\text{a < b > c}$", "html", "flags"),
    R("math-mtext-tag-chars", "$a<b>c$", "html", "flags"),
    R("math-raw-mathml-literal", "<math><maction actiontype=\"statusline\" xlink:href=\"javascript:alert(1)\">x</maction></math>", "html"),
    R("math-raw-annotation-xml", "<math><annotation-xml encoding=\"text/html\"><img src=x onerror=alert(1)></annotation-xml></math>", "html"),
    R("math-raw-mglyph", "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)></style></mglyph></table></mtext></math>", "html"),
    R("math-color-style-stripped", "$\\color{red}{x} \\textcolor{blue}{y}$", "html", "flags"),
    R("math-operatorname-entities", "$\\operatorname{evil}\\,x$", "html", "flags"),
    R("math-def-macro", "$\\def\\x{\\href{javascript:alert(1)}{y}}\\x$", "html", "flags"),
    R("math-newcommand", "$\\newcommand{\\x}{\\url{a}}\\x$", "html", "flags"),
    R("math-display-close-tag", "$$</math><script>alert(1)</script>$$", "html", "flags"),
    R("math-dollar-in-code", "`$x$` $y$", "html", "flags"),
    R("math-label-ref", "$$\\begin{equation}x\\label{a\"onclick=\"b}\\end{equation}$$", "html", "flags"),
])

# ------------------------------------------------------------------ terminal
def T(name, text, opts=None, *fields, media=None):
    inp = {"text": text}
    if opts is not None:
        inp["print"] = opts
    if media is not None:
        inp["media"] = media
    return V(name, inp, *(fields or ("printed",)))
COLOR = {"color": True, "hyperlinks": True}
NEVER = {"color": False, "hyperlinks": False}
F("terminal/escapes.json", "terminal", [
    T("csi-in-text", "a \u001b[31mred\u001b[0m b", NEVER, "styled", "printed"),
    T("osc-title-in-text", "\u001b]0;pwned\u0007 x", NEVER, "styled", "printed"),
    T("osc8-in-text", "\u001b]8;;https://evil.com\u001b\\click\u001b]8;;\u001b\\", NEVER, "styled", "printed"),
    T("c1-csi", "a\u009b31mb \u009d0;x\u009c", NEVER, "styled", "printed"),
    T("bell-backspace", "a\u0007\u0008b", NEVER, "styled", "printed"),
    T("bidi-in-prose", "user\u202etxt.exe", NEVER, "styled", "printed"),
    T("bidi-in-code", "`a\u202eb` and\n\n```\nif x\u2067 then\u2069\n```", NEVER, "styled", "printed"),
    T("cr-overwrite", "safe\rEVIL", NEVER, "styled", "printed"),
    T("escape-in-link-text", "[\u001b[2Jx](https://e.com)", COLOR, "printed"),
    T("escape-in-code-block", "```\n\u001b[2J\u001b[H\n```", COLOR, "printed"),
    T("escape-in-heading", "# \u001b]2;title\u0007H", COLOR, "printed"),
    T("escape-in-alt-and-caption", "![a\u001bb](https://x.example/i.png \"\u001bcap\u009b\")", COLOR, "printed"),
    T("escape-in-link-text-inline", "[a\u001b]0;t\u0007b](https://e.com)", COLOR, "printed"),
    T("bidi-in-media-name", f"![](cid:{CID1})", NEVER, "printed", media=[media(CID1, "application/pdf", name="cv\u202efdp.exe")]),
    T("hidden-spoiler-then-paragraph", "> [!spoiler]\n> s\n\nnext\n\n> [!details]\n> d", NEVER, "printed"),
    T("nested-list-and-quote-gutters", "> - a\n>   - b\n> 1. c", NEVER, "printed"),
    T("table-and-code", "| a | bb |\n|---|---|\n| ccc | d |\n\n```\n\tx\n```", NEVER, "printed"),
    T("escape-in-media-name", f"![](cid:{CID1})", NEVER, "printed", media=[media(CID1, "application/pdf", name="a\u001b[31m.pdf")]),
    T("escape-in-math", "$\\text{\u001b[31m}$ $$\u001b[2J$$", NEVER, "printed"),
    T("escape-in-table", "| \u001b[31m |\n|---|\n| \u009b |", NEVER, "printed"),
    T("escape-in-footnote", "x[^1]\n\n[^1]: \u001b[31m", NEVER, "printed"),
    T("ls-ps-in-text", "a\u2028b\u2029c", NEVER, "printed"),
    T("osc8-link-color", "[docs](https://example.com/a;b c)", COLOR, "printed"),
    T("osc8-only-external", f"[r](at://{DID}/{NS}.post/x) [a](#x) [@m](at://{DID})", COLOR, "printed"),
    T("osc8-too-long", "[x](https://e.com/" + "a" * 2100 + ")", COLOR, "printed"),
    T("real-host-shown", "[google.com](https://evil.com) [https://www.e.com/](https://e.com)", NEVER, "printed"),
    T("color-never-no-escapes", "**b** *i* `c` [l](https://e.com) \u001b[1m", NEVER, "printed"),
    T("spoiler-hidden", "> [!spoiler]\n> secret\n\nafter", NEVER, "printed"),
    T("spoiler-revealed", "> [!spoiler]\n> secret\n\nafter", {"reveal_spoilers": True}, "printed"),
    T("details-expanded", "> [!details] Sum\n> body", NEVER, "printed"),
    T("wrap-80", "word " * 40, {"width": 80}, "printed"),
    T("wrap-long-word", "x" * 100, {"width": 40}, "printed"),
    T("ext-image-placeholder-no-double-host", "![cat](https://img.example/c.png)", NEVER, "printed"),
    T("ext-image-in-link-shows-link-host", "[![cat](https://img.example/c.png)](https://evil.com) and [![](https://img.example/d.png)](https://img.example/d.png)", NEVER, "styled", "printed"),
    T("escaped-run-reverse-video", "a\u0001b", {"color": True}, "printed"),
    T("mention-placeholder", f"hi [@alice](at://{DID})", NEVER, "printed"),
])

# ------------------------------------------------------------------ mathml (engine snapshots)
def MM(name, tex, display=False):
    return V(name, {"tex": tex, "display": display}, "html", "math_errors")
F("mathml/math-core-0.9.json", "mathml", [
    MM("frac", "\\frac{a}{b}"),
    MM("sqrt-root", "\\sqrt{x} + \\sqrt[3]{y}"),
    MM("scripts", "x_i^2 + \\sum_{k=0}^{n} k"),
    MM("integral-display", "\\int_0^\\infty e^{-x}\\,dx", True),
    MM("greek-and-symbols", "\\alpha\\beta\\Gamma \\le \\ne \\infty"),
    MM("matrix", "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", True),
    MM("cases", "f(x)=\\begin{cases} 0 & x<0 \\\\ 1 & x \\ge 0 \\end{cases}", True),
    MM("aligned", "\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}", True),
    MM("accents", "\\hat{x} \\bar{y} \\vec{v} \\dot{a} \\tilde{n}"),
    MM("operators", "\\sin x \\log y \\lim_{n\\to\\infty} a_n \\operatorname{rank} A"),
    MM("delimiters", "\\left( \\frac{1}{2} \\right] \\big( x \\Big)"),
    MM("fonts", "\\mathbb{R} \\mathcal{F} \\mathfrak{g} \\mathbf{v} \\mathrm{d}"),
    MM("text", "\\text{if } x > 0"),
    MM("binom", "\\binom{n}{k}"),
    MM("overbrace", "\\overbrace{a+b}^{n}"),
    MM("cancel-enclose", "\\cancel{x} \\boxed{y}"),
    MM("phantom", "\\phantom{x} a"),
    MM("primes", "f'(x) + g''(y)"),
    MM("spacing", "a\\,b\\;c\\quad d\\qquad e\\!f"),
    MM("colour-dropped", "\\color{red} x"),
])


def merge(path, new):
    full = os.path.join(ROOT, path)
    old = {}
    if os.path.exists(full):
        with open(full) as f:
            for v in json.load(f)["vectors"]:
                old[v["name"]] = v
    for v in new["vectors"]:
        o = old.get(v["name"])
        if o and o["input"] == v["input"]:
            for k in v["expected"]:
                if k in o["expected"]:
                    v["expected"][k] = o["expected"][k]
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(json.dumps(new, indent=2, ensure_ascii=True) + "\n")


for p, doc in FILES.items():
    merge(p, doc)
total = sum(len(d["vectors"]) for p, d in FILES.items() if p.startswith("lfm1/"))
print("lfm1 vectors:", total)
for p, d in FILES.items():
    print(f"  {p}: {len(d['vectors'])}")
