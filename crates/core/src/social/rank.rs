// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ranking (`magic/1`, `social.md` §12): `hot`, the post sorts and the comment sorts.
//!
//! ```text
//! hot(b, t)  = b / pow(max(now − t, 0) / 3_600_000_000 + offsetHours, exponentMilli / 1000)
//! posts      magic:  pinned desc, b > 0 desc, b > 0 ? hot desc : (b desc, t_rank desc), uri asc
//!                    (only t_rank within cutoffDays of now, or pinned)
//!            top:    score desc, t_rank desc, uri asc
//!            new:    t_rank desc, uri asc        old: t_rank asc, uri asc
//!            recentComments: activity desc, uri asc
//! comments   pins first; magic as posts with b = score (no bonuses, no cutoff); top; new; old;
//!            latestReply: latest desc; every key ends with uri asc
//! ```
//!
//! `hot` is the only float of the fold (§1.3): `libm::pow`, compared with `total_cmp`.

use std::cmp::Ordering;

use crate::dto::{CommentSort, PostSort};
use crate::social::policy::EffectivePolicy;

/// µs per hour.
const HOUR_US: f64 = 3_600_000_000.0;
/// µs per day.
const DAY_US: i64 = 86_400_000_000;

/// `hot(b, t)` (§12.1), in exactly the specified order of operations.
pub fn hot(b: i64, t: i64, now: i64, offset_hours: i64, exponent_milli: i64) -> f64 {
    let age = now.saturating_sub(t).max(0) as f64;
    let h = age / HOUR_US;
    let base = h + (offset_hours as f64);
    let e = (exponent_milli as f64) / 1000.0;
    (b as f64) / libm::pow(base, e)
}

/// The sort keys of a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostKey {
    /// The uri.
    pub uri: String,
    /// `t_rank` (µs).
    pub t_rank: i64,
    /// `score`.
    pub score: i64,
    /// The bonus-adjusted score `b(p)`.
    pub b: i64,
    /// Forum-pinned.
    pub pinned: bool,
    /// `activity(p)` for `recentComments`.
    pub activity: i64,
}

fn magic(a: (bool, i64, i64), b: (bool, i64, i64), now: i64, policy: &EffectivePolicy) -> Ordering {
    let (pa, ba, ta) = a;
    let (pb, bb, tb) = b;
    pb.cmp(&pa)
        .then_with(|| (bb > 0).cmp(&(ba > 0)))
        .then_with(|| {
            if ba > 0 && bb > 0 {
                let ha = hot(ba, ta, now, policy.offset_hours, policy.exponent_milli);
                let hb = hot(bb, tb, now, policy.offset_hours, policy.exponent_milli);
                hb.total_cmp(&ha)
            } else {
                bb.cmp(&ba).then_with(|| tb.cmp(&ta))
            }
        })
}

/// Orders posts for `sort` (§12.2); `magic` also drops unpinned posts older than the cutoff.
pub fn order_posts(
    mut posts: Vec<PostKey>,
    sort: PostSort,
    now: i64,
    policy: &EffectivePolicy,
) -> Vec<PostKey> {
    if sort == PostSort::Magic {
        let cutoff = policy.cutoff_days.saturating_mul(DAY_US);
        posts.retain(|p| p.pinned || now.saturating_sub(p.t_rank) <= cutoff);
    }
    posts.sort_by(|a, b| {
        match sort {
            PostSort::Magic => magic(
                (a.pinned, a.b, a.t_rank),
                (b.pinned, b.b, b.t_rank),
                now,
                policy,
            ),
            PostSort::Top => b.score.cmp(&a.score).then(b.t_rank.cmp(&a.t_rank)),
            PostSort::New => b.t_rank.cmp(&a.t_rank),
            PostSort::Old => a.t_rank.cmp(&b.t_rank),
            PostSort::RecentComments => b.activity.cmp(&a.activity),
        }
        .then_with(|| a.uri.cmp(&b.uri))
    });
    posts
}

/// The sort keys of a comment among its siblings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentKey {
    /// The uri.
    pub uri: String,
    /// `t_rank` (µs).
    pub t_rank: i64,
    /// `score`.
    pub score: i64,
    /// Thread-pinned.
    pub pinned: bool,
    /// `latest(c)` for `latestReply`.
    pub latest: i64,
}

/// Compares two sibling comments for `sort` (§12.4).
pub fn cmp_comments(
    a: &CommentKey,
    b: &CommentKey,
    sort: CommentSort,
    now: i64,
    policy: &EffectivePolicy,
) -> Ordering {
    b.pinned
        .cmp(&a.pinned)
        .then_with(|| match sort {
            CommentSort::Magic => magic(
                (false, a.score, a.t_rank),
                (false, b.score, b.t_rank),
                now,
                policy,
            ),
            CommentSort::Top => b.score.cmp(&a.score).then(b.t_rank.cmp(&a.t_rank)),
            CommentSort::New => b.t_rank.cmp(&a.t_rank),
            CommentSort::Old => a.t_rank.cmp(&b.t_rank),
            CommentSort::LatestReply => b.latest.cmp(&a.latest),
        })
        .then_with(|| a.uri.cmp(&b.uri))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_atp::syntax::Datetime;
    use lessdugin_lexicon::records::Forum;
    use lessdugin_lexicon::values::MembershipMode;

    fn policy() -> EffectivePolicy {
        let f = Forum::create(
            "x",
            Datetime::parse("2026-01-01T00:00:00Z").unwrap(),
            MembershipMode::Open,
        );
        EffectivePolicy::of(&f)
    }

    fn post(uri: &str, t: i64, score: i64, pinned: bool) -> PostKey {
        PostKey {
            uri: uri.into(),
            t_rank: t,
            score,
            b: score,
            pinned,
            activity: t,
        }
    }

    #[test]
    fn hot_is_exact() {
        // b / (0 + 2)^1.15
        let h = hot(10, 5, 5, 2, 1150);
        assert_eq!(h, 10.0 / libm::pow(2.0, 1.15));
        assert_eq!(hot(1, 10, 0, 1, 0), 1.0, "a future t has age 0");
    }

    #[test]
    fn post_sorts() {
        let p = policy();
        let now = 100 * DAY_US;
        let posts = vec![
            post("a", now - DAY_US, 0, false),
            post("b", now - 2 * DAY_US, 5, false),
            post("c", now - 200 * DAY_US / 2, 1, false), // 100 days old: past the cutoff
            post("d", now - 3 * DAY_US, 0, true),
            post("e", now - DAY_US, 0, false),
        ];
        let uris = |v: Vec<PostKey>| v.into_iter().map(|p| p.uri).collect::<Vec<_>>();
        assert_eq!(
            uris(order_posts(posts.clone(), PostSort::Magic, now, &p)),
            ["d", "b", "a", "e"]
        );
        assert_eq!(
            uris(order_posts(posts.clone(), PostSort::New, now, &p)),
            ["a", "e", "b", "d", "c"]
        );
        assert_eq!(
            uris(order_posts(posts.clone(), PostSort::Old, now, &p)),
            ["c", "d", "b", "a", "e"]
        );
        assert_eq!(
            uris(order_posts(posts, PostSort::Top, now, &p)),
            ["b", "c", "a", "e", "d"]
        );
    }

    #[test]
    fn comment_sorts() {
        let p = policy();
        let k = |u: &str, t: i64, pinned: bool| CommentKey {
            uri: u.into(),
            t_rank: t,
            score: 0,
            pinned,
            latest: t,
        };
        let mut v = [k("a", 1, false), k("b", 3, false), k("c", 2, true)];
        v.sort_by(|a, b| cmp_comments(a, b, CommentSort::Old, 10, &p));
        assert_eq!(
            v.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        v.sort_by(|a, b| cmp_comments(a, b, CommentSort::Magic, 10, &p));
        assert_eq!(
            v.iter().map(|c| c.uri.as_str()).collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
    }
}
