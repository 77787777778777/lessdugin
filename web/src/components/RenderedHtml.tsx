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
// - `[data-cid]` media: "click to load" placeholders (media loading arrives later);
// - hover/focus previews for eligible record links and DIDs, depth 1 (lfm-1.md §5.3).
// After insertion it also substitutes verified handles into mentions (§5.4) and, when asked,
// copies footnotes next to their references as sidenotes for wide screens.

import { useQueryClient } from "@tanstack/react-query";
import { type Ref, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { cx } from "../lib/cx.ts";
import { sanitizerMismatch, setSanitizedHtml } from "../lib/sanitize.ts";
import { isDid, parseAtUri, previewEligible } from "../lib/uri.ts";
import { HoverPreview, type HoverTarget } from "./HoverPreview.tsx";
import { useOpenRef } from "./RecordLink.tsx";
import styles from "./RenderedHtml.module.css";
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

/** lfm-1.md §4.6 "Resolving an anchor", scoped to one rendered root. */
export function scrollToAnchor(root: Element | null, value: string): void {
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
  target.classList.add("lfm-target");
  setTimeout(() => target?.classList.remove("lfm-target"), 1500);
}

/** Replaces media with "click to load" placeholders (runtime DOM changes, lfm-1.md §8.2). */
function addMediaPlaceholders(root: Element): void {
  for (const el of root.querySelectorAll("img[data-cid], span.media[data-cid]")) {
    const cid = el.getAttribute("data-cid") ?? "";
    const isImage = el.localName === "img";
    const label =
      (isImage ? el.getAttribute("alt") : el.textContent)?.trim() || (isImage ? "image" : "media");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "lfm-media-load";
    button.setAttribute("data-media-cid", cid);
    button.textContent = `${isImage ? "🖼" : "▶"} ${label} (click to load)`;
    el.classList.add("lfm-media-pending");
    el.before(button);
  }
}

/** Copies each footnote next to its first reference, for wide screens. */
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
  }
}

export function RenderedHtml({
  html,
  className,
  preview = false,
  sidenotes = false,
  ref,
}: {
  html: string;
  className?: string | undefined;
  /** Inside a hover preview: no nested previews (depth 1). */
  preview?: boolean | undefined;
  sidenotes?: boolean | undefined;
  ref?: Ref<RenderedHtmlHandle> | undefined;
}) {
  const engine = useEngine();
  const qc = useQueryClient();
  const openRef = useOpenRef();
  const rootRef = useRef<HTMLDivElement>(null);
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
    addMediaPlaceholders(el);
    if (sidenotes) addSidenotes(el);
  }, [html, sidenotes]);

  // Mentions: substitute handles verified both ways, for the first 16 distinct DIDs (§5.4).
  useEffect(() => {
    const root = rootRef.current;
    if (!root || !html.includes('class="mention"')) return;
    let cancelled = false;
    const anchors = [...root.querySelectorAll("a.mention[data-did]")];
    const dids = [...new Set(anchors.map((a) => a.getAttribute("data-did") ?? ""))]
      .filter(isDid)
      .slice(0, MAX_EAGER_MENTIONS);
    for (const did of dids) {
      qc.fetchQuery({
        queryKey: qk.actor(did),
        queryFn: () => engine.resolveActor(did),
        staleTime: 5 * 60_000,
      }).then(
        (actor) => {
          if (cancelled || actor.handleState !== "verified" || !actor.handle) return;
          for (const a of anchors) {
            if (a.getAttribute("data-did") === did && a.isConnected)
              a.textContent = `@${actor.handle}`;
          }
        },
        () => {},
      );
    }
    return () => {
      cancelled = true;
    };
  }, [html, engine, qc]);

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
    const onClick = (e: MouseEvent): void => {
      const target = e.target instanceof Element ? e.target : null;
      const media = target?.closest("button.lfm-media-load");
      if (media && root.contains(media)) {
        e.preventDefault();
        toasts.show({
          message:
            "Media loading arrives in a later milestone: the bytes will be fetched from the author's PDS and checked against their CID first.",
        });
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
    const open = (a: HTMLAnchorElement, delay: number): void => {
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
        open(a, HOVER_DELAY_MS);
    };
    const onFocus = (e: Event): void => {
      const a = linkFrom(e.target);
      if (a && (a.hasAttribute("data-at-uri") || a.hasAttribute("data-did"))) open(a, 0);
    };
    const onOut = (e: Event): void => {
      if (linkFrom(e.target)) scheduleClose();
    };
    root.addEventListener("click", onClick);
    if (!preview) {
      root.addEventListener("mouseover", onOver);
      root.addEventListener("mouseout", onOut);
      root.addEventListener("focusin", onFocus);
      root.addEventListener("focusout", onOut);
    }
    return () => {
      root.removeEventListener("click", onClick);
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
