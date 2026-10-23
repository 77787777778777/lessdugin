// SPDX-License-Identifier: AGPL-3.0-or-later
//! The full social fold (`social.md` §1): `ForumView = fold(FoldInput, Query)` with every layer
//! of §1.4, checked against `testdata/vectors/` (`crates/core/tests/social_fold.rs`). The engine
//! still runs the M2 fold ([`crate::social::fold::fold`]) until the M3 integration switches it to
//! [`fold_view`].
//!
//! | Module | Layer | Spec | Status |
//! |---|---|---|---|
//! | [`cx`] | input | §1.1 well-formedness, §1.6 policy, V1 | implemented |
//! | [`cids`] | – | §0.4 CID comparison by canonical form | implemented |
//! | [`authority`] | L0 | §4.1 authorities and ranks; §1.6 banner | implemented |
//! | [`moderation`] | L1 | §4.2–§4.8, §4.10 forum tier; forum-scope statuses of §4.12 | implemented |
//! | [`participation`] | L2 | §3 participation, seeds, applicants; §4.11 `degraded` | implemented |
//! | [`timeline`] | – | §2 time, §11 thread placement and versions | implemented |
//! | [`modstate`] | L6 (V1–V7) | §10.1 V1–V7 (V6 from the author tier) | implemented |
//! | [`modlog`] | L1/L5 | §4.12 mod log, mod queue, raw view | implemented |
//! | [`karma`] | L3 | §5 opinion validity, §6 power and karma | implemented |
//! | [`scores`] | L3.5 | §7 scores and agreement | implemented |
//! | [`gates`] | L4 | §8 gate values and passes | implemented |
//! | [`author`] | L5 | §4.9 author tier and author-scope statuses | implemented |
//! | [`reacts`] | L5 | §9 the `default/1` palette, react aggregation | implemented |
//! | [`wiki`] | L5 | §13 wiki heads, tag relevance | implemented |
//! | [`sequences`] | L5 | §14 sequence library | **stub** |
//! | [`visibility`] | L6 | §10.1 V8–V10, §10.2 thread pass | implemented |
//! | [`ranking`] | L6 | §12 sorts, filters, Recent Discussion | implemented |
//! | [`project`](mod@project) | – | the vector projection (`testdata/vectors/README.md`) | implemented |
//!
//! A stub returns [`view::Pending`] (some return exact results for inputs that do not exercise
//! them, documented on each); every layer that reads a pending one is pending too. The fold is
//! pure: no I/O, no clock, ordered collections only (§1.3).

pub mod author;
pub mod authority;
pub mod cids;
pub mod cx;
pub mod gates;
pub mod karma;
pub mod moderation;
pub mod modlog;
pub mod modstate;
pub mod participation;
pub mod project;
pub mod query;
pub mod ranking;
pub mod reacts;
pub mod scores;
pub mod sequences;
pub mod timeline;
pub mod view;
pub mod visibility;
pub mod wiki;

use std::collections::BTreeSet;

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::ns::NS;

pub use cx::{Cx, Validity, lexicon_validity};
pub use project::{ProjectError, project, project_fold};
pub use query::Query;
pub use view::{ForumView, Pending, Stage, State};

use crate::social::digest;
use crate::social::fold::FoldError;
use crate::social::input::FoldInput;
use view::Banner;

/// The full fold under this build's namespace ([`NS`]) and lexicon validity.
pub fn fold_view(input: &FoldInput, query: &Query) -> Result<ForumView, FoldError> {
    fold_view_with(input, NS, &lexicon_validity, query)
}

/// The full fold (§1.1) of `input` in namespace `ns`, with record validity `validity`
/// (`data-model.md` §7). Rejects a malformed input (§1.1).
pub fn fold_view_with(
    input: &FoldInput,
    ns: &str,
    validity: Validity<'_>,
    query: &Query,
) -> Result<ForumView, FoldError> {
    let cx = Cx::new(input, ns, validity)?;

    // §1.2 (the preimage is dropped once hashed)
    let input_digest = digest::input_digest(input).map_err(|e| FoldError(e.to_string()))?;

    // L0
    let a = authority::authorities(&cx)?;
    let newer_rules_banner = authority::newer_rules_banner(&cx);
    // L1
    let l1 = moderation::forum_tier(&cx, &a);
    let tiers = cx
        .records()
        .filter(|x| x.coll == Some(Collection::Post) && cx.live(&x.uri))
        .map(|x| (x.uri.clone(), l1.tier(&x.uri)))
        .collect();
    // L2
    let l2 = participation::participation(&cx, &a, &l1);
    let degraded = participation::degraded(&cx, &a);
    // V1–V5, V7 (L0–L2 only)
    let v1_v5 = modstate::v1_v5(&cx, &l1, &l2, degraded);
    let locked_out = modstate::locked_out(&cx, &a, &l1);
    // time and threads
    let tl = timeline::timeline(&cx);

    // L3, L3.5, L4
    let karma = karma::karma(&cx, &a, &l1, &l2);
    let newcomers = karma.as_ref().map_err(|p| *p).map(|k| {
        l2.p.iter()
            .filter(|d| !l2.s.contains(*d) && !k.e.contains(*d))
            .cloned()
            .collect::<BTreeSet<String>>()
    });
    let scores = scores::scores(&cx, &karma);
    let gates = gates::gates(&cx, &a, &karma);
    // L5
    let author = author::author_tier(&cx, &a, &l1, &karma);
    let mod_states = modstate::v1_v7(&cx, &v1_v5, &author, &locked_out);
    let mod_log = modlog::mod_log(&cx, &l1, &author);
    let mod_queue = modlog::mod_queue(&cx, &l1, &l2);
    let raw = modlog::raw(&cx, &l1);
    let reacts = reacts::reacts(&cx, &a, &karma, &gates);
    let wiki = wiki::wiki(&cx, &a, &l1, &v1_v5, &karma, &gates);
    let library = sequences::library(&cx, &l1, &v1_v5, &scores, &gates, &tl.times);
    // L6
    let visibility = visibility::visibility(&cx, &l2, &mod_states, &karma, &scores, &tl.threads);
    let orderings = ranking::orderings(
        &cx,
        query,
        &l1,
        &tl.times,
        &tl.threads,
        &visibility,
        &scores,
        &wiki,
        &author,
    );

    let mut banners = Vec::new();
    if newer_rules_banner {
        banners.push(Banner {
            code: "newerRules".into(),
            message: "this forum uses rules newer than your client".into(),
        });
    }
    if degraded {
        banners.push(Banner {
            code: "moderationUnavailable".into(),
            message: "moderation data unavailable".into(),
        });
    }

    Ok(ForumView {
        ns: cx.ns().to_owned(),
        forum: cx.forum().to_owned(),
        owner: cx.owner().to_owned(),
        fk: cx.fk().to_owned(),
        query: query.clone(),
        policy: cx.policy.clone(),
        newer_rules_banner,
        degraded,
        banners,
        input_digest,
        authorities: a,
        tiers,
        mod_queue,
        raw,
        participation: l2,
        t_decl: tl.t_decl,
        times: tl.times,
        threads: tl.threads,
        revisions: tl.revisions,
        versions: tl.versions,
        edited: tl.edited,
        excluded: cx.excluded.clone(),
        v1_v5,
        locked_out,
        mod_states,
        mod_log,
        moderation: l1,
        karma,
        newcomers,
        scores,
        gates,
        author,
        reacts,
        wiki,
        library,
        visibility,
        orderings,
    })
}
