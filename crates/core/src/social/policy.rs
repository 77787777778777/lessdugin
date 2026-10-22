// SPDX-License-Identifier: AGPL-3.0-or-later
//! The effective policy of a forum (`social.md` §1.6, `data-model.md` §6.1, §13.3): every
//! `PARAMS` value with its lexicon default (absent) or fallback (unknown known value), the
//! effective behaviour IDs, and whether the "rules newer than your client" banner is due.

use lessdugin_lexicon::records::{Forum, Policy};
use lessdugin_lexicon::values::{
    DefaultTier, KarmaAlgorithm, MembershipMode, ModerationPolicy, NewUsers, RankingAlgorithm,
    ReactPalette, TagCreation, Voting,
};

use crate::dto;

/// The names of the `params` line of `inputDigest`, in order (`social.md` §1.2).
pub const PARAMS: [&str; 23] = [
    "membership",
    "defaultTier",
    "voting",
    "authorModeration",
    "tagCreation",
    "pairCap",
    "establish",
    "establishSeeds",
    "offsetHours",
    "exponentMilli",
    "curatedBonus",
    "frontpageBonus",
    "cutoffDays",
    "reactSecond",
    "reactCreate",
    "reactAnti",
    "authorModPersonal",
    "authorModFrontpage",
    "createTag",
    "wikiEdit",
    "sequence",
    "collapseBelow",
    "newUsers",
];

/// Effective policy values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectivePolicy {
    /// `policy.moderation` (behaviour ID).
    pub moderation: ModerationPolicy,
    /// `policy.karma.algorithm`.
    pub karma: KarmaAlgorithm,
    /// `policy.ranking.algorithm`.
    pub ranking: RankingAlgorithm,
    /// `policy.reactPalette`.
    pub react_palette: ReactPalette,
    /// `membership`.
    pub membership: MembershipMode,
    /// `defaultTier`.
    pub default_tier: DefaultTier,
    /// `policy.voting`.
    pub voting: Voting,
    /// `policy.authorModeration`.
    pub author_moderation: bool,
    /// `policy.tagCreation`.
    pub tag_creation: TagCreation,
    /// `policy.karma.pairCap`.
    pub pair_cap: i64,
    /// `policy.karma.establish`.
    pub establish: i64,
    /// `policy.karma.establishSeeds`.
    pub establish_seeds: i64,
    /// `policy.ranking.offsetHours`.
    pub offset_hours: i64,
    /// `policy.ranking.exponentMilli`.
    pub exponent_milli: i64,
    /// `policy.ranking.curatedBonus`.
    pub curated_bonus: i64,
    /// `policy.ranking.frontpageBonus`.
    pub frontpage_bonus: i64,
    /// `policy.ranking.cutoffDays`.
    pub cutoff_days: i64,
    /// Gates (`karma/1` defaults when absent, `social.md` §8).
    pub react_second: i64,
    /// Gate `reactCreate`.
    pub react_create: i64,
    /// Gate `reactAnti`.
    pub react_anti: i64,
    /// Gate `authorModPersonal`.
    pub author_mod_personal: i64,
    /// Gate `authorModFrontpage`.
    pub author_mod_frontpage: i64,
    /// Gate `createTag`.
    pub create_tag: i64,
    /// Gate `wikiEdit`.
    pub wiki_edit: i64,
    /// Gate `sequence`.
    pub sequence: i64,
    /// `policy.display.collapseBelow`.
    pub collapse_below: i64,
    /// `policy.display.newUsers`.
    pub new_users: NewUsers,
    /// Some known value or behaviour ID fell back (§13.3): the "rules newer than your client"
    /// banner.
    pub newer_rules: bool,
}

impl EffectivePolicy {
    /// The effective policy of a declaration.
    pub fn of(decl: &Forum) -> Self {
        let empty = Policy::default();
        let p = decl.policy.as_ref().unwrap_or(&empty);
        let k = p.karma.clone().unwrap_or_default();
        let r = p.ranking.clone().unwrap_or_default();
        let g = p.gates.clone().unwrap_or_default();
        let d = p.display.clone().unwrap_or_default();
        let membership = decl.membership_mode();
        let tier = decl.default_tier();
        let moderation = p.moderation();
        let karma = KarmaAlgorithm::effective(k.algorithm.as_deref());
        let ranking = RankingAlgorithm::effective(r.algorithm.as_deref());
        let palette = p.react_palette();
        let voting = p.voting();
        let tags = p.tag_creation();
        let new_users = NewUsers::effective(d.new_users.as_deref());
        let newer_rules = membership.fallback
            || tier.fallback
            || moderation.fallback
            || karma.fallback
            || ranking.fallback
            || palette.fallback
            || voting.fallback
            || tags.fallback
            || new_users.fallback;
        Self {
            moderation: moderation.value,
            karma: karma.value,
            ranking: ranking.value,
            react_palette: palette.value,
            membership: membership.value,
            default_tier: tier.value,
            voting: voting.value,
            author_moderation: p.author_moderation(),
            tag_creation: tags.value,
            pair_cap: k.pair_cap.unwrap_or(25),
            establish: k.establish.unwrap_or(1),
            establish_seeds: k.establish_seeds.unwrap_or(2),
            offset_hours: r.offset_hours.unwrap_or(2),
            exponent_milli: r.exponent_milli.unwrap_or(1150),
            curated_bonus: r.curated_bonus.unwrap_or(10),
            frontpage_bonus: r.frontpage_bonus.unwrap_or(0),
            cutoff_days: r.cutoff_days.unwrap_or(90),
            react_second: g.react_second.unwrap_or(5),
            react_create: g.react_create.unwrap_or(10),
            react_anti: g.react_anti.unwrap_or(20),
            author_mod_personal: g.author_mod_personal.unwrap_or(50),
            author_mod_frontpage: g.author_mod_frontpage.unwrap_or(2000),
            create_tag: g.create_tag.unwrap_or(10),
            wiki_edit: g.wiki_edit.unwrap_or(5),
            sequence: g.sequence.unwrap_or(1000),
            collapse_below: d.collapse_below.unwrap_or(-5),
            new_users: new_users.value,
            newer_rules,
        }
    }

    /// `(name, value)` for every `PARAMS` name, in order, encoded as in `inputDigest`
    /// (integers in shortest decimal, booleans `true`/`false`, known values as written).
    pub fn params(&self) -> Vec<(String, String)> {
        let vals: [String; 23] = [
            self.membership.as_str().into(),
            self.default_tier.as_str().into(),
            self.voting.as_str().into(),
            self.author_moderation.to_string(),
            self.tag_creation.as_str().into(),
            self.pair_cap.to_string(),
            self.establish.to_string(),
            self.establish_seeds.to_string(),
            self.offset_hours.to_string(),
            self.exponent_milli.to_string(),
            self.curated_bonus.to_string(),
            self.frontpage_bonus.to_string(),
            self.cutoff_days.to_string(),
            self.react_second.to_string(),
            self.react_create.to_string(),
            self.react_anti.to_string(),
            self.author_mod_personal.to_string(),
            self.author_mod_frontpage.to_string(),
            self.create_tag.to_string(),
            self.wiki_edit.to_string(),
            self.sequence.to_string(),
            self.collapse_below.to_string(),
            self.new_users.as_str().into(),
        ];
        PARAMS
            .iter()
            .zip(vals)
            .map(|(n, v)| ((*n).to_owned(), v))
            .collect()
    }

    /// The body of the `params` line: `name=value` joined by `,`.
    pub fn params_line(&self) -> String {
        self.params()
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The body of the `algs` line: `moderation,karma,ranking,reactPalette`.
    pub fn algs_line(&self) -> String {
        format!(
            "{},{},{},{}",
            self.moderation.as_str(),
            self.karma.as_str(),
            self.ranking.as_str(),
            self.react_palette.as_str()
        )
    }

    /// The DTO (`engine-api.md` §4.3 `EffectivePolicy`).
    pub fn dto(&self) -> dto::EffectivePolicy {
        dto::EffectivePolicy {
            moderation: self.moderation.as_str().into(),
            karma: self.karma.as_str().into(),
            ranking: self.ranking.as_str().into(),
            react_palette: self.react_palette.as_str().into(),
            params: self.params(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_atp::syntax::Datetime;

    #[test]
    fn defaults_and_presets() {
        let at = Datetime::parse("2026-01-01T00:00:00Z").unwrap();
        let mut f = Forum::create("x", at.clone(), MembershipMode::Open);
        f.policy = None;
        f.membership = None;
        f.default_tier = None;
        let p = EffectivePolicy::of(&f);
        assert_eq!(
            p.params_line(),
            "membership=approval,defaultTier=personal,voting=reacts,authorModeration=true,\
             tagCreation=open,pairCap=25,establish=1,establishSeeds=2,offsetHours=2,\
             exponentMilli=1150,curatedBonus=10,frontpageBonus=0,cutoffDays=90,reactSecond=5,\
             reactCreate=10,reactAnti=20,authorModPersonal=50,authorModFrontpage=2000,\
             createTag=10,wikiEdit=5,sequence=1000,collapseBelow=-5,newUsers=collapse"
        );
        assert_eq!(p.algs_line(), "mod/1,karma/1,magic/1,default/1");
        assert!(!p.newer_rules);
        let a = EffectivePolicy::of(&Forum::create("x", at, MembershipMode::Approval));
        assert_eq!(a.react_anti, 5);
        assert_eq!(a.membership, MembershipMode::Approval);
        f.membership = Some("invite".into());
        assert!(EffectivePolicy::of(&f).newer_rules);
    }
}
