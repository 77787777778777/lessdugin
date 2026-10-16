// SPDX-License-Identifier: AGPL-3.0-or-later
// Edits start from the author's own record (raw JSON, untrusted): what the form does not show
// must survive the edit, and malformed fields must not.

import { describe, expect, it } from "vitest";
import { forumDraftOf, forumDraftProblems } from "../components/ForumForm.tsx";
import { commentSourceOf } from "../components/OwnCommentActions.tsx";
import { postDraftOf } from "../pages/forum/ForumNewPostPage.tsx";

const DID = "did:plc:abcdefghijklmnopqrstuvwx";
const CID = "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy";
const POST = `at://${DID}/online.7778777.lessdugin.temp.post/3kabc`;
const PARENT = `at://${DID}/online.7778777.lessdugin.temp.comment/3kdef`;
const media = {
  blob: { $type: "blob", ref: { $link: CID }, mimeType: "image/png", size: 1234 },
  alt: "a cat",
  aspectRatio: { width: 4, height: 3 },
};
const md = (text: string, extra: object = {}) => ({
  $type: "online.7778777.lessdugin.markdown",
  text,
  ...extra,
});

describe("comment edits (data-model.md §8.3)", () => {
  it("keep the parent, the quote and the body's media", () => {
    const record = {
      root: { uri: POST, cid: CID },
      parent: { uri: PARENT, cid: CID },
      quote: { exact: "same view", prefix: "the ", start: 3 },
      body: md("Edited ![cat](cid:x)", { media: [media] }),
    };
    expect(commentSourceOf(record)).toEqual({
      markdown: "Edited ![cat](cid:x)",
      parent: PARENT,
      quote: { exact: "same view", prefix: "the ", start: 3 },
      media: [
        {
          id: CID,
          cid: CID,
          mime: "image/png",
          sizeBytes: 1234,
          alt: "a cat",
          aspectRatio: { width: 4, height: 3 },
        },
      ],
    });
  });

  it("drop malformed parts and refuse a body without inline text", () => {
    const record = {
      quote: { exact: 7 },
      body: md("x", { media: [{ blob: { ref: { $link: "not a cid" } } }, "junk"] }),
    };
    expect(commentSourceOf(record)).toEqual({ markdown: "x", media: [] });
    expect(commentSourceOf({ body: { textBlob: {} } })).toBeNull();
    expect(commentSourceOf(null)).toBeNull();
  });
});

describe("post edits", () => {
  it("keep media, labels and the link's metadata", () => {
    const record = {
      kind: "link",
      title: "T",
      body: md("b", { media: [media] }),
      tags: ["a", 3],
      labels: ["nsfw"],
      link: { uri: "https://example.com/", title: "Example", description: "d" },
    };
    const d = postDraftOf(record, "at://f");
    expect(d?.body.media).toHaveLength(1);
    expect(d?.tags).toEqual(["a"]);
    expect(d?.labels).toEqual(["nsfw"]);
    expect(d?.link).toEqual({ uri: "https://example.com/", title: "Example", description: "d" });
    expect(postDraftOf({ kind: "poem", body: md("b") }, "at://f")).toBeNull();
  });
});

describe("forum declaration edits (data-model.md §6.1)", () => {
  it("read the declaration, keeping moderators and the about body's media", () => {
    const record = {
      name: "Forum",
      description: "Tagline",
      about: md("Rules", { media: [media] }),
      membership: "open",
      defaultTier: "frontpage",
      featuredTags: ["ai", "Not A Slug", 4],
      moderators: [
        { did: DID, role: "admin", title: "Lead" },
        { did: "not a did", role: "admin" },
        { did: DID, role: "owner" },
      ],
      reportContact: "mailto:mods@example.com",
    };
    expect(forumDraftOf(record, "forum")).toEqual({
      slug: "forum",
      name: "Forum",
      description: "Tagline",
      about: { markdown: "Rules", media: [expect.objectContaining({ cid: CID })] },
      membership: "open",
      defaultTier: "frontpage",
      featuredTags: ["ai"],
      moderators: [{ did: DID, role: "admin", title: "Lead" }],
      reportContact: "mailto:mods@example.com",
    });
  });

  it("read an absent membership as approval, the spec default", () => {
    expect(forumDraftOf({ name: "F" }, "f")?.membership).toBe("approval");
    expect(forumDraftOf({}, "f")).toBeNull();
  });

  it("validate the slug, name, featured tags and report contact", () => {
    const ok = forumDraftOf({ name: "F", membership: "open" }, "good-slug");
    if (!ok) throw new Error("unreadable");
    expect(forumDraftProblems(ok, true)).toEqual([]);
    expect(forumDraftProblems({ ...ok, slug: "Bad_Slug" }, true)).toHaveLength(1);
    expect(forumDraftProblems({ ...ok, slug: "Bad_Slug" }, false)).toEqual([]);
    expect(forumDraftProblems({ ...ok, name: " " }, false)).toHaveLength(1);
    expect(forumDraftProblems({ ...ok, featuredTags: ["x_y"] }, false)).toHaveLength(1);
    expect(forumDraftProblems({ ...ok, reportContact: "javascript:alert(1)" }, false)).toHaveLength(
      1,
    );
    expect(forumDraftProblems({ ...ok, reportContact: "at://mods.example" }, false)).toEqual([]);
  });
});
