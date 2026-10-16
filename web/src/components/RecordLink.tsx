// SPDX-License-Identifier: AGPL-3.0-or-later
// Router links for engine-supplied at-uris (post, comment, sequence, forum; anything else goes
// through the #/at/* resolver).

import type { Ref } from "@lessdugin/engine/types";
import { Link, useNavigate } from "@tanstack/react-router";
import { type ReactNode, useCallback } from "react";
import { parseAtUri, recordKind } from "../lib/uri.ts";

export function RecordLink({
  uri,
  children,
  className,
}: {
  uri: string;
  children: ReactNode;
  className?: string | undefined;
}) {
  const p = parseAtUri(uri);
  if (!p) return <span className={className}>{children}</span>;
  switch (recordKind(p.collection)) {
    case "post":
      return (
        <Link to="/p/$author/$rkey" params={{ author: p.did, rkey: p.rkey }} className={className}>
          {children}
        </Link>
      );
    case "comment":
      return (
        <Link to="/c/$author/$rkey" params={{ author: p.did, rkey: p.rkey }} className={className}>
          {children}
        </Link>
      );
    case "sequence":
      return (
        <Link to="/s/$author/$rkey" params={{ author: p.did, rkey: p.rkey }} className={className}>
          {children}
        </Link>
      );
    case "forum":
      return (
        <Link to="/f/$owner/$slug" params={{ owner: p.did, slug: p.rkey }} className={className}>
          {children}
        </Link>
      );
    default:
      return (
        <Link to="/at/$" params={{ _splat: uri.slice("at://".length) }} className={className}>
          {children}
        </Link>
      );
  }
}

/** Navigates to the page for an engine `Ref`. */
export function useOpenRef(): (ref: Ref, opts?: { replace?: boolean }) => void {
  const navigate = useNavigate();
  return useCallback(
    (ref: Ref, opts?: { replace?: boolean }) => {
      const replace = opts?.replace ?? false;
      if (ref.kind === "actor") {
        void navigate({ to: "/u/$actor", params: { actor: ref.did }, replace });
        return;
      }
      const p = parseAtUri(ref.uri);
      if (!p) return;
      switch (ref.kind) {
        case "post":
          void navigate({
            to: "/p/$author/$rkey",
            params: { author: p.did, rkey: p.rkey },
            replace,
          });
          return;
        case "comment":
          void navigate({
            to: "/c/$author/$rkey",
            params: { author: p.did, rkey: p.rkey },
            replace,
          });
          return;
        case "sequence":
          void navigate({
            to: "/s/$author/$rkey",
            params: { author: p.did, rkey: p.rkey },
            replace,
          });
          return;
        case "forum":
          void navigate({ to: "/f/$owner/$slug", params: { owner: p.did, slug: p.rkey }, replace });
          return;
        case "record":
          void navigate({
            to: "/at/$",
            params: { _splat: ref.uri.slice("at://".length) },
            replace,
          });
      }
    },
    [navigate],
  );
}
