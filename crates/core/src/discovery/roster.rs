// SPDX-License-Identifier: AGPL-3.0-or-later
//! The participant roster discovery needs (`social.md` §3.1, §4.1–§4.4, `mod/1` L0–L2), and the
//! closure (`sync.md` §4.3). Pure functions of the declaration, the stored records and `now`.
//!
//! ```text
//! A = {owner (rank 3)} ∪ admins (2) ∪ moderators (1)            // decl.moderators per FO2
//! assertion: valid modAction, m.forum = F, rkey ^FK-H16$ with FK = fk(F), known scope and action
//! counts (forum tier): scope_eff = forum, actor X ∈ A,
//!                      X = owner ∨ X = subject ∨ rank(X) > rank(subject), not lapsed
//! access(d)  = resolve(d, access):  the highest rank wins; at equal rank banned < member
//! nullify(d) = resolve(d, nullify): #restrict records assert nullifyVotes (default false);
//!              the highest rank wins; at equal rank true wins
//! G = {access = member}   B = {access = banned}   N = {nullify}   S = (A ∪ G) ∖ (B ∪ N)
//! member(d) ⇔ a valid membership record of d with rkey fk and forum = F
//! Closure = { did(u) : u ∈ refs(records of S in F) } ∖ (A ∪ G)
//!           refs: opinion → subject.uri; comment → root.uri, parent.uri
//! ```
//!
//! `#access` and `#restrict` are allowed for every role and are never positive (no CID binding).
//! This is the subset of the fold that decides *whose repos to sync*; the fold (`crate::social`)
//! recomputes it with everything else.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys::{self, ModActionKey};
use lessdugin_lexicon::records::{Action, Comment, Forum, Membership, ModAction, Opinion};
use lessdugin_lexicon::validity::entry;
use lessdugin_lexicon::values::{AccessLevel, MembershipMode, Role, Scope};
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;

/// The owner's rank.
pub const RANK_OWNER: u8 = 3;
/// An admin's rank.
pub const RANK_ADMIN: u8 = 2;
/// A moderator's rank.
pub const RANK_MODERATOR: u8 = 1;

/// A stored record offered to the roster: valid and fold-eligible (the caller filters).
#[derive(Clone, Debug, PartialEq)]
pub struct RosterRecord {
    /// The author.
    pub did: String,
    /// The collection NSID.
    pub collection: String,
    /// The rkey.
    pub rkey: String,
    /// The value (atproto JSON).
    pub value: Value,
}

/// A, with ranks: the owner and the effective `decl.moderators` entries (FO2).
pub fn authorities(owner: &Did, decl: &Forum) -> BTreeMap<String, u8> {
    let mut a = BTreeMap::from([(owner.to_string(), RANK_OWNER)]);
    for (m, role) in entry::moderators(decl, owner) {
        let rank = match role {
            Role::Admin => RANK_ADMIN,
            Role::Moderator => RANK_MODERATOR,
        };
        a.insert(m.did.to_string(), rank);
    }
    a
}

/// The roster of a forum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    /// The effective membership mode.
    pub mode: Option<MembershipMode>,
    /// A, with ranks.
    pub authorities: BTreeMap<String, u8>,
    /// G: `access = member`.
    pub granted: BTreeSet<String>,
    /// B: `access = banned`.
    pub banned: BTreeSet<String>,
    /// N: `nullify = true`.
    pub nullified: BTreeSet<String>,
    /// S = (A ∪ G) ∖ (B ∪ N).
    pub seeds: BTreeSet<String>,
    /// DIDs with a valid membership record of the forum.
    pub members: BTreeSet<String>,
}

#[derive(Default)]
struct Resolved {
    rank: u8,
    /// For access: `true` = banned seen at `rank`; for nullify: `true` seen at `rank`.
    safe: bool,
    any: bool,
}

impl Resolved {
    fn add(&mut self, rank: u8, safe: bool) {
        if !self.any || rank > self.rank {
            *self = Self {
                rank,
                safe,
                any: true,
            };
        } else if rank == self.rank {
            self.safe |= safe;
        }
    }
}

impl Roster {
    /// The roster of `forum` from its declaration and the stored records (modActions and
    /// memberships; other collections are ignored) at time `now_us`.
    pub fn compute(forum: &ForumKey, decl: &Forum, records: &[RosterRecord], now_us: i64) -> Self {
        let a = authorities(&forum.owner, decl);
        let rank = |d: &str| a.get(d).copied().unwrap_or(0);
        let owner = forum.owner.as_str();
        let mut access: BTreeMap<String, Resolved> = BTreeMap::new();
        let mut nullify: BTreeMap<String, Resolved> = BTreeMap::new();
        let mut members = BTreeSet::new();
        for r in records {
            if r.collection == nsid::MEMBERSHIP {
                if r.rkey == forum.fk
                    && serde_json::from_value::<Membership>(r.value.clone())
                        .is_ok_and(|m| m.forum == forum.uri)
                {
                    members.insert(r.did.clone());
                }
                continue;
            }
            if r.collection != nsid::MOD_ACTION {
                continue;
            }
            let rx = rank(&r.did);
            if rx == 0 {
                continue;
            }
            match keys::split_mod_action_key(&r.rkey) {
                Some(ModActionKey::Assertion { fk, .. }) if fk == forum.fk => {}
                _ => continue,
            }
            let Ok(m) = serde_json::from_value::<ModAction>(r.value.clone()) else {
                continue;
            };
            if m.forum != forum.uri || m.scope() != Some(Scope::Forum) {
                continue;
            }
            if m.until
                .as_ref()
                .is_some_and(|u| u.timestamp_micros() <= now_us)
            {
                continue;
            }
            let t = m.subject.as_str();
            if !t.starts_with("did:") {
                continue;
            }
            if !(r.did == owner || r.did == t || rx > rank(t)) {
                continue;
            }
            match &m.action {
                Action::Access(x) => match x.level() {
                    Some(level) => access
                        .entry(t.to_owned())
                        .or_default()
                        .add(rx, level == AccessLevel::Banned),
                    None => {}
                },
                Action::Restrict(x) => nullify
                    .entry(t.to_owned())
                    .or_default()
                    .add(rx, x.nullify_votes.unwrap_or(false)),
                _ => {}
            }
        }
        let mut granted = BTreeSet::new();
        let mut banned = BTreeSet::new();
        for (d, r) in access {
            if r.safe {
                banned.insert(d);
            } else {
                granted.insert(d);
            }
        }
        let nullified: BTreeSet<String> = nullify
            .into_iter()
            .filter(|(_, r)| r.safe)
            .map(|(d, _)| d)
            .collect();
        let seeds = a
            .keys()
            .chain(granted.iter())
            .filter(|d| !banned.contains(*d) && !nullified.contains(*d))
            .cloned()
            .collect();
        Self {
            mode: Some(decl.membership_mode().value),
            authorities: a,
            granted,
            banned,
            nullified,
            seeds,
            members,
        }
    }

    /// `true` in open mode.
    pub fn is_open(&self) -> bool {
        self.mode == Some(MembershipMode::Open)
    }

    /// A ∪ G.
    pub fn authorities_and_granted(&self) -> BTreeSet<String> {
        self.authorities
            .keys()
            .chain(self.granted.iter())
            .cloned()
            .collect()
    }

    /// The closure (§4.3): the DIDs referenced by the records of S in the forum (opinion
    /// subjects; comment roots and parents), minus A ∪ G. `records` are the forum's valid,
    /// eligible records.
    pub fn closure(&self, forum: &ForumKey, records: &[RosterRecord]) -> BTreeSet<String> {
        let ag = self.authorities_and_granted();
        let mut refs: Vec<AtUri> = Vec::new();
        for r in records {
            if !self.seeds.contains(&r.did) {
                continue;
            }
            if r.collection == nsid::OPINION {
                if let Ok(o) = serde_json::from_value::<Opinion>(r.value.clone())
                    && o.forum == forum.uri
                {
                    refs.push(o.subject.uri);
                }
            } else if r.collection == nsid::COMMENT
                && let Ok(c) = serde_json::from_value::<Comment>(r.value.clone())
                && c.forum == forum.uri
            {
                refs.push(c.root.uri);
                refs.extend(c.parent.map(|p| p.uri));
            }
        }
        refs.iter()
            .map(|u| u.did().to_string())
            .filter(|d| !ag.contains(d))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_atp::syntax::Datetime;
    use lessdugin_lexicon::records::{AccessAction, Moderator, RestrictAction};
    use serde_json::json;

    const OWNER: &str = "did:plc:owner2222222222222222222";
    const ADMIN: &str = "did:plc:admin2222222222222222222";
    const MODR: &str = "did:plc:moder2222222222222222222";

    fn key() -> ForumKey {
        ForumKey::new(&Did::parse(OWNER).unwrap(), "meta").unwrap()
    }

    fn decl(mode: MembershipMode) -> Forum {
        let mut f = Forum::create("Meta", Datetime::parse("2026-01-01T00:00:00Z").unwrap(), mode);
        let m = |d: &str, role: &str| Moderator {
            did: Did::parse(d).unwrap(),
            role: role.into(),
            title: None,
            extra: Default::default(),
        };
        f.moderators = Some(vec![
            m(ADMIN, "admin"),
            m(MODR, "moderator"),
            m(OWNER, "admin"),
            m("did:plc:weird2222222222222222222", "janitor"),
        ]);
        f
    }

    fn action(actor: &str, subject: &str, a: Action, until: Option<&str>) -> RosterRecord {
        let k = key();
        let mut m = ModAction::new(
            k.uri.clone(),
            subject,
            a,
            Datetime::parse("2026-01-01T00:00:00Z").unwrap(),
        );
        m.until = until.map(|u| Datetime::parse(u).unwrap());
        let h = keys::h16("modAction", &["forum", subject, "access"]).unwrap();
        RosterRecord {
            did: actor.into(),
            collection: nsid::MOD_ACTION.into(),
            rkey: format!("{}-{h}", k.fk),
            value: serde_json::to_value(&m).unwrap(),
        }
    }

    fn grant(actor: &str, subject: &str) -> RosterRecord {
        action(
            actor,
            subject,
            Action::Access(AccessAction::new(AccessLevel::Member)),
            None,
        )
    }

    fn ban(actor: &str, subject: &str) -> RosterRecord {
        action(
            actor,
            subject,
            Action::Access(AccessAction::new(AccessLevel::Banned)),
            None,
        )
    }

    fn membership(did: &str, forum: &AtUri, rkey: &str) -> RosterRecord {
        RosterRecord {
            did: did.into(),
            collection: nsid::MEMBERSHIP.into(),
            rkey: rkey.into(),
            value: json!({"$type": nsid::MEMBERSHIP, "forum": forum.as_str(),
                          "createdAt": "2026-01-01T00:00:00Z"}),
        }
    }

    #[test]
    fn ranks_follow_fo2() {
        let a = authorities(&Did::parse(OWNER).unwrap(), &decl(MembershipMode::Approval));
        assert_eq!(
            a,
            BTreeMap::from([
                (ADMIN.to_owned(), 2),
                (MODR.to_owned(), 1),
                (OWNER.to_owned(), 3)
            ])
        );
    }

    #[test]
    fn grants_bans_and_rank_rules() {
        let k = key();
        let d = decl(MembershipMode::Approval);
        let u = |n: u8| format!("did:plc:user{}{}", (b'a' + n) as char, "2".repeat(19));
        let now = Datetime::parse("2026-06-01T00:00:00Z").unwrap().timestamp_micros();
        let recs = vec![
            grant(MODR, &u(1)),
            grant(ADMIN, &u(2)),
            ban(MODR, &u(2)), // lower rank: the admin's grant wins
            grant(MODR, &u(3)),
            ban(MODR, &u(3)), // equal rank: banned wins
            grant("did:plc:rando2222222222222222222", &u(4)), // not an authority
            ban(MODR, ADMIN),  // a moderator cannot ban an admin
            ban(MODR, MODR),   // own DID: allowed
            action(
                OWNER,
                &u(5),
                Action::Access(AccessAction::new(AccessLevel::Member)),
                Some("2026-02-01T00:00:00Z"), // lapsed
            ),
            grant(OWNER, &u(6)),
            action(
                ADMIN,
                &u(6),
                Action::Restrict(RestrictAction::new(&[], true)),
                None,
            ),
            membership(&u(1), &k.uri, &k.fk),
            membership(&u(7), &k.uri, &k.fk),
            membership(&u(8), &k.uri, "zzzzzzzzzzzz"), // wrong rkey
        ];
        let r = Roster::compute(&k, &d, &recs, now);
        assert_eq!(r.mode, Some(MembershipMode::Approval));
        assert_eq!(r.granted, BTreeSet::from([u(1), u(2), u(6)]));
        assert_eq!(r.banned, BTreeSet::from([u(3), MODR.to_owned()]));
        assert_eq!(r.nullified, BTreeSet::from([u(6)]));
        assert_eq!(
            r.seeds,
            BTreeSet::from([ADMIN.to_owned(), OWNER.to_owned(), u(1), u(2)])
        );
        assert_eq!(r.members, BTreeSet::from([u(1), u(7)]));
        assert!(!r.is_open());
    }

    #[test]
    fn closure_is_one_level() {
        let k = key();
        let d = decl(MembershipMode::Open);
        let u = |n: u8| format!("did:plc:user{}{}", (b'a' + n) as char, "2".repeat(19));
        let cid = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifviqyqutqpnr7tvpya";
        let post = |did: &str| format!("at://{did}/{}/{}-3l2a2a2a2a2a2", nsid::POST, k.fk);
        let opinion = |author: &str, subject: &str| RosterRecord {
            did: author.into(),
            collection: nsid::OPINION.into(),
            rkey: "x".into(),
            value: json!({"$type": nsid::OPINION, "forum": k.uri.as_str(),
                          "subject": {"uri": subject, "cid": cid}, "karma": "up",
                          "createdAt": "2026-01-01T00:00:00Z"}),
        };
        let comment = |author: &str, root: &str, parent: Option<&str>| RosterRecord {
            did: author.into(),
            collection: nsid::COMMENT.into(),
            rkey: "y".into(),
            value: json!({"$type": nsid::COMMENT, "forum": k.uri.as_str(),
                          "root": {"uri": root, "cid": cid},
                          "parent": parent.map(|p| json!({"uri": p, "cid": cid})),
                          "body": {"text": "hi"}, "createdAt": "2026-01-01T00:00:00Z"}),
        };
        let recs = vec![
            grant(OWNER, &u(1)),
            opinion(OWNER, &post(&u(2))),
            opinion(&u(1), &post(&u(3))),
            opinion(&u(1), &post(ADMIN)), // an authority: not in the closure
            opinion(&u(2), &post(&u(9))), // u2 is not a seed: one level only
            comment(MODR, &post(&u(4)), Some(&post(&u(5)))),
        ];
        for x in &recs[1..] {
            if x.collection == nsid::OPINION {
                serde_json::from_value::<Opinion>(x.value.clone()).unwrap();
            } else {
                serde_json::from_value::<Comment>(x.value.clone()).unwrap();
            }
        }
        let r = Roster::compute(&k, &d, &recs, 0);
        assert!(r.is_open());
        assert_eq!(
            r.closure(&k, &recs),
            BTreeSet::from([u(2), u(3), u(4), u(5)])
        );
    }
}
