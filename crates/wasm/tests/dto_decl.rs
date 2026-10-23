// SPDX-License-Identifier: AGPL-3.0-or-later
//! The dto/1 schema as TypeScript, generated from the Rust types (`engine-api.md` §5).
//!
//! `testdata/dto/dto-1.d.ts` holds the tsify declaration of every type in
//! `crates/core/src/dto.rs`, plus `EngineError`, `ErrorCode` and `AccountInfo`, doc comments
//! stripped, sorted by name. `packages/engine/src/types.parity.test.ts` compares the
//! hand-written `packages/engine/src/types.ts` with it: the same types, fields, optionality and
//! enum variants. This test fails when the file is not what the Rust types generate;
//!
//! ```text
//! LESSDUGIN_BLESS=1 cargo test -p lessdugin-wasm --features dto-decl --test dto_decl
//! ```
//!
//! rewrites it (then update `types.ts` until the vitest passes).
//!
//! tsify reads the serde attributes: a field is optional (`?:`) when the serialiser may omit it
//! (`skip_serializing_if = "Option::is_none"`) or the deserialiser may (`#[serde(default)]`),
//! so request types show what a caller may leave out.

use std::collections::BTreeSet;
use std::path::PathBuf;

use lessdugin_core::dto::*;
use lessdugin_core::engine::{AccountInfo, EngineError, ErrorCode};
use tsify::Tsify;

/// `(name, declaration)` of every listed type.
macro_rules! decls {
    ($($t:ident),* $(,)?) => {
        vec![$((stringify!($t), <$t as Tsify>::DECL)),*]
    };
}

/// Every DTO, in `dto.rs` order, then the engine types that cross the boundary.
fn all() -> Vec<(&'static str, &'static str)> {
    decls![
        // §4.1
        DtoVersion,
        HandleState,
        ActorView,
        CollapseReason,
        HiddenReason,
        ItemState,
        Badges,
        KarmaValue,
        AgreementValue,
        MyOpinion,
        ScoreView,
        Stance,
        TextQuote,
        Anchor,
        ReactTally,
        TocEntry,
        AspectRatio,
        MediaView,
        RenderedBody,
        Completeness,
        // §4.2
        PostKind,
        Tier,
        PostSort,
        PostFilter,
        CommentSort,
        ExternalLinkView,
        PostSummary,
        PostList,
        HistorySummary,
        SequenceNav,
        PostView,
        VersionRef,
        CommentNode,
        ThreadRoot,
        ThreadView,
        DiscussionItem,
        DiscussionList,
        VersionEntry,
        HistoryView,
        // §4.3
        MembershipMode,
        ModRole,
        ForumRole,
        ForumSummary,
        ModeratorView,
        Banner,
        EffectivePolicy,
        ViewerCaps,
        ForumView,
        KarmaView,
        LinkView,
        UserView,
        MemberView,
        TagSummary,
        TagView,
        SequenceSummary,
        ChapterView,
        SequenceView,
        SearchItem,
        SearchHit,
        SearchResults,
        NotificationReason,
        NotificationView,
        NotificationList,
        ModQueueItem,
        ModQueue,
        ModLogStatus,
        ModLogEntry,
        ModLog,
        TrustLevel,
        RawView,
        VerifyOutcome,
        VerifyCheck,
        VerifyReport,
        SyncIssue,
        SyncReport,
        AuthKind,
        AccountView,
        Ref,
        RefContext,
        // §3.5 writes, operation inputs
        WriteState,
        WriteTicket,
        ModeratorDraft,
        DefaultTier,
        ForumDraft,
        MembersQuery,
        SyncOpts,
        ListPostsQuery,
        ThreadOpts,
        MediaRef,
        BodyDraft,
        ExternalLinkDraft,
        PostDraft,
        CommentDraft,
        Draft,
        EditOpts,
        // M3 social writes and local state
        Capability,
        ModScope,
        AccessLevel,
        PinScope,
        PublicActionKind,
        OpinionIntent,
        ModActionValue,
        ModActionIntent,
        ModsChange,
        NoticeCode,
        ActionNotice,
        ActionTicket,
        NotificationsQuery,
        KarmaCadence,
        Subscription,
        SubscriptionEntry,
        BookmarkEntry,
        ForumUnread,
        UnreadCounts,
        // §3.8 events
        Topic,
        AuthState,
        WarningKind,
        Event,
        // the engine's own
        EngineError,
        ErrorCode,
        AccountInfo,
    ]
}

/// The type names `dto.rs` declares (`pub struct` / `pub enum` inside its macros).
fn dto_rs_types() -> BTreeSet<String> {
    let src = include_str!("../../core/src/dto.rs");
    src.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l
                .strip_prefix("pub struct ")
                .or_else(|| l.strip_prefix("pub enum "))?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// A declaration without its doc comments (the shapes only).
fn strip_docs(decl: &str) -> String {
    let mut out = String::new();
    for line in decl.lines() {
        let t = line.trim_start();
        if t.starts_with("/**") || t.starts_with("* ") || t == "*" || t.starts_with("*/") {
            continue;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim().to_owned()
}

fn generate() -> String {
    let mut decls = all();
    decls.sort_by_key(|(name, _)| *name);
    let mut out = String::from(
        "// SPDX-License-Identifier: AGPL-3.0-or-later\n\
         // GENERATED from the Rust DTOs (crates/core/src/dto.rs, plus EngineError, ErrorCode and\n\
         // AccountInfo) by their tsify derives; do not edit. Regenerate with\n\
         //   LESSDUGIN_BLESS=1 cargo test -p lessdugin-wasm --features dto-decl --test dto_decl\n\
         // packages/engine/src/types.parity.test.ts compares packages/engine/src/types.ts with it.\n\
         \n\
         export type Value = null | boolean | number | string | Value[] | { [key: string]: Value };\n",
    );
    for (name, decl) in decls {
        let d = strip_docs(decl);
        assert!(
            d.starts_with(&format!("export interface {name} "))
                || d.starts_with(&format!("export type {name} = ")),
            "unexpected declaration of {name}: {d}"
        );
        out.push('\n');
        out.push_str(&d);
        out.push('\n');
    }
    out
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/dto/dto-1.d.ts")
}

#[test]
fn every_dto_is_declared() {
    let listed: BTreeSet<String> = all().iter().map(|(n, _)| (*n).to_owned()).collect();
    let missing: Vec<_> = dto_rs_types().difference(&listed).cloned().collect();
    assert!(
        missing.is_empty(),
        "dto.rs types missing from tests/dto_decl.rs: {missing:?}"
    );
    assert_eq!(listed.len(), all().len(), "a type is listed twice");
}

#[test]
fn the_declarations_are_current() {
    let want = generate();
    let path = path();
    if std::env::var("LESSDUGIN_BLESS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, &want).expect("write");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{} is not what the Rust DTOs generate; run\n  LESSDUGIN_BLESS=1 cargo test -p \
         lessdugin-wasm --features dto-decl --test dto_decl\nthen update \
         packages/engine/src/types.ts until `pnpm --filter @lessdugin/engine test` passes",
        path.display()
    );
}
