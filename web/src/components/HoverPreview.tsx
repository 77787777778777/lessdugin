// SPDX-License-Identifier: AGPL-3.0-or-later
// Hover and focus previews for record links and DIDs in rendered content (lfm-1.md §5.3).
// Depth 1: the preview's own RenderedHtml runs in preview mode and opens no further previews.
// The target is re-parsed by the engine before use (F5). Bodies are shown only for posts that
// resolve as visible or collapsed; everything else gets a metadata card with "open".

import type { Ref as EngineRef } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useEffect, useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri, recordKind } from "../lib/uri.ts";
import { ActorName } from "./ActorName.tsx";
import styles from "./HoverPreview.module.css";
import { RecordLink } from "./RecordLink.tsx";
import { RenderedHtml } from "./RenderedHtml.tsx";

export interface HoverTarget {
  /** The raw `data-at-uri` or `data-did` value. */
  raw: string;
  rect: DOMRect;
}

const WIDTH = 384;

function PostPreview({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.post(uri), queryFn: () => engine.getPost(uri) });
  if (q.isPending) return <p className={styles.muted}>Loading…</p>;
  if (q.isError || !q.data) return <MetaCard uri={uri} />;
  const { post, body } = q.data;
  const listed = post.state.kind === "visible" || post.state.kind === "collapsed";
  return (
    <>
      <p className={styles.kicker}>
        {post.kind === "quickTake" ? "Quick take" : post.kind === "question" ? "Question" : "Post"}{" "}
        by <ActorName actor={post.author} link={false} />
      </p>
      {post.title && (
        <p className={styles.title}>
          <bdi>{cleanName(post.title, 300)}</bdi>
        </p>
      )}
      {post.state.kind === "collapsed" && (
        <p className={styles.muted}>Collapsed ({post.state.reason})</p>
      )}
      {listed ? (
        <div className={styles.body}>
          <RenderedHtml html={body.html} preview className="lfm-compact" />
        </div>
      ) : (
        <p className={styles.muted}>This item is not shown.</p>
      )}
      <RecordLink uri={uri} className={styles.open}>
        Open →
      </RecordLink>
    </>
  );
}

function UserCard({ did }: { did: string }) {
  const engine = useEngine();
  const q = useQuery({
    queryKey: qk.user(did, undefined),
    queryFn: () => engine.user(undefined, did),
  });
  if (q.isPending) return <p className={styles.muted}>Loading…</p>;
  if (q.isError || !q.data) return <p className={styles.muted}>Unknown user.</p>;
  return (
    <>
      <p className={styles.title}>
        <ActorName actor={q.data.actor} showIdent />
      </p>
      {q.data.bio && <p className={styles.excerpt}>{q.data.bio.excerpt}</p>}
    </>
  );
}

function MetaCard({ uri }: { uri: string }) {
  const p = parseAtUri(uri);
  const kind = p ? recordKind(p.collection) : "other";
  return (
    <>
      <p className={styles.kicker}>{kind === "other" ? "Record" : kind}</p>
      <p className={styles.mono}>{uri}</p>
      <RecordLink uri={uri} className={styles.open}>
        Open →
      </RecordLink>
    </>
  );
}

function SequencePreview({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.sequence(uri), queryFn: () => engine.sequence(uri) });
  if (!q.data) return <MetaCard uri={uri} />;
  return (
    <>
      <p className={styles.kicker}>
        Sequence by <ActorName actor={q.data.sequence.author} link={false} />
      </p>
      <p className={styles.title}>
        <bdi>{cleanName(q.data.sequence.title, 300)}</bdi>
      </p>
      <p className={styles.muted}>
        {q.data.sequence.chapters} chapters, {q.data.sequence.posts} posts
      </p>
      <RecordLink uri={uri} className={styles.open}>
        Open →
      </RecordLink>
    </>
  );
}

function ForumPreview({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.forumRef(uri), queryFn: () => engine.forumOpen(uri) });
  if (!q.data) return <MetaCard uri={uri} />;
  return (
    <>
      <p className={styles.kicker}>Forum</p>
      <p className={styles.title}>
        <bdi>{cleanName(q.data.forum.name)}</bdi>
      </p>
      {q.data.forum.description && <p className={styles.excerpt}>{q.data.forum.description}</p>}
      <RecordLink uri={uri} className={styles.open}>
        Open →
      </RecordLink>
    </>
  );
}

function PreviewBody({ target }: { target: EngineRef }): ReactNode {
  switch (target.kind) {
    case "actor":
      return <UserCard did={target.did} />;
    case "post":
      return <PostPreview uri={target.uri} />;
    case "sequence":
      return <SequencePreview uri={target.uri} />;
    case "forum":
      return <ForumPreview uri={target.uri} />;
    default:
      return <MetaCard uri={target.uri} />;
  }
}

export function HoverPreview({
  target,
  onClose,
  onPointerEnter,
  onPointerLeave,
}: {
  target: HoverTarget;
  onClose: () => void;
  onPointerEnter: () => void;
  onPointerLeave: () => void;
}) {
  const engine = useEngine();
  const parsed = useQuery({
    queryKey: qk.ref(target.raw),
    queryFn: () => engine.parseRef(target.raw, {}),
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  const ref = useRef<HTMLDivElement>(null);

  // Position through CSS custom properties (CSSOM; no style attributes, CSP style-src 'self').
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { rect } = target;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const left = Math.max(8, Math.min(rect.left, vw - WIDTH - 8));
    const below = rect.bottom + 8;
    const above = vh - rect.top + 8;
    el.style.setProperty("--hp-left", `${left}px`);
    if (below + 240 < vh || rect.top < 240) {
      el.style.setProperty("--hp-top", `${below}px`);
      el.style.removeProperty("--hp-bottom");
    } else {
      el.style.setProperty("--hp-bottom", `${above}px`);
      el.style.removeProperty("--hp-top");
    }
  }, [target]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      ref={ref}
      className={styles.preview}
      role="dialog"
      aria-label="Link preview"
      data-preview-root=""
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
    >
      {parsed.data ? (
        <PreviewBody target={parsed.data} />
      ) : parsed.isError ? (
        <p className={styles.muted}>This link could not be resolved.</p>
      ) : (
        <p className={styles.muted}>Loading…</p>
      )}
    </div>,
    document.body,
  );
}
