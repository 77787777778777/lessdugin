// SPDX-License-Identifier: AGPL-3.0-or-later
// The default view of a forum's post list. Until M3 every client shows sort `new` and filter
// `all` by default (the CLI's `ls`, the TUI's Forum screen and this page; conformance review
// M5): `forum create` writes `defaultTier: personal` (data-model.md §6.1) and nothing in M2 can
// promote a post, so the spec's `frontpage` default (social.md §12.3) would show a new forum as
// empty. M3 (tiers and promotion) moves all three clients to `magic` and `frontpage` together.

import type { PostFilter, PostSort } from "@lessdugin/engine/types";

export const DEFAULT_POST_SORT: PostSort = "new";
export const DEFAULT_POST_FILTER: PostFilter = "all";
