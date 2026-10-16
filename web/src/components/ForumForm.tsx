// SPDX-License-Identifier: AGPL-3.0-or-later
// Creating a forum and editing its declaration (engine-api.md §3.2 `forum_create`,
// `forum_update`; data-model.md §6.1). An edit starts from the owner's own record (`raw`), so
// fields the form does not show (moderators, the about body's media) are kept as they are.
// Moderators are changed one at a time with `mods_set` (ForumSettingsPage), which adopts a
// removed moderator's actions first (social.md §4.10).

import type {
  ForumDraft,
  MembershipMode,
  ModeratorDraft,
  ModRole,
  TagSummary,
} from "@lessdugin/engine/types";
import { type FormEvent, useId, useState } from "react";
import { markdownOf, mediaRefsOf, objectField, stringField, stringsField } from "../lib/records.ts";
import { cleanName } from "../lib/text.ts";
import { isDid } from "../lib/uri.ts";
import ui from "../styles/ui.module.css";
import { errorMessage } from "./Status.tsx";

export const SLUG_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const MAX_SLUG_BYTES = 64;
const MAX_FEATURED_TAGS = 64;
/** data-model.md §6.1 `reportContact`: `mailto:`, `https:` or `at://`. */
const REPORT_CONTACT_RE = /^(?:mailto:|https:\/\/|at:\/\/)\S+$/;

export const EMPTY_FORUM_DRAFT: ForumDraft = {
  slug: "",
  name: "",
  membership: "approval",
  defaultTier: "personal",
  featuredTags: [],
  moderators: [],
};

function moderatorsOf(record: unknown): ModeratorDraft[] {
  const list = (record as { moderators?: unknown } | null)?.moderators;
  if (!Array.isArray(list)) return [];
  const out: ModeratorDraft[] = [];
  for (const m of list) {
    const did = stringField(m, "did");
    const role = stringField(m, "role");
    if (!did || !isDid(did) || (role !== "admin" && role !== "moderator")) continue;
    const d: ModeratorDraft = { did, role: role as ModRole };
    const title = stringField(m, "title");
    if (title !== undefined) d.title = title;
    out.push(d);
  }
  return out;
}

/** The forum declaration being edited, as a draft (record JSON is untrusted: every field is
 * checked; `slug` is the record key). */
export function forumDraftOf(record: unknown, slug: string): ForumDraft | null {
  const name = stringField(record, "name");
  if (name === undefined) return null;
  const membership = stringField(record, "membership");
  const tier = stringField(record, "defaultTier");
  const d: ForumDraft = {
    slug,
    name,
    // An absent `membership` reads as `approval` (data-model.md §6.1 default).
    membership: membership === "open" ? "open" : "approval",
    defaultTier: tier === "frontpage" ? "frontpage" : "personal",
    featuredTags: stringsField(record, "featuredTags").filter((t) => SLUG_RE.test(t)),
    moderators: moderatorsOf(record),
  };
  const description = stringField(record, "description");
  if (description !== undefined) d.description = description;
  const about = objectField(record, "about");
  const aboutMd = markdownOf(about);
  if (aboutMd !== undefined) d.about = { markdown: aboutMd, media: mediaRefsOf(about) };
  const contact = stringField(record, "reportContact");
  if (contact !== undefined) d.reportContact = contact;
  return d;
}

/** Problems with a draft, in form order (empty when it can be sent). */
export function forumDraftProblems(d: ForumDraft, creating: boolean): string[] {
  const out: string[] = [];
  if (creating) {
    if (!SLUG_RE.test(d.slug)) {
      out.push(
        "The address must be lowercase letters and digits, in words joined by single hyphens.",
      );
    } else if (new TextEncoder().encode(d.slug).length > MAX_SLUG_BYTES) {
      out.push(`The address is at most ${MAX_SLUG_BYTES} characters.`);
    }
  }
  if (d.name.trim() === "") out.push("The forum needs a name.");
  if (d.featuredTags.length > MAX_FEATURED_TAGS)
    out.push(`At most ${MAX_FEATURED_TAGS} featured tags.`);
  const bad = d.featuredTags.filter((t) => !SLUG_RE.test(t));
  if (bad.length > 0) out.push(`Not a tag slug: ${bad.join(", ")}`);
  if (d.reportContact !== undefined && !REPORT_CONTACT_RE.test(d.reportContact)) {
    out.push("The report contact must start with mailto:, https:// or at://.");
  }
  return out;
}

function splitTags(s: string): string[] {
  return [
    ...new Set(
      s
        .split(/[\s,]+/)
        .map((t) => t.trim())
        .filter((t) => t !== ""),
    ),
  ];
}

export function ForumForm({
  initial,
  creating,
  tags = [],
  submitLabel,
  onSubmit,
  onCancel,
}: {
  initial: ForumDraft;
  creating: boolean;
  /** The forum's existing tags, offered as featured tags. */
  tags?: readonly TagSummary[] | undefined;
  submitLabel: string;
  /** Resolves to true when the write was done. */
  onSubmit: (draft: ForumDraft) => Promise<boolean>;
  onCancel?: (() => void) | undefined;
}) {
  const ids = {
    slug: useId(),
    name: useId(),
    description: useId(),
    about: useId(),
    featured: useId(),
    contact: useId(),
    hint: useId(),
  };
  const [slug, setSlug] = useState(initial.slug);
  const [name, setName] = useState(initial.name);
  const [description, setDescription] = useState(initial.description ?? "");
  const [about, setAbout] = useState(initial.about?.markdown ?? "");
  const [membership, setMembership] = useState<MembershipMode>(initial.membership);
  const [tier, setTier] = useState(initial.defaultTier);
  const [featured, setFeatured] = useState(initial.featuredTags.join(" "));
  const [contact, setContact] = useState(initial.reportContact ?? "");
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const draft = (): ForumDraft => {
    const d: ForumDraft = {
      slug: creating ? slug.trim() : initial.slug,
      name: name.trim(),
      membership,
      defaultTier: tier,
      featuredTags: splitTags(featured),
      moderators: initial.moderators,
    };
    if (description.trim() !== "") d.description = description.trim();
    if (about.trim() !== "") d.about = { markdown: about, media: initial.about?.media ?? [] };
    if (contact.trim() !== "") d.reportContact = contact.trim();
    return d;
  };

  const submit = async (e: FormEvent): Promise<void> => {
    e.preventDefault();
    if (busy) return;
    const d = draft();
    const found = forumDraftProblems(d, creating);
    setProblems(found);
    if (found.length > 0) return;
    setBusy(true);
    try {
      await onSubmit(d);
    } catch (err) {
      setProblems([errorMessage(err)]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className={ui.stack} onSubmit={(e) => void submit(e)} noValidate>
      {creating && (
        <div className={ui.field}>
          <label className={ui.label} htmlFor={ids.slug}>
            Address
          </label>
          <input
            id={ids.slug}
            className={ui.input}
            value={slug}
            maxLength={MAX_SLUG_BYTES}
            autoCapitalize="none"
            spellCheck={false}
            aria-describedby={ids.hint}
            onChange={(e) => setSlug(e.target.value.toLowerCase())}
            required
          />
          <span id={ids.hint} className={ui.hint}>
            The forum's record key, part of every link to it (for example <code>rationality</code>
            ). It cannot be changed later.
          </span>
        </div>
      )}
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.name}>
          Name
        </label>
        <input
          id={ids.name}
          className={ui.input}
          value={name}
          maxLength={100}
          onChange={(e) => setName(e.target.value)}
          required
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.description}>
          Tagline <span className={ui.hint}>(optional, plain text)</span>
        </label>
        <input
          id={ids.description}
          className={ui.input}
          value={description}
          maxLength={500}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.about}>
          About <span className={ui.hint}>(optional, markdown: rules and introduction)</span>
        </label>
        <textarea
          id={ids.about}
          className={ui.input}
          rows={6}
          value={about}
          onChange={(e) => setAbout(e.target.value)}
        />
      </div>
      <fieldset className={ui.field}>
        <legend className={ui.label}>Membership</legend>
        <div className={ui.row}>
          {(
            [
              ["approval", "Approval required"],
              ["open", "Open"],
            ] as const
          ).map(([value, label]) => (
            <label key={value} className={membership === value ? ui.chipActive : ui.chip}>
              <input
                type="radio"
                name={`${ids.name}-membership`}
                className="visually-hidden"
                checked={membership === value}
                onChange={() => setMembership(value)}
              />
              {label}
            </label>
          ))}
        </div>
      </fieldset>
      <fieldset className={ui.field}>
        <legend className={ui.label}>Default tier of new posts</legend>
        <div className={ui.row}>
          {(
            [
              ["personal", "Personal blog"],
              ["frontpage", "Frontpage"],
            ] as const
          ).map(([value, label]) => (
            <label key={value} className={tier === value ? ui.chipActive : ui.chip}>
              <input
                type="radio"
                name={`${ids.name}-tier`}
                className="visually-hidden"
                checked={tier === value}
                onChange={() => setTier(value)}
              />
              {label}
            </label>
          ))}
        </div>
      </fieldset>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.featured}>
          Featured tags <span className={ui.hint}>(tag slugs, separated by spaces)</span>
        </label>
        <input
          id={ids.featured}
          className={ui.input}
          value={featured}
          autoCapitalize="none"
          spellCheck={false}
          onChange={(e) => setFeatured(e.target.value)}
        />
        {tags.length > 0 && (
          <span className={ui.hint}>
            Existing tags:{" "}
            {tags.map((t, i) => (
              <span key={t.slug}>
                {i > 0 && ", "}
                <code>{t.slug}</code> (<bdi>{cleanName(t.name)}</bdi>)
              </span>
            ))}
          </span>
        )}
      </div>
      <div className={ui.field}>
        <label className={ui.label} htmlFor={ids.contact}>
          Report contact <span className={ui.hint}>(optional: mailto:, https:// or at://)</span>
        </label>
        <input
          id={ids.contact}
          className={ui.input}
          value={contact}
          inputMode="url"
          autoCapitalize="none"
          spellCheck={false}
          onChange={(e) => setContact(e.target.value)}
        />
      </div>
      {problems.length > 0 && (
        <div className={ui.error} role="alert">
          <ul>
            {problems.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
        </div>
      )}
      <p className={ui.small}>
        The declaration is a public record in your repository. Every reader's app applies its
        current version, so policy and membership changes apply retroactively.
      </p>
      <div className={ui.row}>
        <button type="submit" className={ui.primary} disabled={busy}>
          {busy ? "Saving…" : submitLabel}
        </button>
        {onCancel && (
          <button type="button" className={ui.ghost} onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}
