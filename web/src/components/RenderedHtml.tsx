// SPDX-License-Identifier: AGPL-3.0-or-later
//
// RenderedHtml: the ONLY component that puts renderer HTML into the DOM (threat-model SPA-2,
// SPA-3; lfm-1.md §8.5). It never uses dangerouslySetInnerHTML: the HTML goes through the
// `lessdugin-md` Trusted Types policy, i.e. DOMPurify with exactly the LFM/1 allowlist, into a
// ref'd element that React does not otherwise manage. In development it asserts that the
// sanitiser leaves the renderer's output unchanged (lfm-1.md §8.3).
//
// One set of delegated listeners handles the rendered root:
// - `a[data-at-uri]` / `a[data-did]`: re-parsed by the engine (F5), then routed in-app;
// - `a[data-anchor]`: scrolls inside this root only (lfm-1.md §4.6; never `location.hash`);
// - `[data-cid]` media: "click to load" placeholders naming the host that will see the reader's
//   address (sync.md §11.3); a click is the consent (synthesis-v1.md 3.9), or the engine's
//   `MediaView.autoload` verdict for the item (a hover preview's target is the item, lfm-1.md
//   §5.3; the composer's preview never autoloads), and the bytes then come from the author's
//   PDS through `engine.loadMedia`, which streams, CID-checks and sniffs them and hands back a
//   `blob:` URL with the display type. An image or a player gets that URL (lib/media.ts,
//   lib/mediaUrl.ts), anything else becomes a download card; the URL is revoked with the
//   content (threat-model SPA-7);
// - hover/focus previews for eligible record links and DIDs, depth 1 (lfm-1.md §5.3).
// After insertion it also substitutes verified handles into mentions (§5.4: eagerly only for
// the first 16 distinct mentioned DIDs that are forum participants; any other mention only once
// its identity is in the cache, e.g. after a hover preview resolved it), makes in-content anchor
// links keyboard-reachable, names task-list checkboxes, and, when asked, copies footnotes next to
// their references as sidenotes for wide screens.

import type { EngineApi } from "@lessdugin/engine/api";
import type { ActorView, LoadedMedia, MediaView } from "@lessdugin/engine/types";
import { useQueryClient } from "@tanstack/react-query";
import { type Ref, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { cx } from "../lib/cx.ts";
import { downloadObjectUrl } from "../lib/download.ts";
import { loadMediaFor, mediaDisplay, mediaHostNotice } from "../lib/media.ts";
import { setMediaSource } from "../lib/mediaUrl.ts";
import { sanitizerMismatch, setSanitizedHtml } from "../lib/sanitize.ts";
import { isDid, parseAtUri, previewEligible } from "../lib/uri.ts";
import { HoverPreview, type HoverTarget } from "./HoverPreview.tsx";
import { useParticipants } from "./participants.tsx";
import { useOpenRef } from "./RecordLink.tsx";
import styles from "./RenderedHtml.module.css";
import { errorMessage } from "./Status.tsx";
import { toasts } from "./Toast.tsx";

export interface RenderedHtmlHandle {
  root(): HTMLDivElement | null;
  /** Scrolls to a content anchor (`md-…` id, or a heading slug) inside this root. */
  scrollToAnchor(value: string): void;
}

const MAX_EAGER_MENTIONS = 16;
const HOVER_DELAY_MS = 350;
const CLOSE_DELAY_MS = 250;

function elementsWithId(root: Element, id: string): Element | null {
  for (const el of root.querySelectorAll("[id]")) if (el.id === id) return el;
  return null;
}

/** lfm-1.md §4.6 "Resolving an anchor", scoped to one rendered root. With `focus`, keyboard
 * focus follows (the target is made programmatically focusable). */
export function scrollToAnchor(
  root: Element | null,
  value: string,
  opts: { focus?: boolean } = {},
): void {
  if (!root || value === "") return;
  const id = value.startsWith("md-") ? value : `md-h-${value}`;
  let target = elementsWithId(root, id);
  const fn = /^md-fn-([1-9][0-9]*)$/.exec(id);
  if (fn && target instanceof HTMLElement && target.offsetParent === null) {
    // Footnotes are shown as sidenotes on wide screens; go to the sidenote instead.
    for (const note of root.querySelectorAll("[data-sidenote]")) {
      if (note.getAttribute("data-sidenote") === fn[1]) target = note;
    }
  }
  if (!target) return;
  target.scrollIntoView?.({ block: "center" });
  if (opts.focus && target instanceof HTMLElement) {
    if (!target.hasAttribute("tabindex")) target.setAttribute("tabindex", "-1");
    target.focus({ preventScroll: true });
  }
  target.classList.add("lfm-target");
  setTimeout(() => target?.classList.remove("lfm-target"), 1500);
}

/** What a media placeholder is called: its alt or label (lfm-1.md §6.2), else its kind. */
function mediaLabel(el: Element): string {
  const isImage = el.localName === "img";
  return (
    (isImage ? el.getAttribute("alt") : el.textContent)?.trim() || (isImage ? "image" : "media")
  );
}

/** Replaces media with "click to load" placeholders (runtime DOM changes, lfm-1.md §8.2), each
 * with the notice of sync.md §11.3. Returns the placeholders of media the engine lets load
 * without a click (`MediaView.autoload`). */
function addMediaPlaceholders(
  root: Element,
  media: readonly MediaView[] | undefined,
): HTMLButtonElement[] {
  const auto: HTMLButtonElement[] = [];
  for (const el of root.querySelectorAll("img[data-cid], span.media[data-cid]")) {
    const cid = el.getAttribute("data-cid") ?? "";
    const isImage = el.localName === "img";
    const label = mediaLabel(el);
    const view = media?.find((m) => m.cid === cid);
    const button = document.createElement("button");
    button.type = "button";
    button.className = "lfm-media-load";
    button.setAttribute("data-media-cid", cid);
    button.textContent = `${isImage ? "🖼" : "▶"} ${label} (click to load; ${mediaHostNotice(view)})`;
    el.classList.add("lfm-media-pending");
    el.before(button);
    if (view?.autoload === true) auto.push(button);
  }
  return auto;
}

/** Loads the media of the placeholder `button` into `root` (a click, or the engine's autoload
 * verdict, is the consent: the bytes come from the author's PDS, which learns this device's
 * address). The host streamed the bytes, checked them against the CID and sniffed them; the
 * `blob:` URL is pushed to `revokers`, to go with the content it is shown in. */
function loadPlaceholder(
  engine: EngineApi,
  root: Element,
  button: Element,
  view: MediaView | undefined,
  revokers: (() => void)[],
): void {
  if (button.getAttribute("aria-busy") === "true") return;
  const cid = button.getAttribute("data-media-cid") ?? "";
  if (cid === "") return;
  button.setAttribute("aria-busy", "true");
  button.textContent = "loading…";
  const next = button.nextElementSibling;
  const pending =
    next?.getAttribute("data-cid") === cid
      ? next
      : root.querySelector(`[data-cid="${CSS.escape(cid)}"]`);
  const show = async (): Promise<void> => {
    const loaded = await loadMediaFor(engine, cid, view);
    const revoke = (): void => URL.revokeObjectURL(loaded.url);
    if (pending === null || !pending.isConnected || !root.contains(pending)) {
      revoke();
      return;
    }
    revokers.push(revoke);
    showMedia(pending, loaded);
    button.remove();
  };
  void show().catch((err: unknown) => {
    button.removeAttribute("aria-busy");
    button.textContent = `could not load: ${errorMessage(err)}`;
  });
}

/** Shows the loaded media of the placeholder `pending` (lfm-1.md §6.2, sync.md §11.2): as the
 * image itself or a player when its display type allows it, else as a download card. */
function showMedia(pending: Element, loaded: LoadedMedia): void {
  const display = mediaDisplay(loaded, pending.localName === "img" ? "img" : "span");
  if (display.kind === "img" && pending instanceof HTMLImageElement) {
    setMediaSource(pending, loaded.url);
    pending.classList.remove("lfm-media-pending");
    return;
  }
  if (display.kind === "video" || display.kind === "audio") {
    // a player with controls, because a media element without them cannot be started by
    // keyboard
    const el = document.createElement(display.kind);
    setMediaSource(el, loaded.url);
    el.setAttribute("controls", "");
    el.setAttribute("preload", "metadata");
    el.className = "lfm-media";
    pending.replaceWith(el);
    return;
  }
  // A download card: saved as `<cid>.<ext>`, never opened in the page (threat-model SPA-7).
  const card = document.createElement("span");
  card.className = "lfm-media-card";
  const save = document.createElement("button");
  save.type = "button";
  save.className = "lfm-media-download";
  save.textContent = `⤓ Download ${mediaLabel(pending)} (${display.filename})`;
  save.addEventListener("click", () => downloadObjectUrl(loaded.url, display.filename));
  card.append(save);
  pending.replaceWith(card);
}

/** In-content anchor links (`a[data-anchor]`: footnote references, back-links, `#frag` links)
 * have no `href`, so they are made focusable links by hand; Enter activates them through the
 * delegated handler. */
function makeAnchorsFocusable(root: Element): void {
  for (const a of root.querySelectorAll("a[data-anchor]")) {
    a.setAttribute("tabindex", "0");
    a.setAttribute("role", "link");
  }
}

/** A display-maths block scrolls horizontally when the equation is wider than the column
 * (`content.css`), so it must be focusable or a keyboard user can never scroll it. The renderer
 * output is fixed by the blessed LFM vectors, so the attribute is added here rather than there. */
function makeMathScrollable(root: Element): void {
  for (const el of root.querySelectorAll('math[display="block"]')) {
    el.setAttribute("tabindex", "0");
  }
}

/** Task-list checkboxes (`<input type="checkbox" disabled="">` + the item's text) get the
 * item's text as their accessible name; a bare checkbox would be announced without one. */
function labelTaskCheckboxes(root: Element): void {
  for (const input of root.querySelectorAll('input[type="checkbox"]')) {
    const item = input.parentElement;
    if (!item) continue;
    let text = "";
    for (const n of item.childNodes) {
      if (n instanceof Element && (n.localName === "ul" || n.localName === "ol")) break;
      text += n.textContent ?? "";
    }
    input.setAttribute("aria-label", text.replace(/\s+/g, " ").trim().slice(0, 200) || "Task");
  }
}

/** Copies each footnote next to its first reference, for wide screens. Only the copied
 * definitions are hidden there: one whose first reference is not in the root (inside a
 * flattened spoiler, or cut by the output budget) stays in the footnotes section, which is
 * hidden only when every definition was copied (lfm-1.md §8.1). */
function addSidenotes(root: Element): void {
  for (const sup of root.querySelectorAll("sup[id]")) {
    const m = /^md-fnref-([1-9][0-9]*)-1$/.exec(sup.id);
    if (!m?.[1]) continue;
    const li = elementsWithId(root, `md-fn-${m[1]}`);
    if (!li) continue;
    const note = document.createElement("span");
    note.className = "lfm-sidenote";
    note.setAttribute("role", "note");
    note.setAttribute("data-sidenote", m[1]);
    const num = document.createElement("span");
    num.className = "lfm-sidenote-n";
    num.textContent = m[1];
    note.append(num);
    for (const child of li.childNodes) {
      if (child instanceof Element && child.classList.contains("fn-back")) continue;
      if (child instanceof Element && child.localName === "p") {
        for (const c of child.childNodes) note.append(c.cloneNode(true));
        note.append(" ");
      } else {
        note.append(child.cloneNode(true));
      }
    }
    for (const withId of note.querySelectorAll("[id]")) withId.removeAttribute("id");
    sup.after(note);
    li.classList.add("lfm-sidenoted");
  }
  for (const section of root.querySelectorAll("section.footnotes")) {
    const items = [...section.querySelectorAll("li[id]")].filter((li) => /^md-fn-/.test(li.id));
    if (items.every((li) => li.classList.contains("lfm-sidenoted"))) {
      section.classList.add("lfm-all-sidenoted");
      continue;
    }
    // the definitions left in the list keep their numbers when the others are hidden
    for (const li of items) {
      const n = /^md-fn-([1-9][0-9]*)$/.exec(li.id)?.[1];
      if (n) li.setAttribute("value", n);
    }
  }
}

export function RenderedHtml({
  html,
  media,
  className,
  preview = false,
  autoload = true,
  sidenotes = false,
  ref,
}: {
  html: string;
  /** The body's media (`RenderedBody.media`): each names its author's PDS, declared type and
   * size for `loadMedia`. */
  media?: readonly MediaView[] | undefined;
  className?: string | undefined;
  /** Inside a hover preview: no nested previews (depth 1). */
  preview?: boolean | undefined;
  /** false: the media load only on a click, whatever their `MediaView.autoload` (the
   * composer's preview of an unpublished draft). */
  autoload?: boolean | undefined;
  sidenotes?: boolean | undefined;
  ref?: Ref<RenderedHtmlHandle> | undefined;
}) {
  const engine = useEngine();
  const qc = useQueryClient();
  const openRef = useOpenRef();
  const participants = useParticipants();
  const rootRef = useRef<HTMLDivElement>(null);
  const mediaRef = useRef(media);
  mediaRef.current = media;
  /** Revokes the object URLs of the media shown in the current content. */
  const revokers = useRef<(() => void)[]>([]);
  const [mismatch, setMismatch] = useState(false);
  const [hover, setHover] = useState<HoverTarget | null>(null);
  const timers = useRef<{
    open?: ReturnType<typeof setTimeout>;
    close?: ReturnType<typeof setTimeout>;
  }>({});

  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    if (import.meta.env.DEV) {
      const out = sanitizerMismatch(html);
      setMismatch(out !== null);
      if (out !== null) {
        console.error("RenderedHtml: DOMPurify changed the renderer's output (lfm-1.md §8.3)", {
          html,
          sanitized: out,
        });
      }
    }
    setSanitizedHtml(el, html);
    const auto = addMediaPlaceholders(el, mediaRef.current);
    makeAnchorsFocusable(el);
    labelTaskCheckboxes(el);
    makeMathScrollable(el);
    if (sidenotes) {
      addSidenotes(el);
      makeAnchorsFocusable(el);
    }
    // the engine's autoload verdict (sync.md §11.3), inside a hover preview too (lfm-1.md §5.3:
    // the views are the target item's)
    const shown = revokers.current;
    if (autoload) {
      for (const button of auto) {
        const cid = button.getAttribute("data-media-cid");
        loadPlaceholder(
          engine,
          el,
          button,
          mediaRef.current?.find((m) => m.cid === cid),
          shown,
        );
      }
    }
    // the content is replaced or unmounted: its media URLs go with it (SPA-7)
    return () => {
      for (const revoke of shown.splice(0)) revoke();
    };
  }, [html, sidenotes, autoload, engine]);

  // Mentions: substitute handles verified both ways (§5.4). Eager resolution covers only the
  // participants among the first 16 distinct mentioned DIDs; the others are substituted only
  // from the identity cache, i.e. after the reader hovered, focused or opened them.
  useEffect(() => {
    const root = rootRef.current;
    if (!root || !html.includes('class="mention"')) return;
    let cancelled = false;
    const anchors = [...root.querySelectorAll("a.mention[data-did]")];
    const dids = [...new Set(anchors.map((a) => a.getAttribute("data-did") ?? ""))].filter(isDid);
    const substitute = (did: string, actor: ActorView | undefined): void => {
      if (cancelled || !actor || actor.handleState !== "verified" || !actor.handle) return;
      for (const a of anchors) {
        if (a.getAttribute("data-did") === did && a.isConnected) a.textContent = `@${actor.handle}`;
      }
    };
    const eager = new Set(dids.slice(0, MAX_EAGER_MENTIONS).filter((d) => participants.has(d)));
    for (const did of dids) {
      if (eager.has(did)) {
        qc.fetchQuery({
          queryKey: qk.actor(did),
          queryFn: () => engine.resolveActor(did),
          staleTime: 5 * 60_000,
        }).then(
          (actor) => substitute(did, actor),
          () => {},
        );
      } else {
        substitute(did, qc.getQueryData<ActorView>(qk.actor(did)));
      }
    }
    const lazy = new Set(dids.filter((d) => !eager.has(d)));
    const unsubscribe =
      lazy.size === 0
        ? () => {}
        : qc.getQueryCache().subscribe((event) => {
            const [kind, did] = event.query.queryKey as readonly unknown[];
            if (event.type !== "updated" || kind !== "actor" || typeof did !== "string") return;
            if (lazy.has(did)) substitute(did, event.query.state.data as ActorView | undefined);
          });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [html, engine, qc, participants]);

  useImperativeHandle(
    ref,
    () => ({
      root: () => rootRef.current,
      scrollToAnchor: (value: string) => scrollToAnchor(rootRef.current, value),
    }),
    [],
  );

  // Delegated listeners on the rendered root.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const t = timers.current;
    const linkFrom = (target: EventTarget | null): HTMLAnchorElement | null => {
      if (!(target instanceof Element)) return null;
      const a = target.closest("a");
      return a && root.contains(a) ? a : null;
    };
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== "Enter" || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
      const a = linkFrom(e.target);
      const anchor = a?.getAttribute("data-anchor");
      if (!a || anchor === null || anchor === undefined) return;
      e.preventDefault();
      scrollToAnchor(root, anchor, { focus: true });
    };
    const onClick = (e: MouseEvent): void => {
      const target = e.target instanceof Element ? e.target : null;
      const media = target?.closest("button.lfm-media-load");
      if (media && root.contains(media)) {
        e.preventDefault();
        // Clicking IS the consent (synthesis-v1.md 3.9): nothing is fetched until the reader
        // asks, unless the engine's autoload verdict allowed it.
        const cid = media.getAttribute("data-media-cid");
        const view = mediaRef.current?.find((m) => m.cid === cid);
        loadPlaceholder(engine, root, media, view, revokers.current);
        return;
      }
      const a = linkFrom(e.target);
      if (!a) return;
      const anchor = a.getAttribute("data-anchor");
      if (anchor !== null) {
        e.preventDefault();
        scrollToAnchor(root, anchor);
        return;
      }
      const raw = a.getAttribute("data-at-uri") ?? a.getAttribute("data-did");
      if (raw === null || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey)
        return;
      e.preventDefault();
      setHover(null);
      engine
        .parseRef(raw, {})
        .then(openRef, () =>
          toasts.show({ message: "This link could not be resolved.", tone: "error" }),
        );
    };
    const targetFor = (a: HTMLAnchorElement): HoverTarget | null => {
      const uri = a.getAttribute("data-at-uri");
      if (uri !== null) {
        const p = parseAtUri(uri);
        return p && previewEligible(p.collection)
          ? { raw: uri, rect: a.getBoundingClientRect() }
          : null;
      }
      const did = a.getAttribute("data-did");
      return did !== null && isDid(did) ? { raw: did, rect: a.getBoundingClientRect() } : null;
    };
    const openPreview = (a: HTMLAnchorElement, delay: number): void => {
      clearTimeout(t.close);
      clearTimeout(t.open);
      t.open = setTimeout(() => {
        const target = targetFor(a);
        if (target) setHover(target);
      }, delay);
    };
    const scheduleClose = (): void => {
      clearTimeout(t.open);
      clearTimeout(t.close);
      t.close = setTimeout(() => setHover(null), CLOSE_DELAY_MS);
    };
    const onOver = (e: Event): void => {
      const a = linkFrom(e.target);
      if (a && (a.hasAttribute("data-at-uri") || a.hasAttribute("data-did")))
        openPreview(a, HOVER_DELAY_MS);
    };
    const onFocus = (e: Event): void => {
      const a = linkFrom(e.target);
      if (a && (a.hasAttribute("data-at-uri") || a.hasAttribute("data-did"))) openPreview(a, 0);
    };
    const onOut = (e: Event): void => {
      if (linkFrom(e.target)) scheduleClose();
    };
    root.addEventListener("click", onClick);
    root.addEventListener("keydown", onKeyDown);
    if (!preview) {
      root.addEventListener("mouseover", onOver);
      root.addEventListener("mouseout", onOut);
      root.addEventListener("focusin", onFocus);
      root.addEventListener("focusout", onOut);
    }
    return () => {
      root.removeEventListener("click", onClick);
      root.removeEventListener("keydown", onKeyDown);
      root.removeEventListener("mouseover", onOver);
      root.removeEventListener("mouseout", onOut);
      root.removeEventListener("focusin", onFocus);
      root.removeEventListener("focusout", onOut);
      clearTimeout(t.open);
      clearTimeout(t.close);
    };
  }, [engine, openRef, preview]);

  return (
    <div className={cx(styles.wrapper, sidenotes && "lfm-sidenotes", className)}>
      {mismatch && (
        <p className={styles.devWarning} role="alert">
          Development check failed: the sanitiser changed this body. See the console.
        </p>
      )}
      <div ref={rootRef} data-rendered-root="" data-preview={preview ? "" : undefined} />
      {hover && !preview && (
        <HoverPreview
          target={hover}
          onClose={() => setHover(null)}
          onPointerEnter={() => clearTimeout(timers.current.close)}
          onPointerLeave={() => {
            clearTimeout(timers.current.close);
            timers.current.close = setTimeout(() => setHover(null), CLOSE_DELAY_MS);
          }}
        />
      )}
    </div>
  );
}
