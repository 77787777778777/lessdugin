// SPDX-License-Identifier: AGPL-3.0-or-later
// The mock engine's fixture: one forum ("LessDugin Test Forum") with posts of every kind,
// threaded comments in every display state, reacts, tags, a sequence, notifications, a mod
// queue and log, and drafts. Fully deterministic: fixed times, hash-derived identifiers.

import type {
  ActorView,
  Anchor,
  AtUri,
  Badges,
  Did,
  ForumRole,
  HandleState,
  ItemState,
  KarmaView,
  PostKind,
  TextQuote,
  Tier,
  VersionRef,
} from "../types.ts";
import { CONFUSION_MARKDOWN, confusionHtml, WELCOME_MARKDOWN, welcomeHtml } from "./bodies.ts";
import type {
  ActorRec,
  BaseReact,
  BaseScore,
  CommentRec,
  ForumRec,
  MockState,
  ModLogEntryRec,
  NotificationRec,
  PolicyRec,
  PostRec,
  SequenceRec,
  TagRec,
} from "./model.ts";
import { renderMarkdown } from "./render.ts";
import { safeHref } from "./safe-href.ts";
import {
  atUri,
  b32hash,
  bodyFromHtml,
  COLL,
  didFor,
  fakeBlobCid,
  fakeCid,
  shortDid,
  tid,
} from "./util.ts";

/** The fixture's "now": 2026-10-16T09:00:00Z. */
export const FIXTURE_NOW = Date.UTC(2026, 9, 16, 9, 0, 0);
const M = 60_000;
const H = 60 * M;
const D = 24 * H;

export const OPEN_POLICY: PolicyRec = {
  moderation: "mod/1",
  karma: { algorithm: "karma/1", pairCap: 25, establish: 1, establishSeeds: 2 },
  ranking: {
    algorithm: "magic/1",
    offsetHours: 2,
    exponentMilli: 1150,
    curatedBonus: 10,
    frontpageBonus: 0,
    cutoffDays: 90,
  },
  voting: "reacts",
  reactPalette: "default/1",
  gates: {
    reactSecond: 5,
    reactCreate: 10,
    reactAnti: 20,
    authorModPersonal: 50,
    authorModFrontpage: 2000,
    createTag: 10,
    wikiEdit: 5,
    sequence: 1000,
  },
  display: { collapseBelow: -5, newUsers: "collapse" },
  authorModeration: true,
  tagCreation: "open",
};

interface ActorSpec {
  key: string;
  did?: Did;
  displayName: string;
  handle?: string;
  handleState: HandleState;
  status: ForumRole;
  karma: KarmaView;
  bio?: string;
  links?: { uri: string; label?: string }[];
  joinedDaysAgo: number;
  note?: string;
}

const ACTOR_SPECS: ActorSpec[] = [
  {
    key: "ada",
    displayName: "Ada Lovelace",
    handle: "ada.test",
    handleState: "verified",
    status: "owner",
    karma: { k1: 2410, kp: 2380, k: 2395 },
    bio: "Owner of the test forum. I like **deterministic folds** and long walks through Merkle trees.",
    links: [
      { uri: "https://example.com/ada", label: "Homepage" },
      { uri: "javascript:alert(1)", label: "Not a link" },
    ],
    joinedDaysAgo: 120,
  },
  {
    key: "bob",
    displayName: "Bob Moderator",
    handle: "bob.test",
    handleState: "verified",
    status: "moderator",
    karma: { k1: 990, kp: 975, k: 980 },
    bio: "Moderator. Ask me about the moderation policy.",
    joinedDaysAgo: 110,
  },
  {
    key: "carol",
    displayName: "Carol Bayes",
    handle: "carol.test",
    handleState: "verified",
    status: "established",
    karma: { k1: 1530, kp: 1515, k: 1520 },
    bio: "Updating incrementally.",
    links: [{ uri: "https://example.org/carol", label: "Blog" }],
    joinedDaysAgo: 90,
  },
  {
    key: "dan",
    displayName: "Dan Newcomer",
    handleState: "unverified",
    status: "newcomer",
    karma: { k1: 1, kp: 0, k: 1 },
    joinedDaysAgo: 1,
    note: "Hi! I found this forum through a friend.",
  },
  {
    key: "eve",
    displayName: "Eve Contrarian",
    handle: "eve.test",
    handleState: "verified",
    status: "member",
    karma: { k1: -10, kp: -12, k: -12 },
    joinedDaysAgo: 40,
  },
  {
    key: "frank",
    displayName: "Frank Spammer",
    handle: "frank.test",
    handleState: "verified",
    status: "member",
    karma: { k1: -40, kp: -40, k: -40 },
    joinedDaysAgo: 12,
  },
  {
    key: "grace",
    // A right-to-left override in a display name: the SPA must isolate it (<bdi>) and strip it.
    displayName: "Grace \u{202e}esrever",
    handleState: "invalid",
    status: "member",
    karma: { k1: 35, kp: 34, k: 35 },
    joinedDaysAgo: 30,
  },
  {
    key: "heidi",
    did: "did:web:heidi.example.com",
    displayName: "Heidi Web",
    handle: "heidi.example.com",
    handleState: "verified",
    status: "established",
    karma: { k1: 412, kp: 405, k: 410 },
    joinedDaysAgo: 60,
  },
  {
    key: "me",
    displayName: "You (mock account)",
    handle: "you.test",
    handleState: "verified",
    status: "admin",
    karma: { k1: 125, kp: 120, k: 123 },
    bio: "The mock engine's signed-in account.",
    joinedDaysAgo: 50,
  },
];

export const ACTOR_DID: Record<string, Did> = Object.fromEntries(
  ACTOR_SPECS.map((a) => [a.key, a.did ?? didFor(a.key)]),
);

function did(key: string): Did {
  const d = ACTOR_DID[key];
  if (!d) throw new Error(`unknown fixture actor ${key}`);
  return d;
}

export function actorView(spec: {
  did: Did;
  displayName?: string;
  handle?: string;
  handleState: HandleState;
}): ActorView {
  const v: ActorView = {
    did: spec.did,
    handleState: spec.handleState,
    shortDid: shortDid(spec.did),
  };
  if (spec.handle !== undefined && spec.handleState === "verified") v.handle = spec.handle;
  if (spec.displayName !== undefined) v.displayName = spec.displayName;
  return v;
}

export const FORUM_SLUG = "lessdugin-test";
export const FORUM_URI: AtUri = atUri(did("ada"), COLL.forum, FORUM_SLUG);
/** The forum key (`fk/1` shape; hash-derived in the mock). */
export const FK = b32hash(`lessdugin-fk/1:${FORUM_URI}`, 12);

const VISIBLE: ItemState = { kind: "visible" };

function score(s: number, agreement = 0, unweighted = 0, earlier = 0, weighted = 0): BaseScore {
  return { score: s, agreement, unweighted, earlier, weighted };
}

/** Code-point anchor of `exact` in P, or an orphan marker. */
function anchorIn(p: string, exact: string): Anchor {
  const i = p.indexOf(exact);
  if (i < 0) return { orphan: true };
  const start = Array.from(p.slice(0, i)).length;
  return { start, end: start + Array.from(exact).length };
}

function quoteIn(p: string, exact: string): TextQuote {
  const i = p.indexOf(exact);
  if (i < 0) return { exact };
  const before = Array.from(p.slice(0, i));
  const after = Array.from(p.slice(i + exact.length));
  return {
    exact,
    prefix: before.slice(-32).join(""),
    suffix: after.slice(0, 32).join(""),
    start: before.length,
  };
}

interface PostSpec {
  key: string;
  author: string;
  kind: PostKind;
  title?: string;
  markdown?: string;
  html?: string;
  tags: string[];
  tier: Tier;
  pinned?: boolean;
  locked?: boolean;
  ageMs: number;
  /** createdAt claimed by the record, when it differs from t_rank. */
  claimedAgeMs?: number;
  score: BaseScore;
  state?: ItemState;
  badges?: Badges;
  link?: { uri: string; title?: string; description?: string };
  labels?: string[];
  revisions?: number;
  redacted?: boolean;
}

interface CommentSpec {
  key: string;
  post: string;
  parent?: string;
  /** A parent uri that does not exist (missing placeholder). */
  missingParent?: boolean;
  author: string;
  markdown: string;
  ageMs: number;
  score: BaseScore;
  state?: ItemState;
  badges?: Badges;
  retracted?: boolean;
  moderatorHat?: boolean;
  version?: VersionRef;
  orphaned?: boolean;
  pinned?: boolean;
  quote?: string;
  reacts?: BaseReact[];
}

export interface Fixture {
  state: MockState;
  postUri: Record<string, AtUri>;
  commentUri: Record<string, AtUri>;
  sequenceUri: AtUri;
}

export function buildFixture(): Fixture {
  const actors = new Map<Did, ActorRec>();
  for (const a of ACTOR_SPECS) {
    const d = did(a.key);
    const rec: ActorRec = {
      view: actorView({
        did: d,
        displayName: a.displayName,
        handleState: a.handleState,
        ...(a.handle !== undefined ? { handle: a.handle } : {}),
      }),
      pds: d.startsWith("did:web:") ? "https://pds.heidi.example.com" : "https://pds.example.test",
      links: a.links ?? [],
      karma: a.karma,
      status: a.status,
      joinedAtMs: FIXTURE_NOW - a.joinedDaysAgo * D,
      granted: a.status !== "newcomer" && a.key !== "frank",
      pinned: [],
    };
    if (a.bio !== undefined) rec.bio = a.bio;
    if (a.note !== undefined) rec.membershipNote = a.note;
    // The fixture's mod log bans Frank for spam (`MemberView.banned`, social.md §4.9).
    if (a.key === "frank") rec.banned = true;
    actors.set(d, rec);
  }

  // ---------------------------------------------------------------------------- posts
  const postUri: Record<string, AtUri> = {};
  const postTid: Record<string, string> = {};
  const postSpecs: PostSpec[] = [
    {
      key: "welcome",
      author: "ada",
      kind: "article",
      title: "Welcome to LessDugin: a tour of the forum",
      tags: ["meta", "rationality"],
      tier: "curated",
      pinned: true,
      ageMs: 3 * D,
      score: score(86, 12, 2),
      badges: { verified: true },
    },
    {
      key: "confusion",
      author: "carol",
      kind: "article",
      title: "Noticing confusion",
      tags: ["rationality", "practical"],
      tier: "frontpage",
      ageMs: 1 * D,
      score: score(53, 9, 1),
      revisions: 1,
    },
    {
      key: "question",
      author: "bob",
      kind: "question",
      title: "What is the best introduction to decision theory?",
      markdown:
        "I'm looking for an accessible introduction to decision theory — something between a blog post and a textbook.\n\nWhat would you recommend, and **why**?\n\n- Should it cover *Newcomb's problem*?\n- Is the math essential?",
      tags: ["decision-theory"],
      tier: "frontpage",
      ageMs: 20 * H,
      score: score(23, 4),
    },
    {
      key: "fdt",
      author: "carol",
      kind: "link",
      title: "Paper: Functional Decision Theory",
      markdown:
        "A classic paper on functional decision theory. Discussion welcome, especially on how it handles `Newcomb`-like problems.",
      tags: ["decision-theory", "ai-alignment"],
      tier: "frontpage",
      ageMs: 2 * D,
      score: score(31, 6),
      link: {
        uri: "https://arxiv.org/abs/1710.05060",
        title: "Functional Decision Theory: A New Theory of Instrumental Rationality",
        description: "Yudkowsky and Soares, 2017.",
      },
    },
    {
      key: "danQuick",
      author: "dan",
      kind: "quickTake",
      markdown: "Just joined. Is it normal that my first comments are collapsed?",
      tags: [],
      tier: "personal",
      ageMs: 5 * H,
      score: score(2),
      state: { kind: "collapsed", reason: "newUser" },
    },
    {
      key: "carolQuick",
      author: "carol",
      kind: "quickTake",
      markdown:
        "Quick take: the cheapest way to test a belief is to ask what you would expect to see if it were false. Then go and look.",
      tags: ["practical"],
      tier: "personal",
      ageMs: 8 * H,
      score: score(9, 3),
    },
    {
      key: "karmaBad",
      author: "eve",
      kind: "article",
      title: "Karma is just a popularity contest",
      markdown:
        "Every voting system rewards conformity. Change my mind.\n\nI think the seed rule only moves the popularity contest one step back.",
      tags: ["meta"],
      tier: "personal",
      ageMs: 4 * D,
      score: score(-7, -11),
      state: { kind: "collapsed", reason: "lowScore" },
    },
    {
      key: "policy",
      author: "bob",
      kind: "article",
      title: "Moderation policy",
      markdown:
        "## What gets removed\n\n- Spam and advertising\n- Personal attacks\n- Doxxing\n\n## How to appeal\n\nReply in the welcome thread or contact a moderator. Every moderation action appears in the mod log.\n\n> [!IMPORTANT]\n> Moderation actions are public records in the moderator's repository.",
      tags: ["meta"],
      tier: "frontpage",
      pinned: true,
      locked: true,
      ageMs: 10 * D,
      score: score(40, 8),
    },
    {
      key: "map1",
      author: "ada",
      kind: "article",
      title: "Map and Territory, part 1: models",
      markdown:
        "## Models\n\nA model is a compressed description of the world that lets you predict it.\n\n## Where models fail\n\nModels fail at their edges. The trick is knowing where the edges are.",
      tags: ["rationality"],
      tier: "curated",
      ageMs: 30 * D,
      score: score(66, 10),
    },
    {
      key: "map2",
      author: "ada",
      kind: "article",
      title: "Map and Territory, part 2: updating",
      markdown:
        "When the territory disagrees with the map, the territory wins.\n\n## Small updates\n\nMost updates are small. That is fine.",
      tags: ["rationality"],
      tier: "frontpage",
      ageMs: 29 * D,
      score: score(48, 7),
    },
    {
      key: "history",
      author: "grace",
      kind: "article",
      title: "On editing history",
      markdown:
        "This post has been edited twice. Earlier versions are listed in the history view, unless the author redacted them.",
      tags: ["meta"],
      tier: "personal",
      ageMs: 6 * D,
      score: score(5, 1, 0, 3, 12),
      badges: { edited: true, identityUnverified: true },
      revisions: 2,
      redacted: true,
    },
    {
      key: "future",
      author: "heidi",
      kind: "article",
      title: "A post from the future",
      markdown:
        "My clock says this was written three days from now. The engine ranks it by when it was first seen instead.",
      tags: ["meta"],
      tier: "frontpage",
      ageMs: 2 * H,
      claimedAgeMs: -3 * D,
      score: score(12, 2),
      badges: { futureDated: true, stale: true },
    },
    {
      key: "long",
      author: "carol",
      kind: "article",
      title: "A very long thread",
      markdown:
        "Reply here to test long threads. The SPA virtualises comment lists past a threshold, and caps indentation at six levels.",
      tags: ["practical"],
      tier: "frontpage",
      ageMs: 12 * H,
      score: score(15, 1),
    },
    {
      key: "spam",
      author: "frank",
      kind: "article",
      title: "Cheap watches",
      markdown: "Buy cheap watches at example.com!",
      tags: [],
      tier: "personal",
      ageMs: 3 * D,
      score: score(-20),
      state: { kind: "hidden", reason: { kind: "banned" } },
    },
    {
      key: "myQuick",
      author: "me",
      kind: "quickTake",
      markdown: "Testing quick takes from the mock account. Everything here lives in memory.",
      tags: [],
      tier: "personal",
      ageMs: 26 * H,
      score: score(3),
    },
  ];

  let clock = 1;
  for (const p of postSpecs) {
    const created = FIXTURE_NOW - p.ageMs;
    const t = tid(created, clock++);
    postTid[p.key] = t;
    postUri[p.key] = atUri(did(p.author), COLL.post, `${FK}-${t}`);
  }

  const figureCid = fakeBlobCid("welcome-figure");
  const galleryCid = fakeBlobCid("welcome-talk");
  const htmlFor: Record<string, string> = {
    welcome: welcomeHtml({
      mention: did("bob"),
      policyUri: postUri.policy ?? "",
      linkPostUri: postUri.fdt ?? "",
      figureCid,
      galleryCid,
    }),
    confusion: confusionHtml({ mention: did("me") }),
  };
  const markdownFor: Record<string, string> = {
    welcome: WELCOME_MARKDOWN,
    confusion: CONFUSION_MARKDOWN,
  };

  const posts = new Map<AtUri, PostRec>();
  for (const p of postSpecs) {
    const uri = postUri[p.key] ?? "";
    const created = FIXTURE_NOW - p.ageMs;
    const html = htmlFor[p.key];
    const markdown = markdownFor[p.key] ?? p.markdown ?? "";
    const body = html !== undefined ? bodyFromHtml(html) : renderMarkdown(markdown).body;
    if (p.key === "welcome") {
      body.media = [
        {
          index: 0,
          cid: figureCid,
          mime: "image/png",
          alt: "Diagram of the fold layers",
          aspectRatio: { width: 1600, height: 900 },
          author: did("ada"),
          pds: "https://pds.example.test",
          autoload: false,
          sizeBytes: 184_211,
        },
        {
          index: 1,
          cid: galleryCid,
          mime: "video/mp4",
          name: "talk.mp4",
          durationMs: 312_000,
          author: did("ada"),
          pds: "https://pds.example.test",
          autoload: false,
          sizeBytes: 22_381_004,
        },
      ];
    }
    const rec: PostRec = {
      uri,
      cid: fakeCid(`${uri}#v${p.revisions ?? 0}`),
      forum: FORUM_URI,
      author: did(p.author),
      kind: p.kind,
      markdown,
      body,
      tags: p.tags,
      tier: p.tier,
      pinned: p.pinned ?? false,
      locked: p.locked ?? false,
      createdAtMs: FIXTURE_NOW - (p.claimedAgeMs ?? p.ageMs),
      tRankMs: created,
      base: p.score,
      reacts: [],
      state: p.state ?? VISIBLE,
      badges: p.badges ?? {},
      labels: p.labels ?? [],
      revisions: [],
      redacted: p.redacted ?? false,
    };
    if (p.title !== undefined) rec.title = p.title;
    if (p.link) {
      const href = safeHref(p.link.uri, "field");
      const link: NonNullable<PostRec["link"]> = {
        uri: p.link.uri,
        host: href?.kind === "external" ? href.host : "",
      };
      if (p.link.title !== undefined) link.title = p.link.title;
      if (p.link.description !== undefined) link.description = p.link.description;
      rec.link = link;
    }
    for (let n = 1; n <= (p.revisions ?? 0); n++) {
      const v = {
        n,
        cid: fakeCid(`${uri}#v${n - 1}`),
        createdAtMs: created + n * H,
        available: !(rec.redacted && n === 1),
      };
      rec.revisions.push(n === 2 ? { ...v, summary: "Fixed a typo" } : v);
    }
    posts.set(uri, rec);
  }

  const welcome = posts.get(postUri.welcome ?? "");
  if (welcome) {
    const p = welcome.body.text;
    welcome.reacts = [
      { react: "agree", for: 7, against: 0, creator: did("carol") },
      {
        react: "insightful",
        for: 3,
        against: 0,
        quote: quoteIn(p, "Every post, comment and vote is a record"),
        anchor: anchorIn(p, "Every post, comment and vote is a record"),
        creator: did("heidi"),
      },
      { react: "heart", for: 3, against: 0, creator: did("bob") },
      {
        react: "typo",
        for: 1,
        against: 0,
        quote: { exact: "recieve" },
        anchor: { orphan: true },
        creator: did("carol"),
      },
      { react: "changed-mind", for: 1, against: 0, creator: did("heidi") },
    ];
  }
  const confusion = posts.get(postUri.confusion ?? "");
  if (confusion) {
    confusion.reacts = [
      { react: "good-point", for: 5, against: 0, creator: did("ada") },
      { react: "examples", for: 2, against: 0, creator: did("bob") },
    ];
  }
  actors.get(did("ada"))?.pinned.push(postUri.welcome ?? "");

  // ---------------------------------------------------------------------------- comments
  const commentSpecs: CommentSpec[] = [
    {
      key: "c01",
      post: "welcome",
      author: "carol",
      markdown:
        "This is a great introduction. I especially like that **votes are public records**: it keeps everyone honest.",
      ageMs: 70 * H,
      score: score(24, 5),
      reacts: [
        { react: "agree", for: 3, against: 0, creator: did("bob") },
        { react: "insightful", for: 2, against: 0, creator: did("ada") },
      ],
    },
    {
      key: "c02",
      post: "welcome",
      parent: "c01",
      author: "bob",
      markdown:
        "Moderator note: please keep replies on topic, and see the [moderation policy](POLICY) if in doubt.",
      ageMs: 66 * H,
      score: score(10, 2),
      moderatorHat: true,
    },
    {
      key: "c03",
      post: "welcome",
      parent: "c02",
      author: "carol",
      markdown: "Will do!",
      ageMs: 30 * H,
      score: score(3),
    },
    {
      key: "c04",
      post: "welcome",
      parent: "c03",
      author: "ada",
      markdown: "Thanks, both of you. (Depth 4.)",
      ageMs: 29 * H,
      score: score(4),
    },
    {
      key: "c05",
      post: "welcome",
      parent: "c04",
      author: "carol",
      markdown: "Depth 5: threads keep nesting.",
      ageMs: 28 * H,
      score: score(2),
    },
    {
      key: "c06",
      post: "welcome",
      parent: "c05",
      author: "bob",
      markdown: "Depth 6: the last indented level.",
      ageMs: 27 * H,
      score: score(2),
    },
    {
      key: "c07",
      post: "welcome",
      parent: "c06",
      author: "carol",
      markdown: "Depth 7: this reply is past the indentation cap, so it continues on its own page.",
      ageMs: 26 * H,
      score: score(1),
    },
    {
      key: "c08",
      post: "welcome",
      parent: "c07",
      author: "ada",
      markdown: "Depth 8: still here.",
      ageMs: 25 * H,
      score: score(1),
    },
    {
      key: "c09",
      post: "welcome",
      author: "dan",
      markdown: "Hi all, first comment here! Is there a guide for newcomers?",
      ageMs: 4 * H,
      score: score(1),
      state: { kind: "collapsed", reason: "newUser" },
    },
    {
      key: "c10",
      post: "welcome",
      parent: "c09",
      author: "ada",
      markdown: "Welcome! Start with the moderation policy, then lurk a little.",
      ageMs: 3 * H,
      score: score(5),
    },
    {
      key: "c11",
      post: "welcome",
      author: "eve",
      markdown: "Karma systems are just popularity contests with extra steps.",
      ageMs: 50 * H,
      score: score(-6, -8),
      state: { kind: "collapsed", reason: "lowScore" },
      reacts: [
        { react: "disagree", for: 4, against: 0, creator: did("carol") },
        { react: "strawman", for: 2, against: 1, creator: did("ada") },
      ],
    },
    {
      key: "c12",
      post: "welcome",
      parent: "c11",
      author: "carol",
      markdown: "I think the seed rule makes that much harder to game.",
      ageMs: 49 * H,
      score: score(8, 3),
    },
    {
      key: "c13",
      post: "welcome",
      author: "frank",
      markdown: "Great post! Visit example.com for cheap watches.",
      ageMs: 60 * H,
      score: score(-3),
      state: {
        kind: "hidden",
        reason: {
          kind: "removed",
          rank: 1,
          actor: did("bob"),
          spam: false,
          reason: "Off-topic advertising",
        },
      },
    },
    {
      key: "c14",
      post: "welcome",
      parent: "c13",
      author: "carol",
      markdown: "(Replying to the removed comment: that link was spam.)",
      ageMs: 59 * H,
      score: score(2),
    },
    {
      key: "c15",
      post: "welcome",
      parent: "c15",
      author: "grace",
      markdown:
        "My parent pointer formed a cycle after an edit, so the thread pass re-attached me to the root.",
      ageMs: 40 * H,
      score: score(1),
      orphaned: true,
    },
    {
      key: "c16",
      post: "welcome",
      missingParent: true,
      author: "heidi",
      markdown: "My parent comment was deleted, so I appear under a placeholder.",
      ageMs: 45 * H,
      score: score(3),
    },
    {
      key: "c17",
      post: "welcome",
      author: "carol",
      markdown: "I claimed earlier that the fold depends on sync order. That was wrong.",
      ageMs: 55 * H,
      score: score(4),
      retracted: true,
    },
    {
      key: "c18",
      post: "welcome",
      author: "bob",
      markdown: "This comment was written on the first revision of the post.",
      ageMs: 65 * H,
      score: score(3),
      version: { revision: 1 },
    },
    {
      key: "c19",
      post: "welcome",
      author: "eve",
      markdown: "Written on a version whose snapshot was later redacted.",
      ageMs: 64 * H,
      score: score(0),
      version: { unavailable: true },
    },
    {
      key: "c20",
      post: "welcome",
      author: "ada",
      markdown:
        "**Start here.** Use the arrows to vote (shift-click or long-press for a strong vote), `j`/`k` to move between comments, and Enter to expand or collapse one.",
      ageMs: 71 * H,
      score: score(6),
      pinned: true,
    },
    {
      key: "c21",
      post: "welcome",
      author: "frank",
      markdown: "Another spam comment.",
      ageMs: 58 * H,
      score: score(-2),
      state: { kind: "hidden", reason: { kind: "banned" } },
    },
    {
      key: "c22",
      post: "welcome",
      parent: "c21",
      author: "bob",
      markdown: "Frank has been banned for spam; replies to his comments stay visible.",
      ageMs: 57 * H,
      score: score(4),
      moderatorHat: true,
    },
    {
      key: "c23",
      post: "welcome",
      author: "me",
      markdown: "My own comment. Try editing or retracting it.",
      ageMs: 47 * H,
      score: score(2),
    },
    {
      key: "c24",
      post: "welcome",
      parent: "c23",
      author: "carol",
      markdown: "Nice to see the mock account working.",
      ageMs: 2 * H,
      score: score(1),
    },
    {
      key: "c25",
      post: "welcome",
      author: "heidi",
      markdown: "Does this include drafts? I assumed they were local.",
      ageMs: 6 * H,
      score: score(2),
      quote: "Every post, comment and vote is a record",
    },
    {
      key: "c26",
      post: "welcome",
      author: "carol",
      markdown: "This quote no longer matches the post.",
      ageMs: 62 * H,
      score: score(1),
      quote: "text that was edited away",
    },
    {
      key: "c27",
      post: "welcome",
      author: "grace",
      markdown: "Hidden by a moderator, with no replies: you should not see this.",
      ageMs: 61 * H,
      score: score(0),
      state: {
        kind: "hidden",
        reason: { kind: "removed", rank: 1, actor: did("bob"), spam: true },
      },
    },
    // confusion
    {
      key: "d01",
      post: "confusion",
      author: "heidi",
      markdown: "The *write it down* habit is underrated. My log is mostly about trains.",
      ageMs: 20 * H,
      score: score(7, 2),
    },
    {
      key: "d02",
      post: "confusion",
      parent: "d01",
      author: "ada",
      markdown: "Same here. Trains are a surprisingly good source of surprise.",
      ageMs: 19 * H,
      score: score(4),
    },
    {
      key: "d03",
      post: "confusion",
      author: "eve",
      markdown: "Isn't confusion just a lack of information?",
      ageMs: 18 * H,
      score: score(1, -2),
    },
    // question
    {
      key: "q01",
      post: "question",
      author: "carol",
      markdown: "Start with [the FDT paper](FDT): it is readable and covers Newcomb's problem.",
      ageMs: 18 * H,
      score: score(9, 4),
    },
    {
      key: "q02",
      post: "question",
      author: "ada",
      markdown:
        "For something gentler, try an introductory chapter on expected utility first. The math is light.",
      ageMs: 16 * H,
      score: score(6, 2),
    },
    {
      key: "q03",
      post: "question",
      author: "dan",
      markdown: "+1, I would like to know too.",
      ageMs: 3 * H,
      score: score(0),
      state: { kind: "collapsed", reason: "newUser" },
    },
    // fdt
    {
      key: "f01",
      post: "fdt",
      author: "bob",
      markdown: "Mod note: link posts need a short summary in the body. This one is fine.",
      ageMs: 40 * H,
      score: score(5),
      moderatorHat: true,
    },
    {
      key: "f02",
      post: "fdt",
      parent: "f01",
      author: "carol",
      markdown: "Thanks, I added one.",
      ageMs: 39 * H,
      score: score(3),
    },
    // policy (locked)
    {
      key: "l01",
      post: "policy",
      author: "bob",
      markdown: "Comments here are locked. Ask questions in the welcome thread.",
      ageMs: 9 * D,
      score: score(6),
      moderatorHat: true,
    },
    // map1
    {
      key: "m01",
      post: "map1",
      author: "carol",
      markdown: "The point about edges is the whole game.",
      ageMs: 28 * D,
      score: score(11, 3),
    },
  ];

  const commentUri: Record<string, AtUri> = {};
  for (const c of commentSpecs) {
    const created = FIXTURE_NOW - c.ageMs;
    const ct = tid(created, clock++);
    commentUri[c.key] = atUri(did(c.author), COLL.comment, `${FK}-${postTid[c.post]}-${ct}`);
  }

  const replaceRefs = (md: string): string =>
    md.replace("](POLICY)", `](${postUri.policy})`).replace("](FDT)", `](${postUri.fdt})`);

  const comments = new Map<AtUri, CommentRec>();
  for (const c of commentSpecs) {
    const uri = commentUri[c.key] ?? "";
    const root = postUri[c.post] ?? "";
    const created = FIXTURE_NOW - c.ageMs;
    const markdown = replaceRefs(c.markdown);
    const rec: CommentRec = {
      uri,
      cid: fakeCid(uri),
      forum: FORUM_URI,
      root,
      author: did(c.author),
      markdown,
      body: renderMarkdown(markdown).body,
      createdAtMs: created,
      tRankMs: created,
      base: c.score,
      reacts: c.reacts ?? [],
      state: c.state ?? VISIBLE,
      badges: c.badges ?? {},
      retracted: c.retracted ?? false,
      moderatorHat: c.moderatorHat ?? false,
      orphaned: c.orphaned ?? false,
      pinned: c.pinned ?? false,
    };
    if (c.parent !== undefined) rec.parent = commentUri[c.parent] ?? "";
    if (c.missingParent) {
      rec.parent = atUri(
        did("heidi"),
        COLL.comment,
        `${FK}-${postTid[c.post]}-${tid(created - H, 999)}`,
      );
    }
    if (c.version) rec.version = c.version;
    if (c.quote !== undefined) {
      const target = posts.get(root)?.body.text ?? "";
      rec.quote = quoteIn(target, c.quote);
      rec.quoteAnchor = anchorIn(target, c.quote);
    }
    if (c.retracted) rec.badges = { ...rec.badges, edited: true };
    comments.set(uri, rec);
  }

  // The long thread: deterministic shape from a linear congruential generator.
  const longRoot = postUri.long ?? "";
  const longStart = FIXTURE_NOW - 12 * H;
  const authors = ["carol", "bob", "ada", "heidi", "eve", "me", "grace"];
  const phrases = [
    "I agree with the general point, but the details matter.",
    "Could you give a concrete example?",
    "This is the crux for me.",
    "Interesting. I had not considered that angle.",
    "I think this is slightly overconfident.",
    "Strong agree. This matches my experience.",
    "Here is a counterexample: consider the case where nobody votes.",
    "Thanks, that clarifies it.",
    "I would put it differently: the map is not the territory.",
    "Seconding this.",
  ];
  let seed = 20261016;
  const next = (): number => {
    seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
    return seed;
  };
  const longComments: { uri: AtUri; depth: number }[] = [];
  for (let i = 0; i < 160; i++) {
    const created = longStart + (i + 1) * 4 * M;
    const author = authors[next() % authors.length] ?? "carol";
    const uri = atUri(did(author), COLL.comment, `${FK}-${postTid.long}-${tid(created, i % 1000)}`);
    let parent: { uri: AtUri; depth: number } | undefined;
    const r = next() % 10;
    if (r >= 3 && longComments.length > 0) {
      const back = next() % Math.min(6, longComments.length);
      const cand = longComments[longComments.length - 1 - back];
      if (cand && cand.depth < 9) parent = cand;
    }
    const markdown = `${phrases[next() % phrases.length]} (#${i + 1})`;
    const rec: CommentRec = {
      uri,
      cid: fakeCid(uri),
      forum: FORUM_URI,
      root: longRoot,
      author: did(author),
      markdown,
      body: renderMarkdown(markdown).body,
      createdAtMs: created,
      tRankMs: created,
      base: score((next() % 15) - 2, (next() % 7) - 3),
      reacts: [],
      state: VISIBLE,
      badges: {},
      retracted: false,
      moderatorHat: false,
      orphaned: false,
      pinned: false,
    };
    if (parent) rec.parent = parent.uri;
    comments.set(uri, rec);
    longComments.push({ uri, depth: parent ? parent.depth + 1 : 1 });
  }

  // ---------------------------------------------------------------------------- forum
  const forum: ForumRec = {
    summary: {
      uri: FORUM_URI,
      owner: did("ada"),
      name: "LessDugin Test Forum",
      description:
        "A sandbox forum for the lessdugin web app, served by the in-memory mock engine.",
      membership: "open",
    },
    about:
      "Welcome to the **LessDugin Test Forum**. Be kind, be curious, and remember that votes are public.\n\n- Stay on topic\n- No spam or advertising\n- Disagree with ideas, not people",
    moderators: [
      { did: did("bob"), role: "moderator", title: "Moderator", shown: true },
      { did: did("me"), role: "admin", title: "Admin", shown: true },
    ],
    policy: OPEN_POLICY,
    banners: [
      {
        code: "relayDependent",
        message:
          "This forum is in open mode: it relies on public relay infrastructure (Bluesky relay, Jetstream, Constellation) to discover newcomers.",
      },
    ],
    featuredTags: ["rationality", "decision-theory", "meta"],
    defaultTier: "frontpage",
    membership: "open",
  };

  // ---------------------------------------------------------------------------- tags
  const tagRecs: TagRec[] = [
    {
      slug: "rationality",
      name: "Rationality",
      description: "The art of thinking clearly and acting well.",
      unmergedEdits: 0,
    },
    {
      slug: "decision-theory",
      name: "Decision theory",
      description: "How agents should choose between actions.",
      page: "Decision theory studies how an agent should choose between actions when the outcome is uncertain.\n\n## Classic problems\n\n- Newcomb's problem\n- The smoking lesion\n- Counterfactual mugging\n\n## Further reading\n\n- [Functional Decision Theory](https://arxiv.org/abs/1710.05060)\n- The *rationality* tag",
      head: atUri(did("carol"), COLL.wikiRevision, `${FK}-${tid(FIXTURE_NOW - 5 * D, 7)}`),
      unmergedEdits: 1,
    },
    { slug: "ai-alignment", name: "AI alignment", unmergedEdits: 0 },
    {
      slug: "meta",
      name: "Site meta",
      description: "Discussion about this forum.",
      unmergedEdits: 0,
    },
    {
      slug: "practical",
      name: "Practical",
      description: "Advice you can use this week.",
      unmergedEdits: 0,
    },
  ];
  const tags = new Map([[FORUM_URI, new Map(tagRecs.map((t) => [t.slug, t]))]]);

  // ---------------------------------------------------------------------------- sequence
  const sequenceUri = atUri(did("ada"), COLL.sequence, `${FK}-${tid(FIXTURE_NOW - 28 * D, 3)}`);
  const sequence: SequenceRec = {
    uri: sequenceUri,
    author: did("ada"),
    title: "Map and Territory",
    body: "A short sequence about **models and reality**, and what to do when they disagree.",
    chapters: [
      { title: "Part I: Models", posts: [postUri.map1 ?? "", postUri.map2 ?? ""] },
      {
        title: "Part II: Practice",
        body: "Applying the ideas day to day.",
        posts: [postUri.confusion ?? ""],
      },
    ],
    base: score(34, 5),
    pinned: true,
    createdAtMs: FIXTURE_NOW - 28 * D,
  };

  // ---------------------------------------------------------------------------- notifications
  const me = did("me");
  const notifications: NotificationRec[] = [
    {
      id: "n01",
      reason: "reply",
      atMs: FIXTURE_NOW - 2 * H,
      subject: commentUri.c24 ?? "",
      actor: did("carol"),
      read: false,
      preview: "Nice to see the mock account working.",
    },
    {
      id: "n02",
      reason: "mention",
      atMs: FIXTURE_NOW - 1 * D,
      subject: postUri.confusion ?? "",
      actor: did("carol"),
      read: false,
      preview: "Thanks to @you.test for comments on a draft.",
    },
    {
      id: "n03",
      reason: "quote",
      atMs: FIXTURE_NOW - 6 * H,
      subject: commentUri.c25 ?? "",
      actor: did("heidi"),
      read: false,
      preview: "Does this include drafts? I assumed they were local.",
    },
    {
      id: "n05",
      reason: "newComment",
      atMs: FIXTURE_NOW - 3 * H,
      subject: commentUri.q03 ?? "",
      actor: did("dan"),
      read: false,
      preview: "+1, I would like to know too.",
    },
    {
      id: "n06",
      reason: "newPost",
      atMs: FIXTURE_NOW - 8 * H,
      subject: postUri.carolQuick ?? "",
      actor: did("carol"),
      read: true,
      preview: "Quick take: the cheapest way to test a belief…",
    },
    {
      id: "n07",
      reason: "reacted",
      atMs: FIXTURE_NOW - 5 * H,
      subject: commentUri.c23 ?? "",
      read: false,
      preview: "3 people reacted 💡 Insightful",
    },
    {
      id: "n08",
      reason: "modAction",
      atMs: FIXTURE_NOW - 26 * H,
      subject: postUri.myQuick ?? "",
      actor: did("bob"),
      read: true,
      preview: "Your quick take was kept by a moderator.",
    },
    {
      id: "n09",
      reason: "karmaChange",
      atMs: FIXTURE_NOW - 9 * H,
      subject: FORUM_URI,
      read: true,
      preview: "+12 karma since yesterday",
    },
  ];

  // ---------------------------------------------------------------------------- moderation
  const modLog: ModLogEntryRec[] = [
    {
      actor: did("bob"),
      action: "hide",
      subject: commentUri.c27 ?? "",
      value: "hidden (spam)",
      atMs: FIXTURE_NOW - 60 * H,
      lapsed: false,
    },
    {
      actor: did("bob"),
      action: "hide",
      subject: commentUri.c13 ?? "",
      value: "hidden",
      reason: "Off-topic advertising",
      atMs: FIXTURE_NOW - 59 * H,
      lapsed: false,
    },
    {
      actor: did("ada"),
      action: "access",
      subject: did("frank"),
      value: "banned",
      reason: "Repeated spam",
      atMs: FIXTURE_NOW - 58 * H,
      lapsed: false,
    },
    {
      actor: did("ada"),
      action: "tier",
      subject: postUri.welcome ?? "",
      value: "curated",
      atMs: FIXTURE_NOW - 70 * H,
      lapsed: false,
    },
    {
      actor: did("ada"),
      action: "pin:forum",
      subject: postUri.welcome ?? "",
      value: "pinned",
      atMs: FIXTURE_NOW - 71 * H,
      lapsed: false,
    },
    {
      actor: did("bob"),
      action: "lock",
      subject: postUri.policy ?? "",
      value: "locked",
      reason: "Policy discussion happens in the welcome thread",
      atMs: FIXTURE_NOW - 9 * D,
      lapsed: false,
    },
    {
      actor: did("grace"),
      action: "pin:forum",
      subject: postUri.map1 ?? "",
      value: "pinned",
      atMs: FIXTURE_NOW - 25 * D,
      lapsed: true,
    },
    {
      actor: did("ada"),
      action: "pin:forum",
      subject: postUri.map1 ?? "",
      value: "unpinned",
      atMs: FIXTURE_NOW - 20 * D,
      lapsed: false,
      adoptedFrom: atUri(did("grace"), COLL.modAction, `${FK}-${b32hash("pin-map1", 16)}`),
    },
  ];
  const modQueue = [
    {
      kind: "staleAssertion" as const,
      subject: postUri.history ?? "",
      reason: "Edited since kept",
      detail:
        "The keep by @bob.test was bound to revision 1; the post has been edited twice since.",
    },
    {
      kind: "unorderedConflict" as const,
      subject: "tag:decision-theory",
      reason: "Two wiki heads",
      detail: "Two revisions of the decision-theory page claim the same parent.",
    },
    {
      kind: "newcomerItem" as const,
      subject: commentUri.c09 ?? "",
      reason: "Newcomer comment",
      detail: "Collapsed until the author is established.",
    },
    {
      kind: "newcomerItem" as const,
      subject: postUri.danQuick ?? "",
      reason: "Newcomer quick take",
      detail: "Collapsed until the author is established.",
    },
  ];

  // ---------------------------------------------------------------------------- local state
  const state: MockState = {
    actors,
    forums: new Map([[FORUM_URI, forum]]),
    posts,
    comments,
    tags,
    sequences: new Map([[sequenceUri, sequence]]),
    opinions: new Map([
      [
        me,
        new Map([
          [postUri.confusion ?? "", { karma: "up", reacts: [] }],
          [commentUri.c01 ?? "", { agreement: "agree", reacts: [] }],
          [postUri.welcome ?? "", { reacts: [{ react: "heart", stance: "for" }] }],
        ]),
      ],
    ]),
    notifications: new Map([[me, notifications]]),
    modLog: new Map([[FORUM_URI, modLog]]),
    modQueue: new Map([[FORUM_URI, modQueue]]),
    accounts: [{ did: me, pds: "https://pds.example.test", auth: "oauth" }],
    active: me,
    followed: new Map([[FORUM_URI, undefined]]),
    lastVisited: new Map([[postUri.welcome ?? "", FIXTURE_NOW - 2 * D]]),
    drafts: new Map([
      [
        "draft-1",
        {
          id: "draft-1",
          savedAtMs: FIXTURE_NOW - 2 * H,
          draft: {
            kind: "post",
            id: "draft-1",
            post: {
              forum: FORUM_URI,
              kind: "article",
              title: "Goodhart's law in karma systems",
              body: {
                markdown:
                  "When a measure becomes a target, it ceases to be a good measure.\n\n## Draft notes\n\n- seeds\n- pair caps",
                media: [],
              },
              tags: ["meta"],
              labels: [],
            },
          },
        },
      ],
      [
        "draft-2",
        {
          id: "draft-2",
          savedAtMs: FIXTURE_NOW - 30 * M,
          draft: {
            kind: "comment",
            id: "draft-2",
            comment: {
              root: postUri.question ?? "",
              body: {
                markdown: "I would also recommend starting with expected utility.",
                media: [],
              },
            },
          },
        },
      ],
    ]),
    bookmarks: new Map([
      [postUri.confusion ?? "", { uri: postUri.confusion ?? "", savedAtMs: FIXTURE_NOW - 1 * D }],
    ]),
    subscriptions: [
      {
        subscription: { kind: "post", uri: postUri.question ?? "" },
        sinceMs: FIXTURE_NOW - 19 * H,
      },
      { subscription: { kind: "user", did: did("carol") }, sinceMs: FIXTURE_NOW - 10 * D },
      { subscription: { kind: "forum", uri: FORUM_URI }, sinceMs: FIXTURE_NOW - 50 * D },
    ],
    mutes: new Set(),
    settings: {
      sources: "assisted",
      verify: "pds",
      indexers: [],
      externalImagesForums: [],
      mediaAutoloadHosts: [],
      karmaNotifications: "daily",
      acknowledgedPublicActions: [],
    },
    nextId: 1,
  };

  return { state, postUri, commentUri, sequenceUri };
}
