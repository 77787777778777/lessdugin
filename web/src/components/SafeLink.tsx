// SPDX-License-Identifier: AGPL-3.0-or-later
// The only component that renders an `href` (threat-model SPA-4; lfm-1.md §5.1). It accepts
// either an `Href` the engine already produced, or a raw URL from a record field, which it
// passes through the engine's `safeHref(raw, "field")` first. Whatever it renders is checked
// again here: internal links become routes (`#/at/…`, `#/u/…`), external links carry
// `rel="nofollow ugc noopener noreferrer"`, no referrer, and show their real host; anything
// else renders as plain text.

import type { Href, HrefContext } from "@lessdugin/engine/types";
import type { ReactNode } from "react";
import { useSafeHref } from "../engine/queries.ts";
import { cx } from "../lib/cx.ts";
import { isDid } from "../lib/uri.ts";
import styles from "./SafeLink.module.css";

export const EXTERNAL_REL = "nofollow ugc noopener noreferrer";

const EXTERNAL_URL = /^https?:\/\/[^\s"<>\\]+$/i;
const MAILTO_URL = /^mailto:[^\s"<>\\]+$/i;
const RECORD_URI =
  /^at:\/\/did:(plc:[a-z2-7]{24}|web:[a-z0-9.-]+)\/[a-zA-Z0-9.-]+\/[A-Za-z0-9._:~-]+$/;

type Common = { children?: ReactNode; className?: string | undefined };
export type SafeLinkProps =
  | (Common & { href: Href; raw?: never; ctx?: never })
  | (Common & { raw: string; ctx?: HrefContext; href?: never });

function textOf(node: ReactNode): string | null {
  return typeof node === "string" ? node : null;
}

function Plain({ children, className }: Common) {
  return <span className={cx(styles.plain, className)}>{children}</span>;
}

/** Renders an already-safe `Href`. */
export function HrefLink({ href, children, className }: Common & { href: Href }) {
  switch (href.kind) {
    case "external":
    case "mailto": {
      const ok = href.kind === "external" ? EXTERNAL_URL.test(href.url) : MAILTO_URL.test(href.url);
      if (!ok) return <Plain className={className}>{children ?? href.url}</Plain>;
      const label = children ?? href.url;
      const text = textOf(label);
      const showHost = href.host !== "" && text !== href.host;
      return (
        <a
          href={href.url}
          rel={EXTERNAL_REL}
          referrerPolicy="no-referrer"
          target="_blank"
          className={cx(styles.external, className)}
        >
          {label}
          {showHost && <span className={styles.host}>{href.host}</span>}
        </a>
      );
    }
    case "record":
      if (!RECORD_URI.test(href.uri))
        return <Plain className={className}>{children ?? href.uri}</Plain>;
      return (
        <a
          href={`#/at/${href.uri.slice("at://".length)}`}
          className={cx(styles.internal, className)}
        >
          {children ?? href.uri}
        </a>
      );
    case "profile":
      if (!isDid(href.did)) return <Plain className={className}>{children ?? href.did}</Plain>;
      return (
        <a href={`#/u/${href.did}`} className={cx(styles.internal, className)}>
          {children ?? href.did}
        </a>
      );
    case "profileHandle":
      // Navigable only after the handle verifies both ways (lfm-1.md §5.2).
      return <Plain className={className}>{children ?? `@${href.handle}`}</Plain>;
    case "anchor":
      return <Plain className={className}>{children}</Plain>;
  }
}

function RawLink({
  raw,
  ctx = "field",
  children,
  className,
}: Common & { raw: string; ctx?: HrefContext }) {
  const { data } = useSafeHref(raw, ctx);
  if (!data) return <Plain className={className}>{children ?? raw}</Plain>;
  return (
    <HrefLink href={data} className={className}>
      {children}
    </HrefLink>
  );
}

export function SafeLink(props: SafeLinkProps) {
  if (props.href !== undefined) {
    return (
      <HrefLink href={props.href} className={props.className}>
        {props.children}
      </HrefLink>
    );
  }
  return (
    <RawLink raw={props.raw} ctx={props.ctx ?? "field"} className={props.className}>
      {props.children}
    </RawLink>
  );
}
