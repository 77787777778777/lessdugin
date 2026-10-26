// SPDX-License-Identifier: AGPL-3.0-or-later
// The markdown composer: CodeMirror 6 with the markdown language, a toolbar (bold, italic,
// link, image, math, spoiler), a live preview rendered by the engine (150 ms debounce) and
// draft autosave through `saveDraft`. A save still in flight when the text is published (or
// discarded) deletes the draft it wrote instead of resurrecting it.
//
// CSP: CodeMirror injects its base theme through style-mod. In a document that is a <style>
// element, which `style-src 'self'` forbids; in a shadow root it is a constructed stylesheet
// (adoptedStyleSheets), which CSP allows. So the editor is mounted in an open shadow root, and
// themed with CSS custom properties, which inherit through the shadow boundary.

import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView, keymap, placeholder as placeholderExt } from "@codemirror/view";
import type { Draft, RenderedBody } from "@lessdugin/engine/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { cx } from "../lib/cx.ts";
import { applyMention, type MentionCandidate, mentionAt, rankMentions } from "../lib/mentions.ts";
import ui from "../styles/ui.module.css";
import { type Attachment, Attachments } from "./Attachments.tsx";
import styles from "./Composer.module.css";
import { RenderedHtml } from "./RenderedHtml.tsx";
import { errorMessage } from "./Status.tsx";

const PREVIEW_DEBOUNCE_MS = 150;
const DRAFT_DEBOUNCE_MS = 1000;

const editorTheme = EditorView.theme({
  "&": {
    color: "var(--color-text)",
    backgroundColor: "var(--color-surface)",
    fontSize: "0.95rem",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-content": {
    fontFamily: "var(--font-mono)",
    minHeight: "var(--composer-min-height, 7rem)",
    padding: "0.6rem 0.75rem",
    caretColor: "var(--color-text)",
  },
  ".cm-scroller": { lineHeight: "1.55" },
  ".cm-placeholder": { color: "var(--color-text-faint)" },
  ".cm-cursor": { borderLeftColor: "var(--color-text)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--color-primary-soft)",
  },
});

type Wrap = { before: string; after: string; placeholder: string; block?: boolean };

const BOLD: Wrap = { before: "**", after: "**", placeholder: "bold text" };
const ITALIC: Wrap = { before: "_", after: "_", placeholder: "italic text" };

const TOOLS: { id: string; label: string; glyph: string; wrap: Wrap }[] = [
  { id: "bold", label: "Bold", glyph: "B", wrap: BOLD },
  { id: "italic", label: "Italic", glyph: "I", wrap: ITALIC },
  {
    id: "link",
    label: "Link",
    glyph: "🔗",
    wrap: { before: "[", after: "](https://)", placeholder: "link text" },
  },
  {
    id: "image",
    label: "Image",
    glyph: "🖼",
    wrap: { before: "![", after: "](cid:)", placeholder: "alt text" },
  },
  { id: "math", label: "Math", glyph: "∑", wrap: { before: "$", after: "$", placeholder: "x^2" } },
  {
    id: "spoiler",
    label: "Spoiler",
    glyph: "▣",
    wrap: { before: "> [!spoiler]\n> ", after: "", placeholder: "hidden text", block: true },
  },
];

/** Wraps every selection range in `before`/`after` (inserting a placeholder when empty). */
export function applyWrap(view: EditorView, w: Wrap): void {
  const tr = view.state.changeByRange((range) => {
    const text = view.state.sliceDoc(range.from, range.to) || w.placeholder;
    const line = view.state.doc.lineAt(range.from);
    const lead = w.block && range.from !== line.from ? "\n\n" : "";
    const before = lead + w.before;
    const body = w.block ? text.replace(/\n/g, "\n> ") : text;
    return {
      changes: { from: range.from, to: range.to, insert: before + body + w.after },
      range: EditorSelection.range(
        range.from + before.length,
        range.from + before.length + body.length,
      ),
    };
  });
  view.dispatch(tr);
  view.focus();
}

export function Composer({
  initial = "",
  draftId: initialDraftId,
  makeDraft,
  submitLabel = "Submit",
  placeholder = "Write in markdown…",
  label = "Comment",
  autoFocus = false,
  compact = false,
  attachments = false,
  forumUri = undefined,
  onSubmit,
  onCancel,
}: {
  initial?: string | undefined;
  draftId?: string | undefined;
  /** Builds the draft to autosave for `markdown`; omit to disable autosave. */
  makeDraft?: ((markdown: string, id: string | undefined) => Draft) | undefined;
  submitLabel?: string | undefined;
  placeholder?: string | undefined;
  label?: string | undefined;
  autoFocus?: boolean | undefined;
  compact?: boolean | undefined;
  /** Resolves to true when the text was accepted (the composer then clears). */
  onSubmit: (markdown: string, attachments: Attachment[]) => Promise<boolean>;
  /** Show the attachment picker (a post's composer does; a reply box does not, M4). */
  attachments?: boolean | undefined;
  /** The forum whose members can be mentioned; omit to offer no completion (M4). */
  forumUri?: string | undefined;
  onCancel?: (() => void) | undefined;
}) {
  const engine = useEngine();
  // Who can be mentioned: the forum's members, by handle. Only a VERIFIED handle is offered,
  // because inserting an unverified one would assert an identity this client cannot vouch
  // for (synthesis-v1.md §3.4 shows those struck through).
  const members = useQuery({
    queryKey: qk.members(forumUri ?? ""),
    queryFn: () => engine.members(forumUri as string, {}),
    enabled: forumUri !== undefined,
    staleTime: 60_000,
  });
  const candidatesRef = useRef<MentionCandidate[]>([]);
  // the first candidate, for the keyboard; set below once `mentions` is known
  const pendingMention = useRef<MentionCandidate | null>(null);
  const acceptMentionRef = useRef<(c: MentionCandidate) => void>(() => {});
  candidatesRef.current = (members.data ?? []).flatMap((m) =>
    m.actor.handle !== undefined && m.actor.handleState === "verified"
      ? [
          {
            did: m.actor.did,
            handle: m.actor.handle,
            ...(m.actor.displayName ? { label: m.actor.displayName } : {}),
          },
        ]
      : [],
  );
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [text, setText] = useState(initial);
  const [preview, setPreview] = useState<RenderedBody | null>(null);
  const [showPreview, setShowPreview] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<Attachment[]>([]);
  // mention completion (M4): the token under the caret and the candidates for it
  const [mentions, setMentions] = useState<{ start: number; items: MentionCandidate[] } | null>(
    null,
  );
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const qc = useQueryClient();
  const submitRef = useRef<() => void>(() => {});
  // The editor is created once; these only seed it.
  const mountOptions = useRef({ initial, placeholder, autoFocus, label });
  // Autosave state lives in refs: async completions must see the latest values.
  const draftIdRef = useRef<string | undefined>(initialDraftId);
  const makeDraftRef = useRef(makeDraft);
  makeDraftRef.current = makeDraft;
  /** Bumped when the text is published: saves started before then must not leave a draft
   * behind. (Unmounting keeps the draft, which is the point of a draft.) */
  const generation = useRef(0);
  const pendingSave = useRef<Promise<void> | null>(null);
  const autosave = makeDraft !== undefined;

  // Mount CodeMirror inside a shadow root (see the header comment).
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const shadow = host.shadowRoot ?? host.attachShadow({ mode: "open" });
    const parent = document.createElement("div");
    shadow.replaceChildren(parent);
    const { initial, placeholder, autoFocus, label } = mountOptions.current;
    const view = new EditorView({
      parent,
      root: shadow,
      state: EditorState.create({
        doc: initial,
        extensions: [
          history(),
          keymap.of([
            // while the mention popup is open, Enter and Tab take the first candidate;
            // Escape closes it. This is checked against a ref because the keymap is built
            // once, while the popup state changes as the reader types.
            {
              key: "Enter",
              run: () => {
                const c = pendingMention.current;
                if (c === null) return false;
                acceptMentionRef.current(c);
                return true;
              },
            },
            {
              key: "Tab",
              run: () => {
                const c = pendingMention.current;
                if (c === null) return false;
                acceptMentionRef.current(c);
                return true;
              },
            },
            {
              key: "Escape",
              run: () => {
                if (pendingMention.current === null) return false;
                setMentions(null);
                return true;
              },
            },
            {
              key: "Mod-Enter",
              run: () => {
                submitRef.current();
                return true;
              },
            },
            {
              key: "Mod-b",
              run: (v) => {
                applyWrap(v, BOLD);
                return true;
              },
            },
            {
              key: "Mod-i",
              run: (v) => {
                applyWrap(v, ITALIC);
                return true;
              },
            },
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          markdown(),
          EditorView.lineWrapping,
          placeholderExt(placeholder),
          editorTheme,
          // The editable element lives in a shadow root, so `aria-labelledby` cannot reach a
          // label in the document: name it directly.
          EditorView.contentAttributes.of({ "aria-label": label, "aria-multiline": "true" }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) setText(u.state.doc.toString());
            // completion follows the caret, so it is recomputed on selection changes too
            const sel = u.state.selection.main;
            const token = mentionAt(u.state.doc.toString(), sel.head);
            const items = token === null ? [] : rankMentions(token.query, candidatesRef.current);
            setMentions(token !== null && items.length > 0 ? { start: token.start, items } : null);
          }),
        ],
      }),
    });
    viewRef.current = view;
    if (autoFocus) view.focus();
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  // Live preview through the engine's renderer.
  useEffect(() => {
    if (text.trim() === "") {
      setPreview(null);
      return;
    }
    let cancelled = false;
    const t = setTimeout(() => {
      engine.render(text, [], { html: true, text: true, styled: false }).then(
        (body) => {
          if (!cancelled) setPreview(body);
        },
        () => {},
      );
    }, PREVIEW_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [text, engine]);

  // Draft autosave.
  useEffect(() => {
    if (!autosave || text.trim() === "" || text === initial) return;
    const gen = generation.current;
    const t = setTimeout(() => {
      const build = makeDraftRef.current;
      if (!build) return;
      const save = engine.saveDraft(build(text, draftIdRef.current)).then(
        (id) => {
          if (generation.current !== gen) {
            // Published meanwhile: this save must not resurrect the draft.
            void engine.deleteDraft(id).catch(() => {});
            return;
          }
          draftIdRef.current = id;
          setSaveError(null);
          setSavedAt(Date.now());
          // a composer mounted again (a reply box scrolled back into a long thread, a reload)
          // resumes from the drafts list
          void qc.invalidateQueries({ queryKey: qk.drafts() });
        },
        (e: unknown) => {
          if (generation.current === gen) setSaveError(errorMessage(e));
        },
      );
      pendingSave.current = save;
    }, DRAFT_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [text, autosave, engine, initial, qc]);

  const acceptMention = (c: MentionCandidate): void => {
    const view = viewRef.current;
    if (!view || mentions === null) return;
    const cur = view.state.doc.toString();
    const caret = view.state.selection.main.head;
    const out = applyMention(cur, mentions.start, caret, c.handle);
    view.dispatch({
      changes: { from: 0, to: cur.length, insert: out.text },
      selection: EditorSelection.cursor(out.caret),
    });
    view.focus();
    setMentions(null);
  };

  // the keymap reads these, so they follow the current state without re-creating the editor
  acceptMentionRef.current = acceptMention;
  pendingMention.current = mentions?.items[0] ?? null;

  const submit = async (): Promise<void> => {
    const value = viewRef.current?.state.doc.toString() ?? text;
    if (value.trim() === "" || busy) return;
    setBusy(true);
    setError(null);
    try {
      const ok = await onSubmit(value, files);
      if (ok) {
        generation.current++;
        const view = viewRef.current;
        view?.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" } });
        // A save still in flight deletes its own draft when it lands (generation changed).
        await pendingSave.current?.catch(() => {});
        const id = draftIdRef.current;
        draftIdRef.current = undefined;
        if (id !== undefined) await engine.deleteDraft(id).catch(() => {});
        setSavedAt(null);
        setFiles([]);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  submitRef.current = () => void submit();

  return (
    <div className={cx(styles.composer, compact && styles.compact)}>
      <div className={styles.toolbar} role="toolbar" aria-label="Formatting">
        {TOOLS.map((t) => (
          <button
            key={t.id}
            type="button"
            className={styles.tool}
            title={t.label}
            aria-label={t.label}
            onClick={() => {
              if (viewRef.current) applyWrap(viewRef.current, t.wrap);
            }}
          >
            <span aria-hidden="true" className={styles[`glyph_${t.id}`]}>
              {t.glyph}
            </span>
          </button>
        ))}
        <span className={styles.spacer} />
        <button
          type="button"
          className={styles.tool}
          aria-pressed={showPreview}
          onClick={() => setShowPreview((s) => !s)}
        >
          Preview
        </button>
      </div>
      <div ref={hostRef} className={styles.editor} data-testid="composer-editor" />
      {mentions && (
        <ul className={styles.mentions} data-testid="mention-menu" aria-label="Mention">
          {mentions.items.map((c, i) => (
            <li key={c.did}>
              <button
                type="button"
                className={styles.mention}
                // the first candidate is the default, so Enter in the editor still works
                aria-current={i === 0 ? "true" : undefined}
                onMouseDown={(e) => {
                  // keep the editor's selection: a blur would move the caret
                  e.preventDefault();
                  acceptMention(c);
                }}
              >
                <span className={styles.mentionHandle}>@{c.handle}</span>
                {c.label && <span className={styles.mentionLabel}>{c.label}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
      {attachments ? <Attachments value={files} onChange={setFiles} disabled={busy} /> : null}
      {showPreview && preview && (
        <section className={styles.preview} aria-label="Preview">
          <RenderedHtml html={preview.html} preview className="lfm-compact" />
          {preview.notices.map((n) => (
            <p key={n} className={styles.notice}>
              {n}
            </p>
          ))}
        </section>
      )}
      {error && (
        <p className={ui.error} role="alert">
          {error}
        </p>
      )}
      <div className={styles.actions}>
        <button
          type="button"
          className={ui.primary}
          disabled={busy || text.trim() === ""}
          onClick={() => void submit()}
        >
          {busy ? "Publishing…" : submitLabel}
        </button>
        {onCancel && (
          <button type="button" className={ui.ghost} onClick={onCancel}>
            Cancel
          </button>
        )}
        <span className={styles.status}>
          {saveError !== null
            ? `Draft not saved: ${saveError}`
            : savedAt !== null
              ? "Draft saved locally"
              : "Ctrl/⌘ + Enter to submit"}
        </span>
      </div>
    </div>
  );
}
