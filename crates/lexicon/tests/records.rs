// SPDX-License-Identifier: MIT OR Apache-2.0
//! Record structs: every lexicon property maps to a typed struct field (conformance), maximal
//! samples round-trip exactly, unknown fields and unknown union members survive
//! read-modify-write, serialised records pass the validator, and the opinion write order of
//! `data-model.md` §8.5 is deterministic.

mod common;

use common::{add_unknown_fields, nsid, sample_def, sample_record};
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::records::*;
use lessdugin_lexicon::schemas::{Def, TypeDef, registry};
use lessdugin_lexicon::validate::validate_record;
use lessdugin_lexicon::values::{
    AccessLevel, Agreement, Capability, MembershipMode, PinScope, Stance, Tier, VoteValue,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

type RoundTrip = fn(Value) -> Result<Value, String>;

fn rt<T: DeserializeOwned + Serialize>(v: Value) -> Result<Value, String> {
    let t: T = serde_json::from_value(v).map_err(|e| e.to_string())?;
    serde_json::to_value(&t).map_err(|e| e.to_string())
}

struct Entry {
    def: String,
    /// The struct carries `$type` (records and union members).
    typed: bool,
    rt: RoundTrip,
}

fn e(def: &str, typed: bool, rt: RoundTrip) -> Entry {
    let def = if def.starts_with("com.atproto") {
        def.to_owned()
    } else {
        nsid(def)
    };
    Entry { def, typed, rt }
}

/// Every modelled definition and its struct.
fn table() -> Vec<Entry> {
    vec![
        e("forum#main", true, rt::<Forum>),
        e("forum#moderator", false, rt::<Moderator>),
        e("forum#policy", false, rt::<Policy>),
        e("forum#karmaP", false, rt::<KarmaParams>),
        e("forum#rankP", false, rt::<RankParams>),
        e("forum#gates", false, rt::<Gates>),
        e("forum#display", false, rt::<DisplayParams>),
        e("forum#discovery", false, rt::<Discovery>),
        e("forum#endpoint", false, rt::<Endpoint>),
        e("membership#main", true, rt::<Membership>),
        e("post#main", true, rt::<Post>),
        e("comment#main", true, rt::<Comment>),
        e("revision#main", true, rt::<Revision>),
        e("opinion#main", true, rt::<Opinion>),
        e("opinion#react", false, rt::<React>),
        e("opinion#tagVote", false, rt::<TagVote>),
        e("modAction#main", true, rt::<ModAction>),
        e("modAction#hide", true, rt::<HideAction>),
        e("modAction#access", true, rt::<AccessAction>),
        e("modAction#restrict", true, rt::<RestrictAction>),
        e("modAction#tier", true, rt::<TierAction>),
        e("modAction#pin", true, rt::<PinAction>),
        e("modAction#lock", true, rt::<LockAction>),
        e("modAction#tag", true, rt::<TagAction>),
        e("wikiRevision#main", true, rt::<WikiRevision>),
        e("sequence#main", true, rt::<Sequence>),
        e("sequence#chapter", false, rt::<Chapter>),
        e("profile#main", true, rt::<Profile>),
        e("defs#media", false, rt::<Media>),
        e("defs#aspectRatio", false, rt::<AspectRatio>),
        e("defs#textQuote", false, rt::<TextQuote>),
        e("defs#externalLink", false, rt::<ExternalLink>),
        e("defs#link", false, rt::<Link>),
        e("markdown#main", true, rt::<Markdown>),
        e("com.atproto.repo.strongRef#main", false, rt::<StrongRef>),
        e("com.atproto.label.defs#selfLabels", true, rt::<SelfLabels>),
        e("com.atproto.label.defs#selfLabel", false, rt::<SelfLabel>),
    ]
}

fn sample_for(entry: &Entry) -> Value {
    let mut v = sample_def(registry(), &entry.def, 0);
    if entry.typed {
        let t = entry.def.strip_suffix("#main").unwrap_or(&entry.def);
        v.as_object_mut().unwrap().insert("$type".into(), json!(t));
    }
    v
}

fn properties(def: &str) -> Vec<String> {
    match registry().def(def) {
        Some(Def::Record(r)) => r.record.properties.iter().map(|(n, _)| n.clone()).collect(),
        Some(Def::Type(TypeDef::Object(o))) => {
            o.properties.iter().map(|(n, _)| n.clone()).collect()
        }
        other => panic!("{def}: {other:?}"),
    }
}

#[test]
fn every_object_definition_has_a_struct() {
    let reg = registry();
    let mut want: Vec<String> = Vec::new();
    for d in reg.docs() {
        let ours = d.id.starts_with(lessdugin_lexicon::ns::NS) && !d.id.contains(".index.");
        for (name, def) in &d.defs {
            let r = format!("{}#{name}", d.id);
            let modelled_external = r == "com.atproto.repo.strongRef#main"
                || r == "com.atproto.label.defs#selfLabels"
                || r == "com.atproto.label.defs#selfLabel";
            if (ours || modelled_external)
                && matches!(def, Def::Record(_) | Def::Type(TypeDef::Object(_)))
            {
                want.push(r);
            }
        }
    }
    want.sort();
    let mut have: Vec<String> = table().into_iter().map(|e| e.def).collect();
    have.sort();
    assert_eq!(have, want);
}

#[test]
fn every_property_maps_to_a_typed_field() {
    // Replacing any one property by a value of the wrong JSON type must make deserialisation
    // fail; a property that fell into `extra` would be accepted.
    for entry in table() {
        let base = sample_for(&entry);
        for p in properties(&entry.def) {
            let mut v = base.clone();
            let wrong = if v[&p].is_boolean() {
                json!("x")
            } else {
                json!(true)
            };
            v[&p] = wrong;
            assert!(
                (entry.rt)(v).is_err(),
                "{}: property {p:?} is not a typed field",
                entry.def
            );
        }
    }
}

#[test]
fn maximal_samples_round_trip_exactly() {
    for entry in table() {
        let v = sample_for(&entry);
        let back = (entry.rt)(v.clone()).unwrap_or_else(|e| panic!("{}: {e}\n{v:#}", entry.def));
        assert_eq!(back, v, "{}", entry.def);
    }
}

#[test]
fn unknown_fields_survive_at_every_level() {
    for entry in table() {
        let mut v = sample_for(&entry);
        add_unknown_fields(&mut v);
        let back = (entry.rt)(v.clone()).unwrap_or_else(|e| panic!("{}: {e}", entry.def));
        assert_eq!(back, v, "{}", entry.def);
    }
}

#[test]
fn records_round_trip_through_record_and_pass_the_validator() {
    for c in Collection::ALL {
        let mut v = sample_record(c.nsid());
        add_unknown_fields(&mut v);
        let rec = Record::from_value(c, v.clone()).unwrap_or_else(|e| panic!("{c}: {e}"));
        assert_eq!(rec.collection(), c);
        assert_eq!(rec.r#type(), c.nsid());
        assert!(rec.extra().contains_key("zzUnknown"));
        let out = rec.to_value().unwrap();
        assert_eq!(out, v, "{c}");
        let report = validate_record(c.nsid(), &out);
        assert!(report.is_valid(), "{c}: {:?}", report.violations);
        // and through the nsid dispatcher
        assert_eq!(Record::from_nsid_value(c.nsid(), v).unwrap().unwrap(), rec);
    }
    assert!(Record::from_nsid_value("app.bsky.feed.post", json!({})).is_none());
}

#[test]
fn absent_type_defaults_to_the_build_nsid() {
    let mut v = sample_record(Collection::Membership.nsid());
    v.as_object_mut().unwrap().remove("$type");
    let m: Membership = serde_json::from_value(v).unwrap();
    assert_eq!(m.r#type, Collection::Membership.nsid());
    assert_eq!(
        serde_json::to_value(&m).unwrap()["$type"],
        Collection::Membership.nsid()
    );
}

#[test]
fn unknown_union_members_survive() {
    let mut v = sample_record(Collection::Post.nsid());
    let foreign_md = json!({"$type": "com.example.richtext", "blocks": [1, 2]});
    v["body"] = foreign_md.clone();
    v["labels"] = json!({"$type": "com.example.labels", "x": true});
    let p: Post = serde_json::from_value(v.clone()).unwrap();
    assert!(matches!(p.body, Some(OpenUnion::Unknown(_))));
    assert!(matches!(p.labels, Some(OpenUnion::Unknown(_))));
    assert_eq!(serde_json::to_value(&p).unwrap(), v);

    // a member of the other namespace prefix is unknown too (§1.3)
    let other = if lessdugin_lexicon::ns::NS == lessdugin_lexicon::ns::NS_TEMP {
        "online.7778777.lessdugin.markdown"
    } else {
        "online.7778777.lessdugin.temp.markdown"
    };
    v["body"] = json!({"$type": other, "text": "hi"});
    let p: Post = serde_json::from_value(v.clone()).unwrap();
    assert!(matches!(p.body, Some(OpenUnion::Unknown(_))));
    // `nsid#main` names the main def too
    v["body"] =
        json!({"$type": format!("{}#main", lessdugin_lexicon::nsid::MARKDOWN), "text": "hi"});
    let p: Post = serde_json::from_value(v.clone()).unwrap();
    assert!(matches!(p.body, Some(OpenUnion::Known(_))));
    assert_eq!(serde_json::to_value(&p).unwrap(), v);

    let mut m = sample_record(Collection::ModAction.nsid());
    m["action"] =
        json!({"$type": format!("{}#feature", lessdugin_lexicon::nsid::MOD_ACTION), "until": 3});
    let ma: ModAction = serde_json::from_value(m.clone()).unwrap();
    assert!(matches!(ma.action, Action::Unknown(_)));
    assert_eq!(ma.action.kind(), None);
    assert_eq!(
        ma.action.action_key(),
        format!("{}#feature", lessdugin_lexicon::nsid::MOD_ACTION)
    );
    assert_eq!(serde_json::to_value(&ma).unwrap(), m);
}

#[test]
fn integral_floats_are_integers() {
    let v = json!({"width": 3.0, "height": 4});
    let a: AspectRatio = serde_json::from_value(v).unwrap();
    assert_eq!((a.width, a.height), (3, 4));
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        json!({"width": 3, "height": 4})
    );
    assert!(serde_json::from_value::<AspectRatio>(json!({"width": 3.5, "height": 4})).is_err());
}

#[test]
fn actions_dispatch_and_keys() {
    let ns = lessdugin_lexicon::ns::NS;
    let cases = [
        (Action::Hide(HideAction::hide()), "hide"),
        (
            Action::Access(AccessAction::new(AccessLevel::Banned)),
            "access",
        ),
        (
            Action::Restrict(RestrictAction::new(&[Capability::Vote], true)),
            "restrict",
        ),
        (Action::Tier(TierAction::new(Tier::Curated)), "tier"),
        (Action::Pin(PinAction::new(PinScope::Thread)), "pin:thread"),
        (Action::Lock(LockAction::new(vec![])), "lock"),
        (Action::Tag(TagAction::new(Some(true), None)), "tag"),
    ];
    for (a, key) in cases {
        let v = serde_json::to_value(&a).unwrap();
        let ty = v["$type"].as_str().unwrap();
        assert!(ty.starts_with(&format!("{ns}.modAction#")), "{ty}");
        let back: Action = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back, a);
        assert_eq!(a.action_key(), key);
        assert_eq!(
            lessdugin_lexicon::keys::action_key(ns, v.as_object().unwrap()).unwrap(),
            key
        );
    }
}

#[test]
fn forum_create_presets() {
    let now = Datetime::parse("2026-10-16T12:00:00.000Z").unwrap();
    let f = Forum::create("Rationality", now.clone(), MembershipMode::Approval);
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["membership"], "approval");
    assert_eq!(v["defaultTier"], "personal");
    let p = &v["policy"];
    assert_eq!(p["moderation"], "mod/1");
    assert_eq!(p["reactPalette"], "default/1");
    assert_eq!(p["voting"], "reacts");
    assert_eq!(p["authorModeration"], true);
    assert_eq!(p["tagCreation"], "open");
    assert_eq!(
        p["karma"],
        json!({"algorithm": "karma/1", "pairCap": 25, "establish": 1, "establishSeeds": 2})
    );
    assert_eq!(
        p["ranking"],
        json!({"algorithm": "magic/1", "offsetHours": 2, "exponentMilli": 1150,
               "curatedBonus": 10, "frontpageBonus": 0, "cutoffDays": 90})
    );
    assert_eq!(
        p["gates"],
        json!({"reactSecond": 0, "reactCreate": 0, "reactAnti": 5, "authorModPersonal": 10,
               "authorModFrontpage": 100, "createTag": 0, "wikiEdit": 0, "sequence": 0})
    );
    assert_eq!(
        p["display"],
        json!({"collapseBelow": -5, "newUsers": "collapse"})
    );
    let o = serde_json::to_value(Forum::create("x", now, MembershipMode::Open)).unwrap();
    assert_eq!(o["membership"], "open");
    assert_eq!(
        o["policy"]["gates"],
        json!({"reactSecond": 5, "reactCreate": 10, "reactAnti": 20, "authorModPersonal": 50,
               "authorModFrontpage": 2000, "createTag": 10, "wikiEdit": 5, "sequence": 1000})
    );
    assert!(validate_record(lessdugin_lexicon::nsid::FORUM, &v).is_valid());
    assert!(validate_record(lessdugin_lexicon::nsid::FORUM, &o).is_valid());
    assert_eq!(f.membership_mode().value, MembershipMode::Approval);
}

#[test]
fn media_names() {
    let mut m = Media::new(Blob::new(common::RAW_CID, "image/png", 3));
    for (name, shown) in [
        ("photo.png", true),
        ("a/b.png", false),
        ("a\\b", false),
        ("x\u{7}", false),
        ("x\u{85}", false),
        ("x\u{2028}", false),
        ("x\u{7f}", true),
    ] {
        m.name = Some(name.into());
        assert_eq!(m.display_name().is_some(), shown, "{name:?}");
    }
    assert!(m.blob.cid().is_some());
}

// ---------------------------------------------------------------------------------------------
// opinion apply (§8.5)

fn uri(s: &str) -> AtUri {
    AtUri::parse(s).unwrap()
}

fn dt(s: &str) -> Datetime {
    Datetime::parse(s).unwrap()
}

#[test]
fn qkey_normalisation() {
    assert_eq!(qkey(None), "");
    let q = |s: &str| TextQuote::new(s);
    assert_eq!(qkey(Some(&q("  a \t\n b  "))), "a b");
    // NFC: e + U+0301 == U+00E9
    assert_eq!(qkey(Some(&q("cafe\u{301}"))), qkey(Some(&q("caf\u{e9}"))));
    // White_Space includes U+00A0 and U+3000
    assert_eq!(qkey(Some(&q("a\u{a0}\u{3000}b"))), "a b");
    assert_eq!(qkey(Some(&q(" \u{2003} "))), "");
}

#[test]
fn opinion_apply_is_deterministic() {
    let ns = lessdugin_lexicon::ns::NS;
    let forum = uri(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{ns}.forum/rationality"
    ));
    let subject = uri(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{ns}.post/uetjixxr3a33-3l2s5xxv2ze2c"
    ));
    let t0 = dt("2026-10-16T12:00:00.000Z");
    let t1 = dt("2026-10-16T12:05:00.000Z");
    let crux = TextQuote::new("the  crux");
    let first = Opinion::apply(
        None,
        &subject,
        &forum,
        "bafyreia",
        &[
            OpinionIntent::Karma(Some(VoteValue::Up)),
            OpinionIntent::AddReact {
                react: "agree".into(),
                quote: None,
                stance: Stance::For,
            },
            OpinionIntent::AddReact {
                react: "crux".into(),
                quote: Some(crux.clone()),
                stance: Stance::For,
            },
            OpinionIntent::Tag {
                tag: "ai".into(),
                value: Some(VoteValue::Up),
            },
        ],
        &t0,
    );
    assert_eq!(first.created_at, t0);
    assert_eq!(first.updated_at, None);
    assert_eq!(first.subject.cid, "bafyreia");
    assert_eq!(first.karma(), Some(VoteValue::Up));
    let reacts: Vec<&str> = first
        .reacts
        .iter()
        .flatten()
        .map(|r| r.react.as_str())
        .collect();
    assert_eq!(reacts, ["agree", "crux"]);
    assert!(!first.is_empty());

    // keep an unknown field across the update
    let mut stored = first.clone();
    stored.extra.insert("zz".into(), json!(1));
    let second = Opinion::apply(
        Some(&stored),
        &subject,
        &forum,
        "bafyreib",
        &[
            OpinionIntent::AddReact {
                react: "insightful".into(),
                quote: None,
                stance: Stance::For,
            },
            // replaced in place
            OpinionIntent::AddReact {
                react: "agree".into(),
                quote: None,
                stance: Stance::Against,
            },
            // same qkey as "the  crux"
            OpinionIntent::RemoveReact {
                react: "crux".into(),
                quote: Some(TextQuote::new(" the crux ")),
            },
            OpinionIntent::Tag {
                tag: "ai".into(),
                value: None,
            },
            OpinionIntent::Agreement(Some(Agreement::StrongAgree)),
            OpinionIntent::Karma(None),
        ],
        &t1,
    );
    let reacts: Vec<(&str, Option<&str>)> = second
        .reacts
        .iter()
        .flatten()
        .map(|r| (r.react.as_str(), r.stance.as_deref()))
        .collect();
    assert_eq!(
        reacts,
        [("agree", Some("against")), ("insightful", Some("for"))]
    );
    assert_eq!(second.tags, None, "an emptied array is removed");
    assert_eq!(second.karma, None);
    assert_eq!(second.agreement(), Some(Agreement::StrongAgree));
    assert_eq!(second.created_at, t0);
    assert_eq!(second.updated_at, Some(t1.clone()));
    assert_eq!(second.subject.cid, "bafyreib");
    assert_eq!(second.extra.get("zz"), Some(&json!(1)));

    // determinism: the same inputs give the same bytes
    let again = Opinion::apply(Some(&stored), &subject, &forum, "bafyreib", &[], &t1);
    assert_eq!(
        serde_json::to_string(&again).unwrap(),
        serde_json::to_string(&Opinion::apply(
            Some(&stored),
            &subject,
            &forum,
            "bafyreib",
            &[],
            &t1
        ))
        .unwrap()
    );
    assert!(again.eq_ignoring_updated_at(&{
        let mut x = stored.clone();
        x.subject.cid = "bafyreib".into();
        x
    }));

    // empty(v): only $type, subject, forum and timestamps (unknown fields count as content)
    let cleared = Opinion::apply(
        Some(&first),
        &subject,
        &forum,
        "bafyreia",
        &[
            OpinionIntent::Karma(None),
            OpinionIntent::RemoveReact {
                react: "agree".into(),
                quote: None,
            },
            OpinionIntent::RemoveReact {
                react: "crux".into(),
                quote: Some(crux),
            },
            OpinionIntent::Tag {
                tag: "ai".into(),
                value: None,
            },
        ],
        &t1,
    );
    assert!(cleared.is_empty());
    assert_eq!(cleared.reacts, None);
    let mut with_unknown = cleared.clone();
    with_unknown.extra.insert("future".into(), json!(true));
    assert!(!with_unknown.is_empty());
    let mut empty_arrays = cleared;
    empty_arrays.reacts = Some(vec![]);
    empty_arrays.tags = Some(vec![]);
    assert!(empty_arrays.is_empty());
}

#[test]
fn opinion_apply_drops_duplicate_keys() {
    let ns = lessdugin_lexicon::ns::NS;
    let forum = uri(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{ns}.forum/f"
    ));
    let subject = uri(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{ns}.post/uetjixxr3a33-3l2s5xxv2ze2c"
    ));
    let t = dt("2026-10-16T12:00:00.000Z");
    let mut stored = Opinion::new(
        StrongRef::new(subject.clone(), "c"),
        forum.clone(),
        t.clone(),
    );
    let react = |r: &str, q: Option<&str>| React {
        react: r.into(),
        stance: None,
        quote: q.map(TextQuote::new),
        extra: Map::new(),
    };
    stored.reacts = Some(vec![
        react("a", Some("x  y")),
        react("b", None),
        react("a", Some("x y")),
    ]);
    stored.tags = Some(vec![
        TagVote {
            tag: "t".into(),
            value: "up".into(),
            extra: Map::new(),
        },
        TagVote {
            tag: "t".into(),
            value: "down".into(),
            extra: Map::new(),
        },
    ]);
    let out = Opinion::apply(Some(&stored), &subject, &forum, "c", &[], &t);
    assert_eq!(out.reacts.as_ref().unwrap().len(), 2);
    assert_eq!(out.tags.as_ref().unwrap().len(), 1);
    assert_eq!(out.tags.as_ref().unwrap()[0].value, "up");
}

#[test]
fn constructors_write_type_and_defaults() {
    let ns = lessdugin_lexicon::ns::NS;
    let forum = uri(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{ns}.forum/f"
    ));
    let t = dt("2026-10-16T12:00:00.000Z");
    let p = Post::article(forum.clone(), "Title", "Body", t.clone());
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["$type"], format!("{ns}.post"));
    assert_eq!(v["body"]["$type"], format!("{ns}.markdown"));
    assert_eq!(v["body"]["profile"], "lfm/1");
    assert!(validate_record(&format!("{ns}.post"), &v).is_valid());
    let prof = Profile::default();
    assert_eq!(
        serde_json::to_value(&prof).unwrap(),
        json!({"$type": format!("{ns}.profile")})
    );
    let m = Membership::new(forum, t);
    assert_eq!(
        serde_json::to_value(&m).unwrap()["$type"],
        format!("{ns}.membership")
    );
    let labels = SelfLabels::new(["porn"]);
    assert_eq!(
        serde_json::to_value(&labels).unwrap(),
        json!({"$type": "com.atproto.label.defs#selfLabels", "values": [{"val": "porn"}]})
    );
    let _ = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
}
