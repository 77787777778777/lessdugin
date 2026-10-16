// SPDX-License-Identifier: MIT OR Apache-2.0
//! Record-key derivation (`fk/1`, `sk/1`) and the per-collection rkey grammars
//! (`data-model.md` §3).
//!
//! ```text
//! b32s(bytes)    base32, MSB-first 5-bit groups, alphabet "234567abcdefghijklmnopqrstuvwxyz", no padding
//! H(tag, p1..pk) sha256(UTF8(tag) ‖ (0x1F ‖ UTF8(NFC(p_i)))*); rejects any p_i containing U+001F
//! fk(forumUri)   b32s(H("lessdugin-fk/1", forumUri))[0..12]
//! h16(name, p…)  b32s(H("lessdugin-sk/1", name, p…))[0..16]
//! threadTid, stid, P, actionKey and the rkey of every collection
//! ```
//!
//! **String level.** Every function works on strings and takes the namespace prefix `ns`
//! explicitly ([`crate::ns::NS`] for this build), so the shared vectors
//! (`testdata/vectors/keys/*.json`) run for both prefixes in every build. Derivation never checks
//! record validity (G4, G5, OP2, MA1–MA4): that is [`crate::validity`]'s job; at-uris are split
//! leniently (`at://` + exactly three `/`-separated segments), exactly as the reference generator
//! `gen_keys.py` does.
//!
//! **Grammars.** [`matches_grammar`] and the `split_*` functions implement the §3.2 rkey grammars
//! (`FK`, `H16`, `TID`, `SLUG` and their combinations).

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::collection::Collection;
use crate::limits::SLUG_MAX_BYTES;

/// The `b32s` alphabet; its order is the bit order (sortable).
pub const ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";
/// Hash domain tag of `fk/1` (unchanged under `.temp`).
pub const TAG_FK: &str = "lessdugin-fk/1";
/// Hash domain tag of `sk/1` (unchanged under `.temp`).
pub const TAG_SK: &str = "lessdugin-sk/1";
/// Length of an fk.
pub const FK_LEN: usize = 12;
/// Length of an h16.
pub const H16_LEN: usize = 16;
/// Length of a TID.
pub const TID_LEN: usize = 13;
/// The `stid` of opinions on wikiRevisions and sequences (sorts after every TID).
pub const STID_UNTHREADED: &str = "~";
/// The rkey of a profile.
pub const PROFILE_RKEY: &str = "self";

/// Why a derivation yields no key (the vector error codes of `data-model.md` §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum KeyError {
    /// A hash part contains U+001F, so the input is invalid.
    #[error("a hash part contains U+001F")]
    UnitSeparator,
    /// `threadTid`, `stid` or `P` is undefined for the input, a comment root is not an `NS.post`
    /// uri with key `^FK-TID$`, or the collection's key is not derived (`forum`).
    #[error("undefined for this input")]
    Undefined,
    /// A `tid` input is not TID syntax.
    #[error("not a TID")]
    InvalidTid,
}

impl KeyError {
    /// The vector error code (`"UnitSeparator"`, `"Undefined"`, `"InvalidTid"`).
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnitSeparator => "UnitSeparator",
            Self::Undefined => "Undefined",
            Self::InvalidTid => "InvalidTid",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Primitives

/// The first `n` characters of `b32s(bytes)`; `None` if `5n > 8·len` (only complete 5-bit groups
/// are ever used).
pub fn b32s(bytes: &[u8], n: usize) -> Option<String> {
    if n.checked_mul(5)? > bytes.len().checked_mul(8)? {
        return None;
    }
    let mut out = String::with_capacity(n);
    for i in 0..n {
        let bit = 5 * i;
        let (byte, off) = (bit / 8, bit % 8);
        let hi = u16::from(bytes[byte]) << 8;
        let lo = bytes.get(byte + 1).map_or(0, |b| u16::from(*b));
        let idx = ((hi | lo) >> (11 - off)) & 0x1f;
        out.push(char::from(ALPHABET[usize::from(idx)]));
    }
    Some(out)
}

/// The preimage of `H(tag, parts)`: `UTF8(tag)`, then `0x1F ‖ UTF8(NFC(p))` per part.
pub fn preimage(tag: &str, parts: &[&str]) -> Result<Vec<u8>, KeyError> {
    let mut buf = Vec::with_capacity(tag.len() + parts.iter().map(|p| p.len() + 1).sum::<usize>());
    buf.extend_from_slice(tag.as_bytes());
    for p in parts {
        if p.contains('\u{1f}') {
            return Err(KeyError::UnitSeparator);
        }
        buf.push(0x1f);
        if p.is_ascii() {
            buf.extend_from_slice(p.as_bytes()); // NFC is the identity on ASCII
        } else {
            buf.extend(p.nfc().collect::<String>().bytes());
        }
    }
    Ok(buf)
}

/// `H(tag, p1..pk)`: the sha256 of [`preimage`].
pub fn hash(tag: &str, parts: &[&str]) -> Result<[u8; 32], KeyError> {
    Ok(Sha256::digest(preimage(tag, parts)?).into())
}

/// `fk(forumUri)` (`fk/1`): 12 characters, 60 bits.
pub fn fk(forum_uri: &str) -> Result<String, KeyError> {
    let d = hash(TAG_FK, &[forum_uri])?;
    Ok(b32s(&d, FK_LEN).unwrap_or_default())
}

/// `h16(name, parts…)` (`sk/1`): 16 characters, 80 bits; the hashed parts are `[name, …parts]`.
/// `name` is the short collection name, `"opinion"` or `"modAction"`.
pub fn h16(name: &str, parts: &[&str]) -> Result<String, KeyError> {
    let mut all = Vec::with_capacity(parts.len() + 1);
    all.push(name);
    all.extend_from_slice(parts);
    let d = hash(TAG_SK, &all)?;
    Ok(b32s(&d, H16_LEN).unwrap_or_default())
}

/// The fk segment of an rkey of a forum-scoped collection (its first 12 characters).
pub fn fk_of(rkey: &str) -> Option<&str> {
    rkey.get(..FK_LEN)
}

// ---------------------------------------------------------------------------------------------
// Token grammars

fn is_b32(b: u8) -> bool {
    matches!(b, b'2'..=b'7' | b'a'..=b'z')
}

/// `FK = [2-7a-z]{12}`
pub fn is_fk(s: &str) -> bool {
    s.len() == FK_LEN && s.bytes().all(is_b32)
}

/// `H16 = [2-7a-z]{16}`
pub fn is_h16(s: &str) -> bool {
    s.len() == H16_LEN && s.bytes().all(is_b32)
}

/// `TID = [2-7a-j][2-7a-z]{12}` (atproto TID syntax).
pub fn is_tid(s: &str) -> bool {
    s.len() == TID_LEN
        && s.bytes().all(is_b32)
        && matches!(s.as_bytes()[0], b'2'..=b'7' | b'a'..=b'j')
}

/// `SLUG = [a-z0-9]+(-[a-z0-9]+)*`, at most 64 bytes (`data-model.md` §2.1).
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= SLUG_MAX_BYTES
        && s.split('-').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `^FK-TID$` (post, wikiRevision, sequence; reserved modAction form): `(fk, tid)`.
pub fn split_fk_tid(rkey: &str) -> Option<(&str, &str)> {
    let (a, b) = rkey.split_once('-')?;
    (is_fk(a) && is_tid(b)).then_some((a, b))
}

/// `^FK-TID-TID$` (comment, `{fk}-{rootTid}-{tid}`): `(fk, rootTid, tid)`.
pub fn split_comment_key(rkey: &str) -> Option<(&str, &str, &str)> {
    let mut it = rkey.split('-');
    let (a, b, c) = (it.next()?, it.next()?, it.next()?);
    (it.next().is_none() && is_fk(a) && is_tid(b) && is_tid(c)).then_some((a, b, c))
}

/// `^FK-(TID|~)-H16$` (opinion): `(fk, stid, h16)`.
pub fn split_opinion_key(rkey: &str) -> Option<(&str, &str, &str)> {
    let mut it = rkey.split('-');
    let (a, b, c) = (it.next()?, it.next()?, it.next()?);
    (it.next().is_none() && is_fk(a) && (b == STID_UNTHREADED || is_tid(b)) && is_h16(c))
        .then_some((a, b, c))
}

/// The two modAction key forms (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModActionKey<'a> {
    /// `^FK-H16$`: a v1 assertion.
    Assertion {
        /// The fk segment.
        fk: &'a str,
        /// The h16 segment.
        h16: &'a str,
    },
    /// `^FK-TID$`: reserved for P2 list actions; such records are ignored in v1.
    Reserved {
        /// The fk segment.
        fk: &'a str,
        /// The TID segment.
        tid: &'a str,
    },
}

/// Classifies a modAction rkey.
pub fn split_mod_action_key(rkey: &str) -> Option<ModActionKey<'_>> {
    let (a, b) = rkey.split_once('-')?;
    if !is_fk(a) {
        return None;
    }
    if is_h16(b) {
        Some(ModActionKey::Assertion { fk: a, h16: b })
    } else if is_tid(b) {
        Some(ModActionKey::Reserved { fk: a, tid: b })
    } else {
        None
    }
}

/// `^P-TID$` (revision): `(P, tid)`, where `P` is the key prefix of some revisable subject:
/// `FK` (a forum), `FK-TID` (a post, a sequence or a reserved-form modAction), `FK-TID-TID` (a
/// comment) or `FK-H16` (a modAction). The exact `P` is checked against the subject (RV3).
pub fn split_revision_key(rkey: &str) -> Option<(&str, &str)> {
    let (p, tid) = rkey.rsplit_once('-')?;
    if !is_tid(tid) {
        return None;
    }
    let ok = is_fk(p)
        || split_fk_tid(p).is_some()
        || split_comment_key(p).is_some()
        || matches!(
            split_mod_action_key(p),
            Some(ModActionKey::Assertion { .. })
        );
    ok.then_some((p, tid))
}

/// `true` if `rkey` matches the §3.2 grammar of `collection` (for a modAction, either form).
pub fn matches_grammar(collection: Collection, rkey: &str) -> bool {
    match collection {
        Collection::Forum => is_slug(rkey),
        Collection::Membership => is_fk(rkey),
        Collection::Post | Collection::WikiRevision | Collection::Sequence => {
            split_fk_tid(rkey).is_some()
        }
        Collection::Comment => split_comment_key(rkey).is_some(),
        Collection::Revision => split_revision_key(rkey).is_some(),
        Collection::Opinion => split_opinion_key(rkey).is_some(),
        Collection::ModAction => split_mod_action_key(rkey).is_some(),
        Collection::Profile => rkey == PROFILE_RKEY,
    }
}

// ---------------------------------------------------------------------------------------------
// Structural functions (string level, explicit prefix)

/// Splits `at://A/C/K` into `(A, C, K)`; `Undefined` unless it has exactly that shape.
pub fn split_uri(uri: &str) -> Result<(&str, &str, &str), KeyError> {
    let rest = uri.strip_prefix("at://").ok_or(KeyError::Undefined)?;
    let mut it = rest.split('/');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(a), Some(c), Some(k), None) => Ok((a, c, k)),
        _ => Err(KeyError::Undefined),
    }
}

fn coll_in(ns: &str, nsid: &str) -> Option<Collection> {
    Collection::from_nsid_in(ns, nsid)
}

/// `threadTid(uri)`: the TID of a post key `^FK-TID$`, or the first TID of a comment key
/// `^FK-TID-TID$`; undefined otherwise.
pub fn thread_tid<'u>(ns: &str, uri: &'u str) -> Result<&'u str, KeyError> {
    let (_, c, k) = split_uri(uri)?;
    match coll_in(ns, c) {
        Some(Collection::Post) => split_fk_tid(k).map(|(_, t)| t),
        Some(Collection::Comment) => split_comment_key(k).map(|(_, t, _)| t),
        _ => None,
    }
    .ok_or(KeyError::Undefined)
}

/// `stid(subject)`: `threadTid` for posts and comments, `"~"` for wikiRevisions and sequences.
pub fn stid<'u>(ns: &str, subject_uri: &'u str) -> Result<&'u str, KeyError> {
    let (_, c, _) = split_uri(subject_uri)?;
    match coll_in(ns, c) {
        Some(Collection::Post | Collection::Comment) => thread_tid(ns, subject_uri),
        Some(Collection::WikiRevision | Collection::Sequence) => Ok(STID_UNTHREADED),
        _ => Err(KeyError::Undefined),
    }
}

/// `P(subject)`, the revision key prefix: `fk(uri)` for a forum, the rkey for a post, comment,
/// sequence or modAction; undefined otherwise.
pub fn revision_prefix(ns: &str, subject_uri: &str) -> Result<String, KeyError> {
    let (_, c, k) = split_uri(subject_uri)?;
    match coll_in(ns, c) {
        Some(Collection::Forum) => fk(subject_uri),
        Some(
            Collection::Post | Collection::Comment | Collection::Sequence | Collection::ModAction,
        ) => Ok(k.to_owned()),
        _ => Err(KeyError::Undefined),
    }
}

/// `actionKey(action)`: the def name for `#hide`, `#access`, `#restrict`, `#tier`, `#lock` and
/// `#tag` of `ns`, `"pin:" + scope` for `#pin`, and the `$type` string itself for anything else
/// (an unknown or foreign action). `Undefined` if `$type` (or a `#pin`'s `scope`) is not a string.
pub fn action_key(ns: &str, action: &Map<String, Value>) -> Result<String, KeyError> {
    let t = action
        .get("$type")
        .and_then(Value::as_str)
        .ok_or(KeyError::Undefined)?;
    let name = t
        .strip_prefix(ns)
        .and_then(|r| r.strip_prefix(".modAction#"));
    Ok(match name {
        Some(n @ ("hide" | "access" | "restrict" | "tier" | "lock" | "tag")) => n.to_owned(),
        Some("pin") => {
            let scope = action
                .get("scope")
                .and_then(Value::as_str)
                .ok_or(KeyError::Undefined)?;
            format!("pin:{scope}")
        }
        _ => t.to_owned(),
    })
}

fn check_tid(tid: &str) -> Result<&str, KeyError> {
    if is_tid(tid) {
        Ok(tid)
    } else {
        Err(KeyError::InvalidTid)
    }
}

// ---------------------------------------------------------------------------------------------
// Per-collection rkeys

/// The inputs of an rkey derivation (§3.2), as strings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RkeyInput<'a> {
    /// A forum's key is its slug: nothing to derive (`Undefined`).
    Forum,
    /// `membership`: `fk(forum)`.
    Membership {
        /// The forum at-uri.
        forum: &'a str,
    },
    /// `post`: `fk(forum) + "-" + tid`.
    Post {
        /// The forum at-uri.
        forum: &'a str,
        /// The post's TID.
        tid: &'a str,
    },
    /// `comment`: `fk(forum) + "-" + threadTid(root) + "-" + tid`.
    Comment {
        /// The forum at-uri.
        forum: &'a str,
        /// The root post's at-uri.
        root: &'a str,
        /// The comment's TID.
        tid: &'a str,
    },
    /// `revision`: `P(subject) + "-" + tid`.
    Revision {
        /// The subject's at-uri.
        subject: &'a str,
        /// The revision's TID.
        tid: &'a str,
    },
    /// `opinion`: `fk(forum) + "-" + stid(subject) + "-" + h16("opinion", subject)`.
    Opinion {
        /// The forum at-uri.
        forum: &'a str,
        /// The subject's at-uri.
        subject: &'a str,
    },
    /// `modAction`: `fk(forum) + "-" + h16("modAction", scope_eff, subject, actionKey(action))`.
    ModAction {
        /// The forum at-uri.
        forum: &'a str,
        /// The `scope` field (`None` = absent = `"forum"`).
        scope: Option<&'a str>,
        /// The `subject` field (did, at-uri or `tag:` + slug).
        subject: &'a str,
        /// The `action` object (its `$type`, and `scope` for `#pin`).
        action: &'a Map<String, Value>,
    },
    /// `wikiRevision`: `fk(forum) + "-" + tid`.
    WikiRevision {
        /// The forum at-uri.
        forum: &'a str,
        /// The revision's TID.
        tid: &'a str,
    },
    /// `sequence`: `fk(forum) + "-" + tid`.
    Sequence {
        /// The forum at-uri.
        forum: &'a str,
        /// The sequence's TID.
        tid: &'a str,
    },
    /// `profile`: `"self"`.
    Profile,
}

/// An rkey and the intermediates of its derivation (the vectors' `expected` fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derivation {
    /// The canonical rkey.
    pub rkey: String,
    /// `fk(forum)`.
    pub fk: Option<String>,
    /// `threadTid(root)` (comments).
    pub thread_tid: Option<String>,
    /// `stid(subject)` (opinions).
    pub stid: Option<String>,
    /// The h16 (opinions, modActions).
    pub h16: Option<String>,
    /// `P(subject)` (revisions).
    pub p: Option<String>,
    /// `scope_eff` (modActions).
    pub scope_eff: Option<String>,
    /// `actionKey(action)` (modActions).
    pub action_key: Option<String>,
}

/// Derives the canonical rkey for `input` under the prefix `ns`.
pub fn derive(ns: &str, input: &RkeyInput<'_>) -> Result<Derivation, KeyError> {
    let mut d = Derivation::default();
    match *input {
        RkeyInput::Forum => return Err(KeyError::Undefined),
        RkeyInput::Profile => d.rkey = PROFILE_RKEY.to_owned(),
        RkeyInput::Membership { forum } => {
            let f = fk(forum)?;
            d.rkey = f.clone();
            d.fk = Some(f);
        }
        RkeyInput::Post { forum, tid }
        | RkeyInput::WikiRevision { forum, tid }
        | RkeyInput::Sequence { forum, tid } => {
            let f = fk(forum)?;
            d.rkey = format!("{f}-{}", check_tid(tid)?);
            d.fk = Some(f);
        }
        RkeyInput::Comment { forum, root, tid } => {
            let (_, rc, rk) = split_uri(root)?;
            if coll_in(ns, rc) != Some(Collection::Post) || split_fk_tid(rk).is_none() {
                return Err(KeyError::Undefined);
            }
            let f = fk(forum)?;
            let t = thread_tid(ns, root)?;
            d.rkey = format!("{f}-{t}-{}", check_tid(tid)?);
            d.fk = Some(f);
            d.thread_tid = Some(t.to_owned());
        }
        RkeyInput::Revision { subject, tid } => {
            let p = revision_prefix(ns, subject)?;
            d.rkey = format!("{p}-{}", check_tid(tid)?);
            d.p = Some(p);
        }
        RkeyInput::Opinion { forum, subject } => {
            let f = fk(forum)?;
            let s = stid(ns, subject)?;
            let h = h16("opinion", &[subject])?;
            d.rkey = format!("{f}-{s}-{h}");
            d.fk = Some(f);
            d.stid = Some(s.to_owned());
            d.h16 = Some(h);
        }
        RkeyInput::ModAction {
            forum,
            scope,
            subject,
            action,
        } => {
            let scope_eff = scope.unwrap_or("forum");
            let f = fk(forum)?;
            let ak = action_key(ns, action)?;
            let h = h16("modAction", &[scope_eff, subject, &ak])?;
            d.rkey = format!("{f}-{h}");
            d.fk = Some(f);
            d.scope_eff = Some(scope_eff.to_owned());
            d.action_key = Some(ak);
            d.h16 = Some(h);
        }
    }
    Ok(d)
}

/// `membership` rkey: `fk(forum)`.
pub fn membership_rkey(forum: &str) -> Result<String, KeyError> {
    fk(forum)
}

/// `post` rkey: `{fk}-{tid}`.
pub fn post_rkey(forum: &str, tid: &str) -> Result<String, KeyError> {
    Ok(format!("{}-{}", fk(forum)?, check_tid(tid)?))
}

/// `wikiRevision` rkey: `{fk}-{tid}`.
pub fn wiki_revision_rkey(forum: &str, tid: &str) -> Result<String, KeyError> {
    post_rkey(forum, tid)
}

/// `sequence` rkey: `{fk}-{tid}`.
pub fn sequence_rkey(forum: &str, tid: &str) -> Result<String, KeyError> {
    post_rkey(forum, tid)
}

/// `comment` rkey: `{fk}-{rootTid}-{tid}`.
pub fn comment_rkey(ns: &str, forum: &str, root: &str, tid: &str) -> Result<String, KeyError> {
    derive(ns, &RkeyInput::Comment { forum, root, tid }).map(|d| d.rkey)
}

/// `revision` rkey: `{P}-{tid}`.
pub fn revision_rkey(ns: &str, subject: &str, tid: &str) -> Result<String, KeyError> {
    derive(ns, &RkeyInput::Revision { subject, tid }).map(|d| d.rkey)
}

/// `opinion` rkey: `{fk}-{stid}-{h16}`.
pub fn opinion_rkey(ns: &str, forum: &str, subject: &str) -> Result<String, KeyError> {
    derive(ns, &RkeyInput::Opinion { forum, subject }).map(|d| d.rkey)
}

/// `modAction` rkey: `{fk}-{h16}`.
pub fn mod_action_rkey(
    ns: &str,
    forum: &str,
    scope: Option<&str>,
    subject: &str,
    action: &Map<String, Value>,
) -> Result<String, KeyError> {
    derive(
        ns,
        &RkeyInput::ModAction {
            forum,
            scope,
            subject,
            action,
        },
    )
    .map(|d| d.rkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::NS_FINAL;
    use serde_json::json;

    const FORUM: &str =
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.forum/rationality";
    const POST: &str = "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2ze2c";

    #[test]
    fn worked_example() {
        // data-model.md §3.4
        assert_eq!(fk(FORUM).unwrap(), "uetjixxr3a33");
        assert_eq!(
            fk("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.temp.forum/rationality")
                .unwrap(),
            "2xbiijsnjtau"
        );
        assert_eq!(
            post_rkey(FORUM, "3l2s5xxv2ze2c").unwrap(),
            "uetjixxr3a33-3l2s5xxv2ze2c"
        );
        assert_eq!(h16("opinion", &[POST]).unwrap(), "wcz54x7liy3jsh3g");
        assert_eq!(
            opinion_rkey(NS_FINAL, FORUM, POST).unwrap(),
            "uetjixxr3a33-3l2s5xxv2ze2c-wcz54x7liy3jsh3g"
        );
        let comment = comment_rkey(NS_FINAL, FORUM, POST, "3l2s6aaaaaaa2").unwrap();
        assert_eq!(comment, "uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2");
        let c_uri = format!(
            "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.comment/{comment}"
        );
        assert_eq!(
            opinion_rkey(NS_FINAL, FORUM, &c_uri).unwrap(),
            "uetjixxr3a33-3l2s5xxv2ze2c-3mjfititlivah7tw"
        );
        let a = |t: &str| json!({"$type": format!("online.7778777.lessdugin.modAction#{t}"), "scope": "forum"});
        let ma = |s: &str, act: &Value| {
            mod_action_rkey(NS_FINAL, FORUM, None, s, act.as_object().unwrap()).unwrap()
        };
        assert_eq!(
            ma("did:plc:abcdefghijklmnopqrstuvwx", &a("access")),
            "uetjixxr3a33-ckjbz7a2pjeplbao"
        );
        assert_eq!(ma(POST, &a("hide")), "uetjixxr3a33-jmsm5bwwzljcu2zo");
        assert_eq!(ma(POST, &a("pin")), "uetjixxr3a33-4vdjzbg6ohwpa323");
        assert_eq!(
            ma("tag:decision-theory", &a("tag")),
            "uetjixxr3a33-yext7l62fmg5ufrt"
        );
    }

    #[test]
    fn b32s_basics() {
        assert_eq!(b32s(&[], 0).unwrap(), "");
        assert_eq!(b32s(&[0; 5], 8).unwrap(), "22222222");
        assert_eq!(b32s(&[0xff; 5], 8).unwrap(), "zzzzzzzz");
        assert_eq!(b32s(&[0x80, 0, 0, 0, 0], 1).unwrap(), "k");
        assert!(b32s(&[0; 5], 9).is_none());
        assert!(b32s(&[0; 32], 52).is_none());
        assert_eq!(b32s(&[0; 32], 51).unwrap().len(), 51);
    }

    #[test]
    fn grammars() {
        assert!(is_fk("uetjixxr3a33"));
        assert!(!is_fk("uetjixxr3a3"));
        assert!(!is_fk("uetjixxr3a31"));
        assert!(is_tid("3l2s5xxv2ze2c"));
        assert!(!is_tid("zl2s5xxv2ze2c"));
        assert!(!is_tid("3L2S5XXV2ZE2C"));
        assert!(is_slug("a"));
        assert!(is_slug("2024-ai-safety-camp"));
        for bad in ["", "-a", "a-", "a--b", "A", "a_b", "é"] {
            assert!(!is_slug(bad), "{bad}");
        }
        assert!(!is_slug(&"a".repeat(65)));
        assert!(is_slug(&"a".repeat(64)));
        assert!(matches_grammar(
            Collection::Post,
            "uetjixxr3a33-3l2s5xxv2ze2c"
        ));
        assert!(!matches_grammar(
            Collection::Post,
            "uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2"
        ));
        assert!(matches_grammar(
            Collection::Comment,
            "uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2"
        ));
        assert!(matches_grammar(
            Collection::Opinion,
            "uetjixxr3a33-~-wcz54x7liy3jsh3g"
        ));
        assert!(matches_grammar(
            Collection::ModAction,
            "uetjixxr3a33-jmsm5bwwzljcu2zo"
        ));
        assert!(matches_grammar(
            Collection::ModAction,
            "uetjixxr3a33-3l2s5xxv2ze2c"
        ));
        assert!(matches_grammar(
            Collection::Revision,
            "uetjixxr3a33-3l2s5xxv2ze2c"
        ));
        assert!(matches_grammar(
            Collection::Revision,
            "uetjixxr3a33-jmsm5bwwzljcu2zo-3l2s5xxv2ze2c"
        ));
        assert!(matches_grammar(
            Collection::Revision,
            "uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2-3l2s7bbbbbbb2"
        ));
        assert!(!matches_grammar(Collection::Revision, "3l2s5xxv2ze2c"));
        assert!(matches_grammar(Collection::Profile, "self"));
        assert!(matches_grammar(Collection::Forum, "rationality"));
        assert!(matches_grammar(Collection::Membership, "uetjixxr3a33"));
    }

    #[test]
    fn errors() {
        assert_eq!(fk("a\u{1f}b"), Err(KeyError::UnitSeparator));
        assert_eq!(post_rkey(FORUM, "3l2s5xxv2ze2"), Err(KeyError::InvalidTid));
        assert_eq!(thread_tid(NS_FINAL, FORUM), Err(KeyError::Undefined));
        assert_eq!(stid(NS_FINAL, FORUM), Err(KeyError::Undefined));
        assert_eq!(
            revision_prefix(NS_FINAL, "not a uri"),
            Err(KeyError::Undefined)
        );
        assert_eq!(
            derive(NS_FINAL, &RkeyInput::Forum),
            Err(KeyError::Undefined)
        );
        assert_eq!(
            action_key(NS_FINAL, json!({"x": 1}).as_object().unwrap()),
            Err(KeyError::Undefined)
        );
        assert_eq!(KeyError::InvalidTid.code(), "InvalidTid");
    }
}
