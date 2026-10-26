// Mention completion for the composer (docs/plan.md M4: "mention autocomplete").
//
// The rule is deliberately small: find the `@`-token the caret is inside, and offer the
// participants whose handle or display name starts with what was typed. Completion inserts the
// HANDLE, never a DID, because a handle is what a reader recognises and what the renderer's
// verified-mention path re-checks (lfm-1.md §5.4, synthesis-v1.md §3.4) - an unverified handle is
// shown struck through rather than being treated as identity.

/** One completion candidate. */
export interface MentionCandidate {
  did: string;
  /** The handle to insert (no leading `@`). */
  handle: string;
  /** What the reader sees under the handle, if any. */
  label?: string;
}

/** The `@`-token containing `caret`, or null when the caret is not in one. */
export function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  if (caret < 0 || caret > text.length) return null;
  // walk back to the `@` that opens this token
  let i = caret;
  while (i > 0) {
    const c = text[i - 1] ?? "";
    if (c === "@") break;
    // a token cannot contain whitespace or a closing bracket, and a newline always ends it
    if (/[\s()[\],;]/.test(c)) return null;
    i -= 1;
  }
  if (i === 0) return null;
  const start = i - 1;
  // an `@` is only an opener at the start of the text or after whitespace/bracket, so
  // "someone@example" in prose does not open a completion
  const before = start > 0 ? (text[start - 1] ?? "") : "";
  if (before !== "" && !/[\s([]/.test(before)) return null;
  const query = text.slice(start + 1, caret);
  // a handle is [a-z0-9.-]; anything else means this was not a mention
  if (!/^[a-zA-Z0-9.-]*$/.test(query)) return null;
  return { start, query };
}

/** The candidates matching `query`, best first: prefix matches on the handle, then on the label. */
export function rankMentions(
  query: string,
  candidates: readonly MentionCandidate[],
  limit = 8,
): MentionCandidate[] {
  const q = query.toLowerCase();
  const scored: { c: MentionCandidate; rank: number }[] = [];
  for (const c of candidates) {
    const handle = c.handle.toLowerCase();
    if (handle.startsWith(q)) {
      // an exact handle beats a longer one, so typing a whole handle puts it first
      scored.push({ c, rank: handle === q ? 0 : 1 });
    } else if ((c.label ?? "").toLowerCase().startsWith(q)) {
      scored.push({ c, rank: 2 });
    }
  }
  scored.sort((a, b) => a.rank - b.rank || a.c.handle.localeCompare(b.c.handle));
  // one entry per handle, because two DIDs can claim the same one
  const seen = new Set<string>();
  const out: MentionCandidate[] = [];
  for (const { c } of scored) {
    if (seen.has(c.handle)) continue;
    seen.add(c.handle);
    out.push(c);
    if (out.length >= limit) break;
  }
  return out;
}

/** Replaces the token at `start` with `@handle ` and returns the text and the new caret. */
export function applyMention(
  text: string,
  start: number,
  caret: number,
  handle: string,
): { text: string; caret: number } {
  const insert = `@${handle} `;
  return {
    text: text.slice(0, start) + insert + text.slice(caret),
    caret: start + insert.length,
  };
}
