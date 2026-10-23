// SPDX-License-Identifier: AGPL-3.0-or-later
// The default view of a forum's post list: sort `magic`, filter `frontpage`
// (the M3 defaults of `engine-api.md` §3.4 / `social.md` §12.3, switched from
// M2's `new`/`all` together with the CLI's `ls` and the TUI's forum screen so
// the three clients show the same posts for the same forum).

import type { PostFilter, PostSort } from "@lessdugin/engine/types";

export const DEFAULT_POST_SORT: PostSort = "magic";
export const DEFAULT_POST_FILTER: PostFilter = "frontpage";
