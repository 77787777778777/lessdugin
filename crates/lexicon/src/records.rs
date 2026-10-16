// SPDX-License-Identifier: MIT OR Apache-2.0
//! Owned serde structs for every lessdugin record type and shared definition
//! (`data-model.md` §4–§6).
//!
//! **Writer rules (§2.5, §8.1).**
//! - Every struct has `#[serde(flatten)] pub extra: Map<String, Value>`: fields this build does not
//!   know survive a read-modify-write (W3), at every nesting level.
//! - Records and union members carry `$type` as the `r#type` field. Deserialising keeps whatever
//!   was read; when it is absent it defaults to this build's NSID ([`crate::collection::nsid`]),
//!   and serialising always writes it (W1).
//! - Optional fields are `Option`s skipped when `None`, and arrays are `Option<Vec<_>>`, so an
//!   absent field and an empty array round-trip exactly (the CID does not change).
//! - Known-value (`kv`) fields are `String`s, so unknown values round-trip; read them through
//!   [`crate::values`] (for example [`Post::kind`]).
//! - Open unions ([`OpenUnion`], [`Action`]) keep unknown members verbatim.
//! - Integers accept the data model's integral floats (`3.0`) on read and are written as integers.
//!
//! These structs are for **valid** records: a record that fails lexicon validation (G2) or the
//! canonical-reference rule (G4) may fail to deserialise. Check [`crate::validity`] first, and use
//! the raw JSON for the raw view.

mod defs;
mod forum;
mod mod_action;
mod opinion;
mod post;
mod profile;
mod revision;
mod sequence;

pub use defs::{
    AspectRatio, Blob, CidLink, ExternalLink, Labels, Link, Markdown, Md, Media, SelfLabel,
    SelfLabels, StrongRef, TextQuote,
};
pub use forum::{
    Discovery, DisplayParams, Endpoint, Forum, Gates, KarmaParams, Membership, Moderator, Policy,
    RankParams,
};
pub use mod_action::{
    AccessAction, Action, HideAction, LockAction, ModAction, PinAction, RestrictAction, TagAction,
    TierAction,
};
pub use opinion::{Opinion, OpinionIntent, React, TagVote, qkey};
pub use post::{Comment, Post};
pub use profile::Profile;
pub use revision::Revision;
pub use sequence::{Chapter, Sequence, WikiRevision};

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::collection::{Collection, type_matches};

/// Serde helpers for integer fields: accept JSON integers and integral floats within
/// ±(2^53 − 1), as the data model does.
pub(crate) mod int {
    use serde::Deserializer;
    use serde::de::{self, Visitor};

    struct V;

    impl Visitor<'_> for V {
        type Value = i64;

        fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("an integer")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            i64::try_from(v).map_err(|_| E::custom("integer out of the signed 64-bit range"))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<i64, E> {
            if v.is_finite() && v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0 {
                Ok(v as i64)
            } else {
                Err(E::custom("expected an integer, found a float"))
            }
        }
    }

    pub fn de<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        d.deserialize_any(V)
    }

    pub fn de_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        de(d).map(Some)
    }
}

/// The `$type` of a known union member.
pub trait UnionMember {
    /// The member's reference (build NSID, `#main` omitted).
    const TYPE: &'static str;
}

/// An open union with one known member type `T` (`data-model.md` §2.4: all unions are open).
///
/// A value whose `$type` names `T` deserialises as [`OpenUnion::Known`]; any other value (an
/// unknown member, a member of the other namespace prefix, or one without `$type`) is kept
/// verbatim as [`OpenUnion::Unknown`] and written back unchanged (W3).
#[derive(Debug, Clone, PartialEq)]
pub enum OpenUnion<T> {
    /// The known member.
    Known(T),
    /// Any other member, verbatim.
    Unknown(Map<String, Value>),
}

impl<T> OpenUnion<T> {
    /// The known member, if this is one.
    pub fn known(&self) -> Option<&T> {
        match self {
            Self::Known(t) => Some(t),
            Self::Unknown(_) => None,
        }
    }

    /// The known member, mutably.
    pub fn known_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Known(t) => Some(t),
            Self::Unknown(_) => None,
        }
    }
}

impl<T: Serialize> Serialize for OpenUnion<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(t) => t.serialize(s),
            Self::Unknown(m) => m.serialize(s),
        }
    }
}

impl<'de, T: DeserializeOwned + UnionMember> Deserialize<'de> for OpenUnion<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m = Map::<String, Value>::deserialize(d)?;
        match m.get("$type").and_then(Value::as_str) {
            Some(t) if type_matches(t, T::TYPE) => serde_json::from_value(Value::Object(m))
                .map(Self::Known)
                .map_err(D::Error::custom),
            _ => Ok(Self::Unknown(m)),
        }
    }
}

/// Any lessdugin record, dispatched on its collection. Variants are boxed: record structs are
/// large (a forum declaration is over a kilobyte) and differ widely in size.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    /// `forum`
    Forum(Box<Forum>),
    /// `membership`
    Membership(Box<Membership>),
    /// `post`
    Post(Box<Post>),
    /// `comment`
    Comment(Box<Comment>),
    /// `revision`
    Revision(Box<Revision>),
    /// `opinion`
    Opinion(Box<Opinion>),
    /// `modAction`
    ModAction(Box<ModAction>),
    /// `wikiRevision`
    WikiRevision(Box<WikiRevision>),
    /// `sequence`
    Sequence(Box<Sequence>),
    /// `profile`
    Profile(Box<Profile>),
}

impl Record {
    /// Deserialises a record value of `collection`.
    pub fn from_value(collection: Collection, value: Value) -> Result<Self, serde_json::Error> {
        fn f<T: DeserializeOwned>(v: Value) -> Result<Box<T>, serde_json::Error> {
            serde_json::from_value(v).map(Box::new)
        }
        Ok(match collection {
            Collection::Forum => Self::Forum(f(value)?),
            Collection::Membership => Self::Membership(f(value)?),
            Collection::Post => Self::Post(f(value)?),
            Collection::Comment => Self::Comment(f(value)?),
            Collection::Revision => Self::Revision(f(value)?),
            Collection::Opinion => Self::Opinion(f(value)?),
            Collection::ModAction => Self::ModAction(f(value)?),
            Collection::WikiRevision => Self::WikiRevision(f(value)?),
            Collection::Sequence => Self::Sequence(f(value)?),
            Collection::Profile => Self::Profile(f(value)?),
        })
    }

    /// Deserialises a record of the build-namespace collection `nsid`; `None` for an NSID that is
    /// not one of this build's collections.
    pub fn from_nsid_value(nsid: &str, value: Value) -> Option<Result<Self, serde_json::Error>> {
        Collection::from_nsid(nsid).map(|c| Self::from_value(c, value))
    }

    /// The record's collection.
    pub fn collection(&self) -> Collection {
        match self {
            Self::Forum(_) => Collection::Forum,
            Self::Membership(_) => Collection::Membership,
            Self::Post(_) => Collection::Post,
            Self::Comment(_) => Collection::Comment,
            Self::Revision(_) => Collection::Revision,
            Self::Opinion(_) => Collection::Opinion,
            Self::ModAction(_) => Collection::ModAction,
            Self::WikiRevision(_) => Collection::WikiRevision,
            Self::Sequence(_) => Collection::Sequence,
            Self::Profile(_) => Collection::Profile,
        }
    }

    /// The record's `$type` as held.
    pub fn r#type(&self) -> &str {
        match self {
            Self::Forum(r) => &r.r#type,
            Self::Membership(r) => &r.r#type,
            Self::Post(r) => &r.r#type,
            Self::Comment(r) => &r.r#type,
            Self::Revision(r) => &r.r#type,
            Self::Opinion(r) => &r.r#type,
            Self::ModAction(r) => &r.r#type,
            Self::WikiRevision(r) => &r.r#type,
            Self::Sequence(r) => &r.r#type,
            Self::Profile(r) => &r.r#type,
        }
    }

    /// The record's unknown top-level fields.
    pub fn extra(&self) -> &Map<String, Value> {
        match self {
            Self::Forum(r) => &r.extra,
            Self::Membership(r) => &r.extra,
            Self::Post(r) => &r.extra,
            Self::Comment(r) => &r.extra,
            Self::Revision(r) => &r.extra,
            Self::Opinion(r) => &r.extra,
            Self::ModAction(r) => &r.extra,
            Self::WikiRevision(r) => &r.extra,
            Self::Sequence(r) => &r.extra,
            Self::Profile(r) => &r.extra,
        }
    }

    /// Serialises the record to JSON.
    pub fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

impl Serialize for Record {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Forum(r) => r.serialize(s),
            Self::Membership(r) => r.serialize(s),
            Self::Post(r) => r.serialize(s),
            Self::Comment(r) => r.serialize(s),
            Self::Revision(r) => r.serialize(s),
            Self::Opinion(r) => r.serialize(s),
            Self::ModAction(r) => r.serialize(s),
            Self::WikiRevision(r) => r.serialize(s),
            Self::Sequence(r) => r.serialize(s),
            Self::Profile(r) => r.serialize(s),
        }
    }
}

macro_rules! record_from {
    ($($v:ident),*) => {$(
        impl From<$v> for Record {
            fn from(r: $v) -> Self {
                Self::$v(Box::new(r))
            }
        }
    )*};
}

record_from!(
    Forum,
    Membership,
    Post,
    Comment,
    Revision,
    Opinion,
    ModAction,
    WikiRevision,
    Sequence,
    Profile
);
