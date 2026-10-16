// SPDX-License-Identifier: AGPL-3.0-or-later
// Hand-written LFM/1 HTML for the two showcase posts of the mock fixture. Each string follows
// the HTML output contract exactly (lfm-1.md §8.1 construct mapping, §8.2 allowlist, §8.3
// serialisation: attribute order, escaping, newlines), so `DOMPurify(html, LFM_CONFIG) === html`.

import type { Did } from "../types.ts";
import { escapeAttr, shortDid } from "./util.ts";

const REL = "nofollow ugc noopener noreferrer";

export interface WelcomeRefs {
  mention: Did;
  /** at-uri of the moderation-policy post (an internal record link). */
  policyUri: string;
  /** at-uri of the FDT link post (a second record link, used by hover previews). */
  linkPostUri: string;
  figureCid: string;
  galleryCid: string;
}

function recordLink(uri: string, text: string): string {
  const u = escapeAttr(uri);
  return `<a href="#/at/${escapeAttr(uri.slice(5))}" data-at-uri="${u}" class="at-link">${text}</a>`;
}

function mention(did: Did): string {
  const d = escapeAttr(did);
  return `<a href="#/u/${d}" data-did="${d}" class="mention">@${shortDid(did)}</a>`;
}

export function welcomeHtml(r: WelcomeRefs): string {
  return `<div class="lfm" dir="auto">
<p dir="auto">Welcome to <strong>LessDugin</strong>, a forum where <em>your own PDS is the database</em>. This post is a tour of what LFM/1 markdown renders to.<sup id="md-fnref-1-1"><a data-anchor="md-fn-1">1</a></sup></p>
<h2 id="md-h-how-it-works" dir="auto">How it works</h2>
<p dir="auto">Every post, comment and vote is a record in its author's repository, and every client folds the same records into the same view.<sup id="md-fnref-2-1"><a data-anchor="md-fn-2">2</a></sup> Read ${recordLink(r.policyUri, "the moderation policy")} before posting, skim ${recordLink(r.linkPostUri, "the decision theory paper")}, and ask ${mention(r.mention)} if anything is unclear.</p>
<div class="alert-note">
<p dir="auto">Votes, reacts, memberships and moderation actions are <strong>public records</strong>. Drafts, read marks and bookmarks stay on your device.</p>
</div>
<div class="alert-warning">
<p dir="auto">This forum is served by the in-memory mock engine. Nothing you write leaves this tab.</p>
</div>
<h2 id="md-h-the-layers-of-the-fold" dir="auto">The layers of the fold</h2>
<table>
<thead>
<tr>
<th class="ta-l" dir="auto">Layer</th>
<th dir="auto">Computes</th>
<th class="ta-r" dir="auto">Reads</th>
</tr>
</thead>
<tbody>
<tr>
<td class="ta-l" dir="auto">L0</td>
<td dir="auto">authorities and ranks</td>
<td class="ta-r" dir="auto">the declaration</td>
</tr>
<tr>
<td class="ta-l" dir="auto">L3</td>
<td dir="auto">opinion validity and karma</td>
<td class="ta-r" dir="auto">L0–L2</td>
</tr>
<tr>
<td class="ta-l" dir="auto">L6</td>
<td dir="auto">visibility and ranking</td>
<td class="ta-r" dir="auto">L0–L5</td>
</tr>
</tbody>
</table>
<h2 id="md-h-ranking" dir="auto">Ranking</h2>
<p dir="auto">Posts are ranked by <code>magic/1</code>, which divides the score by a power of the age:</p>
<p dir="auto"><math display="block"><mrow><mi>hot</mi><mo>=</mo><mfrac><mi>b</mi><msup><mrow><mo stretchy="false">(</mo><mi>h</mi><mo>+</mo><mn>2</mn><mo stretchy="false">)</mo></mrow><mn>1.15</mn></msup></mfrac></mrow></math></p>
<p dir="auto">Inline math works too: <math><msup><mi>e</mi><mrow><mi>i</mi><mi>π</mi></mrow></msup><mo>+</mo><mn>1</mn><mo>=</mo><mn>0</mn></math>, and a disabled macro renders as an error: <code class="math-error">\\href{x}{y}</code>.</p>
<pre><code class="language-rust">fn hot(b: i64, age_hours: f64) -&gt; f64 {
    b as f64 / (age_hours + 2.0).powf(1.15)
}
</code></pre>
<h2 id="md-h-spoilers-and-details" dir="auto">Spoilers and details</h2>
<details class="spoiler">
<summary dir="auto">Spoiler</summary>
<p dir="auto">The fold is deterministic: every client computes byte-identical scores.</p>
</details>
<details>
<summary dir="auto">Why is raw HTML not allowed?</summary>
<p dir="auto">Because rendered HTML must survive <code>DOMPurify(html) === html</code> with a fixed allowlist.</p>
</details>
<blockquote>
<p dir="auto">The map is not the territory.</p>
</blockquote>
<h3 id="md-h-checklist" dir="auto">Checklist</h3>
<ul>
<li dir="auto"><input type="checkbox" checked="" disabled=""> Read the specification</li>
<li dir="auto"><input type="checkbox" disabled=""> Write your first post</li>
</ul>
<ol start="3">
<li dir="auto">Numbered lists can start anywhere.</li>
<li dir="auto">Like this one.</li>
</ol>
<p dir="auto">External links show where they go: <a href="https://atproto.com/" rel="${REL}">the AT Protocol</a>, and a link whose text looks like a different host shows the real one: <a href="https://example.com/login" rel="${REL}">bank.example<span class="link-host">example.com</span></a>. Images from other sites are never fetched: <a href="https://example.com/cat.png" class="ext-image" rel="${REL}">image: a cat</a>. <del>Struck-through text</del> is fine too.</p>
<figure>
<img data-cid="${escapeAttr(r.figureCid)}" alt="Diagram of the fold layers" width="16" height="9">
<figcaption dir="auto">The fold, layer by layer</figcaption>
</figure>
<hr>
<p dir="auto">Happy posting!</p>
<section class="footnotes">
<ol>
<li id="md-fn-1" dir="auto"><p dir="auto">Footnotes become sidenotes on wide screens.</p>
<a data-anchor="md-fnref-1-1" class="fn-back">↩</a></li>
<li id="md-fn-2" dir="auto"><p dir="auto">The fold is specified in <code>docs/spec/social.md</code>.</p>
<a data-anchor="md-fnref-2-1" class="fn-back">↩</a></li>
</ol>
</section>
<div class="gallery">
<span class="media" data-cid="${escapeAttr(r.galleryCid)}">talk.mp4</span>
</div>
</div>
`;
}

export const WELCOME_MARKDOWN = `Welcome to **LessDugin**, a forum where *your own PDS is the database*. This post is a tour of what LFM/1 markdown renders to.[^1]

## How it works

Every post, comment and vote is a record in its author's repository, and every client folds the same records into the same view.[^2] Read [the moderation policy](at://…) before posting, skim [the decision theory paper](at://…), and ask [@bob](at://…) if anything is unclear.

> [!NOTE]
> Votes, reacts, memberships and moderation actions are **public records**. Drafts, read marks and bookmarks stay on your device.

…

[^1]: Footnotes become sidenotes on wide screens.
[^2]: The fold is specified in \`docs/spec/social.md\`.
`;

export interface ConfusionRefs {
  mention: Did;
}

export function confusionHtml(r: ConfusionRefs): string {
  return `<div class="lfm" dir="auto">
<p dir="auto">Your strength as a reasoner is to be more confused by fiction than by reality.<sup id="md-fnref-1-1"><a data-anchor="md-fn-1">1</a></sup> This post is about the small, quiet feeling of confusion, and why it deserves more attention than it usually gets.</p>
<h2 id="md-h-the-feeling" dir="auto">The feeling</h2>
<p dir="auto">Confusion rarely announces itself. It shows up as a faint itch: a story that does not quite add up, a number that is a little too round, a friend whose explanation leaves you nodding without understanding. Most of the time we smooth it over, because smoothing it over is cheap and noticing it is not.</p>
<p dir="auto">But that itch is information. It is your model telling you that the world just did something the model did not predict. Either the model is wrong, or the story is.</p>
<h2 id="md-h-three-habits" dir="auto">Three habits</h2>
<h3 id="md-h-1-say-it-out-loud" dir="auto">1. Say it out loud</h3>
<p dir="auto">When something feels off, say “I notice I am confused” before you say anything else. Naming the feeling makes it much harder to explain away.<sup id="md-fnref-2-1"><a data-anchor="md-fn-2">2</a></sup></p>
<h3 id="md-h-2-write-it-down" dir="auto">2. Write it down</h3>
<p dir="auto">Keep a short log of the moments you were confused and what resolved it. After a month, look for patterns: the same kinds of surprises tend to come back.</p>
<h3 id="md-h-3-look-for-the-model" dir="auto">3. Look for the model</h3>
<p dir="auto">Ask which belief made the observation surprising. Then ask how confident you were in that belief, and whether the surprise should move you.</p>
<blockquote>
<p dir="auto">If you are equally good at explaining any outcome, you have zero knowledge.</p>
</blockquote>
<h2 id="md-h-acknowledgements" dir="auto">Acknowledgements</h2>
<p dir="auto">Thanks to ${mention(r.mention)} for comments on a draft.</p>
<section class="footnotes">
<ol>
<li id="md-fn-1" dir="auto"><p dir="auto">A paraphrase of an old LessWrong essay title.</p>
<a data-anchor="md-fnref-1-1" class="fn-back">↩</a></li>
<li id="md-fn-2" dir="auto"><p dir="auto">This is the single most useful habit in this post.</p>
<a data-anchor="md-fnref-2-1" class="fn-back">↩</a></li>
</ol>
</section>
</div>
`;
}

export const CONFUSION_MARKDOWN = `Your strength as a reasoner is to be more confused by fiction than by reality.[^1] …

## The feeling

…

## Three habits

### 1. Say it out loud
### 2. Write it down
### 3. Look for the model

## Acknowledgements

Thanks to [@you](at://…) for comments on a draft.
`;
