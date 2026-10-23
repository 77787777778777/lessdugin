// SPDX-License-Identifier: AGPL-3.0-or-later
// The application frame: a header with the forum switcher, search, notifications badge and
// account menu; the main landmark (focused on every route change); a bottom navigation bar on
// small screens; and the toast region.

import type { Href } from "@lessdugin/engine/types";
import { useQueryClient } from "@tanstack/react-query";
import { Link, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { useEngine, useEngineHandle } from "../engine/context.tsx";
import { useCurrentForum } from "../engine/currentForum.ts";
import { qk, useActiveAccount, useFollowed, useUnreadNotifications } from "../engine/queries.ts";
import { useWritesInFlight } from "../engine/writes.ts";
import { cleanName } from "../lib/text.ts";
import { parseAtUri } from "../lib/uri.ts";
import { useAccountActor } from "./AccountName.tsx";
import { actorLabel } from "./ActorName.tsx";
import styles from "./AppShell.module.css";
import { SafeLink } from "./SafeLink.tsx";
import { errorMessage } from "./Status.tsx";
import { ToastRegion, toasts } from "./Toast.tsx";

function ForumSwitcher() {
  const followed = useFollowed();
  const current = useCurrentForum();
  const navigate = useNavigate();
  const forums = followed.data ?? [];
  if (forums.length === 0) return null;
  return (
    <label className={styles.switcher}>
      <span className="visually-hidden">Forum</span>
      <select
        value={current ?? ""}
        onChange={(e) => {
          const p = parseAtUri(e.target.value);
          if (p) void navigate({ to: "/f/$owner/$slug", params: { owner: p.did, slug: p.rkey } });
        }}
      >
        {current === null && <option value="">Choose a forum…</option>}
        {forums.map((f) => (
          <option key={f.uri} value={f.uri}>
            {cleanName(f.alias ?? f.name)}
          </option>
        ))}
      </select>
    </label>
  );
}

function AccountMenu() {
  const handle = useEngineHandle();
  const qc = useQueryClient();
  const active = useActiveAccount();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: Event): void => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target))
        setOpen(false);
    };
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const account = active.data;
  const actor = useAccountActor(account);
  if (!account || !actor) {
    return (
      <Link to="/login" className={styles.signIn}>
        Sign in
      </Link>
    );
  }
  return (
    <div className={styles.account} ref={ref}>
      <button
        type="button"
        className={styles.accountButton}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className={styles.avatar} aria-hidden="true">
          {actorLabel(actor).slice(0, 1).toUpperCase()}
        </span>
        <bdi className={styles.accountName}>{actorLabel(actor)}</bdi>
      </button>
      {open && (
        <div className={styles.menu} role="menu">
          <Link
            to="/u/$actor"
            params={{ actor: account.did }}
            role="menuitem"
            className={styles.menuItem}
            onClick={() => setOpen(false)}
          >
            Your profile
          </Link>
          <Link
            to="/drafts"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => setOpen(false)}
          >
            Drafts
          </Link>
          <Link
            to="/bookmarks"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => setOpen(false)}
          >
            Bookmarks
          </Link>
          <Link
            to="/settings"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => setOpen(false)}
          >
            Settings
          </Link>
          <Link
            to="/login"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => setOpen(false)}
          >
            Add or switch account
          </Link>
          <button
            type="button"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => {
              setOpen(false);
              handle.login.logout(account.did).then(
                () => qc.invalidateQueries(),
                (e: unknown) => toasts.show({ message: errorMessage(e), tone: "error" }),
              );
            }}
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

function NotificationsLink({
  className,
  label,
}: {
  className: string | undefined;
  label: boolean;
}) {
  const unread = useUnreadNotifications();
  const n = unread.data?.items.length ?? 0;
  return (
    <Link
      to="/notifications"
      className={className}
      aria-label={`Notifications${n > 0 ? ` (${n} unread)` : ""}`}
    >
      <span aria-hidden="true">🔔</span>
      {label && <span className={styles.navLabel}>Inbox</span>}
      {n > 0 && (
        <span className={styles.badge} aria-hidden="true" data-testid="unread-count">
          {n > 99 ? "99+" : n}
        </span>
      )}
    </Link>
  );
}

function SearchBox() {
  const navigate = useNavigate();
  const [q, setQ] = useState("");
  return (
    <search className={styles.search}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void navigate({ to: "/search", search: q.trim() === "" ? {} : { q: q.trim() } });
        }}
      >
        <label>
          <span className="visually-hidden">Search</span>
          <input
            type="search"
            className={styles.searchInput}
            placeholder="Search"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </label>
      </form>
    </search>
  );
}

function EngineBanner() {
  const handle = useEngineHandle();
  if (!handle.notice) return null;
  return (
    <p
      className={styles.engineBanner}
      data-testid={handle.kind === "mock" ? "mock-banner" : "engine-banner"}
    >
      {handle.notice}
    </p>
  );
}

/** Where the Corresponding Source of this build is offered (AGPL-3.0 §13; threat-model SITE-2). */
const SOURCE_URL: string =
  import.meta.env.VITE_SOURCE_URL || "https://github.com/77787777778777/lessdugin";

function sourceHref(url: string): Href | null {
  try {
    const u = new URL(url);
    return u.protocol === "https:" ? { kind: "external", url: u.href, host: u.hostname } : null;
  } catch {
    return null;
  }
}

/** Moves focus to the new page's heading (or the main landmark) after navigation to another
 * page. Keyed on the path only: a change of search parameters (a sort, a filter chip) keeps the
 * page, so focus stays on the control the user is operating. Pages that load their data first
 * render their heading later: focus waits on the main landmark and moves to the heading when it
 * appears, unless the user has moved it meanwhile. */
function useRouteFocus(mainRef: React.RefObject<HTMLElement | null>): void {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const first = useRef(true);
  useEffect(() => {
    void pathname;
    if (first.current) {
      first.current = false;
      return;
    }
    const main = mainRef.current;
    if (!main) return;
    const focus = (target: HTMLElement): void => {
      if (!target.hasAttribute("tabindex")) target.setAttribute("tabindex", "-1");
      target.focus({ preventScroll: true });
    };
    const heading = (): HTMLElement | null => {
      const h1 = main.querySelector("h1");
      return h1 instanceof HTMLElement ? h1 : null;
    };
    let observer: MutationObserver | null = null;
    let giveUp: ReturnType<typeof setTimeout> | undefined;
    const t = setTimeout(() => {
      const h1 = heading();
      if (h1) {
        focus(h1);
        return;
      }
      focus(main);
      observer = new MutationObserver(() => {
        const h = heading();
        if (!h) return;
        observer?.disconnect();
        const active = document.activeElement;
        if (active === main || active === document.body || active === null) focus(h);
      });
      observer.observe(main, { childList: true, subtree: true });
      giveUp = setTimeout(() => observer?.disconnect(), 5000);
    }, 0);
    return () => {
      clearTimeout(t);
      clearTimeout(giveUp);
      observer?.disconnect();
    };
  }, [pathname, mainRef]);
}

export function AppShell() {
  const mainRef = useRef<HTMLElement>(null);
  const source = sourceHref(SOURCE_URL);
  const inFlight = useWritesInFlight();
  useRouteFocus(mainRef);
  // Keep the active account warm for components that need "mine".
  const engine = useEngine();
  const qc = useQueryClient();
  useEffect(() => {
    void qc.prefetchQuery({ queryKey: qk.activeAccount(), queryFn: () => engine.activeAccount() });
  }, [engine, qc]);
  return (
    <div className={styles.shell}>
      <button
        type="button"
        className={styles.skip}
        onClick={() => {
          mainRef.current?.focus();
        }}
      >
        Skip to content
      </button>
      <header className={styles.header}>
        <div className={styles.headerInner}>
          <Link to="/" className={styles.brand}>
            LessDugin
          </Link>
          <ForumSwitcher />
          <SearchBox />
          <nav className={styles.headerNav} aria-label="Main">
            <NotificationsLink className={styles.navIcon} label={false} />
            <Link to="/drafts" className={styles.navText}>
              Drafts
            </Link>
          </nav>
          {inFlight > 0 && (
            <span className={styles.saving} role="status">
              Saving…
            </span>
          )}
          <AccountMenu />
        </div>
      </header>
      <EngineBanner />
      <main id="main" ref={mainRef} tabIndex={-1} className={styles.main}>
        <Outlet />
      </main>
      <nav className={styles.bottomNav} aria-label="Mobile">
        <Link to="/" className={styles.bottomItem} activeOptions={{ exact: true }}>
          <span aria-hidden="true">🏠</span>
          <span className={styles.navLabel}>Home</span>
        </Link>
        <Link to="/search" className={styles.bottomItem}>
          <span aria-hidden="true">🔍</span>
          <span className={styles.navLabel}>Search</span>
        </Link>
        <NotificationsLink className={styles.bottomItem} label />
        <Link to="/drafts" className={styles.bottomItem}>
          <span aria-hidden="true">📝</span>
          <span className={styles.navLabel}>Drafts</span>
        </Link>
        <Link to="/settings" className={styles.bottomItem}>
          <span aria-hidden="true">⚙</span>
          <span className={styles.navLabel}>Settings</span>
        </Link>
      </nav>
      <footer className={styles.footer}>
        <p>
          LessDugin: a forum whose database is your own PDS. Free software under the GNU AGPL v3 or
          later
          {source && (
            <>
              {": "}
              <SafeLink href={source}>source code</SafeLink>
            </>
          )}
          .
        </p>
      </footer>
      <ToastRegion />
    </div>
  );
}
