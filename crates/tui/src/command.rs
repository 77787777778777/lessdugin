// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `:` command line (`synthesis-v1.md` §6.11 Overlays: "`:` command line (accepts any CLI
//! subcommand)").
//!
//! The CLI is a separate crate, so the TUI does not call it. Instead this is a thin parser
//! over the M3 verbs that maps each one onto the same `engine.*` calls the keys already make
//! (`docs/plan.md` §M3: "every MVP action is reachable in all three clients (in the TUI at
//! least through the `:` command line)"). The verbs mirror the CLI's names where they exist:
//!
//! | Command | Engine call | Same as key |
//! |---|---|---|
//! | `vote up\|down\|strong-up\|strong-down\|clear` | `opinion` (`Karma`) | `u`/`U`/`d`/`D`/`0` |
//! | `agree for\|against\|strong-for\|strong-against\|clear` | `opinion` (`Agreement`) | `a`/`A`/`z`/`Z`/`0` |
//! | `react <id> [for\|against]` | `opinion` (`React`) | `x`, then Enter |
//! | `unreact <id>` | `opinion` (`React`, remove) | `x`, then Enter again |
//! | `mod <action> [subject] [reason]` | `mod_action` | `m` |
//! | `notifications [all]`, `read`, `read-all` | `notifications`, `mark_*` | `N`, `a`, `M` |
//! | `activity` | `recent_discussion` | `Tab` on a forum |
//! | `user [actor]` | the forum view's `moderators` | - |
//! | `mods [role]` | the forum view's `moderators` | - |
//! | `mods add <actor> [admin|moderator]`, `mods remove <actor>` | `mods_set` | - |
//!
//! `parse` is pure (no engine, no state); `State::command` runs it.

use lessdugin_core::dto::{
    AccessLevel, AgreementValue, Capability, KarmaValue, ModActionIntent, ModActionValue, ModRole,
    PinScope, Tier,
};

/// What the `:` command line was asked to do, once parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// `vote up|down|strong-up|strong-down|clear`: the karma axis.
    Vote(Vote),
    /// `agree for|against|strong-for|strong-against|clear`: the agreement axis.
    Agree(Agree),
    /// `react <id> [for|against]`: one react from the `default/1` palette.
    React {
        /// The palette id (`default/1`).
        id: String,
        /// Against, when the verb said so.
        against: bool,
    },
    /// `unreact <id>`: take a react back.
    Unreact {
        /// The palette id.
        id: String,
    },
    /// `mod <action> [subject...]`: a forum-tier moderation action.
    Mod(ModVerb),
    /// `notifications [all]`: the Notifications screen, unread-only or all.
    Notifications {
        /// List every notification, not just the unread ones.
        all: bool,
    },
    /// `read`: mark the selected notification read.
    Read,
    /// `read-all`: mark every listed notification read.
    ReadAll,
    /// `activity`: the forum's Recent Discussion tab.
    Activity,
    /// `user [actor]`: who an actor is and what they posted here.
    User {
        /// The actor, when named (else the selected item's author).
        actor: Option<String>,
    },
    /// `mods [role]`: lists the forum's declared moderators.
    ModsList {
        /// The role filter, when named (`owner`, `admin`, `moderator`).
        role: Option<String>,
    },
    /// `mods add <actor> [admin|moderator]`, `mods remove <actor>`: `mods_set`.
    ModsSet {
        /// The DID or handle.
        subject: String,
        /// The role it takes (ignored when removing).
        role: ModRole,
        /// Remove instead of add.
        remove: bool,
    },
}

/// The karma axis's values (`vote`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    /// `up`.
    Up,
    /// `strong-up`.
    StrongUp,
    /// `down`.
    Down,
    /// `strong-down`.
    StrongDown,
    /// `clear`.
    Clear,
}

/// The agreement axis's values (`agree`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agree {
    /// `for`.
    For,
    /// `strong-for`.
    StrongFor,
    /// `against`.
    Against,
    /// `strong-against`.
    StrongAgainst,
    /// `clear`.
    Clear,
}

/// A moderation action (`mod ...`), each of which becomes a [`ModActionIntent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModVerb {
    /// `hide [reason]`.
    Hide,
    /// `spam [reason]`.
    Spam,
    /// `unhide`.
    Unhide,
    /// `ban [reason]`.
    Ban,
    /// `unban`.
    Unban,
    /// `pin`.
    Pin,
    /// `unpin`.
    Unpin,
    /// `lock`.
    Lock,
    /// `unlock`.
    Unlock,
    /// `tier personal|frontpage|curated [reason]`.
    Tier(Tier),
    /// `restrict [reason]`.
    Restrict,
    /// `unrestrict`.
    Unrestrict,
    /// `nullify [reason]`.
    Nullify,
    /// `approve`: grants a membership (`engine.grant`).
    Approve,
}

/// One parsed command line: a verb, its optional explicit subject, and an optional reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// What to do.
    pub verb: Verb,
    /// A subject named on the command line (a uri, a DID or a handle). `None`: the selection.
    pub subject: Option<String>,
    /// The reason (a moderation action's `reason`, or the `user`/`mods` selector).
    pub reason: Option<String>,
}

/// What a command line said, or why it made no sense.
pub type Parsed = Result<Command, String>;

/// Parses a command line (without its leading `:`).
///
/// # Errors
/// Returns the usage message for an unknown verb or an unusable argument.
pub fn parse(line: &str) -> Parsed {
    let mut words = line.split_whitespace();
    let Some(verb) = words.next() else {
        return Err(usage());
    };
    let rest: Vec<&str> = words.collect();
    match verb {
        "vote" => Ok(Command {
            verb: Verb::Vote(vote(rest.first().copied())?),
            subject: rest.get(1).map(|s| (*s).to_owned()),
            reason: None,
        }),
        "agree" => Ok(Command {
            verb: Verb::Agree(agree(rest.first().copied())?),
            subject: rest.get(1).map(|s| (*s).to_owned()),
            reason: None,
        }),
        "react" => {
            let Some(id) = rest.first() else {
                return Err("react needs a react id (try `react heart`).".into());
            };
            let against = match rest.get(1).copied() {
                None | Some("for") => false,
                Some("against") => true,
                Some(other) => {
                    return Err(format!("react takes `for` or `against`, not `{other}`."));
                }
            };
            Ok(Command {
                verb: Verb::React {
                    id: (*id).to_owned(),
                    against,
                },
                subject: rest.get(2).map(|s| (*s).to_owned()),
                reason: None,
            })
        }
        "unreact" => {
            let Some(id) = rest.first() else {
                return Err("unreact needs a react id.".into());
            };
            Ok(Command {
                verb: Verb::Unreact {
                    id: (*id).to_owned(),
                },
                subject: rest.get(1).map(|s| (*s).to_owned()),
                reason: None,
            })
        }
        "mod" | "moderate" => mod_verb(&rest),
        "activity" => Ok(Command {
            verb: Verb::Activity,
            subject: None,
            reason: None,
        }),
        "notifications" | "notifs" => match rest.first().copied() {
            None | Some("unread") => Ok(Command {
                verb: Verb::Notifications { all: false },
                subject: None,
                reason: None,
            }),
            Some("all") => Ok(Command {
                verb: Verb::Notifications { all: true },
                subject: None,
                reason: None,
            }),
            Some(other) => Err(format!(
                "notifications takes `all` or `unread`, not `{other}`."
            )),
        },
        "read" => Ok(Command {
            verb: Verb::Read,
            subject: None,
            reason: None,
        }),
        "read-all" | "mark-read" => Ok(Command {
            verb: Verb::ReadAll,
            subject: None,
            reason: None,
        }),
        "user" | "whoami" => Ok(Command {
            verb: Verb::User {
                actor: rest.first().map(|s| (*s).to_owned()),
            },
            subject: None,
            reason: None,
        }),
        "mods" | "moderators" => {
            // `mods`: list them; `mods add <did> [role]` / `mods remove <did>`: `mods_set`
            match rest.first().copied() {
                Some("add") => {
                    let Some(did) = rest.get(1).copied() else {
                        return Err("mods add needs a DID or handle.".into());
                    };
                    let role = match rest.get(2).copied() {
                        None | Some("moderator") | Some("mod") => ModRole::Moderator,
                        Some("admin") => ModRole::Admin,
                        Some(other) => {
                            return Err(format!(
                                "mods add takes admin or moderator, not `{other}`."
                            ));
                        }
                    };
                    Ok(Command {
                        verb: Verb::ModsSet {
                            subject: (*did).to_owned(),
                            role,
                            remove: false,
                        },
                        subject: None,
                        reason: None,
                    })
                }
                Some("remove") | Some("rm") => {
                    let Some(did) = rest.get(1).copied() else {
                        return Err("mods remove needs a DID or handle.".into());
                    };
                    Ok(Command {
                        verb: Verb::ModsSet {
                            subject: (*did).to_owned(),
                            role: ModRole::Moderator,
                            remove: true,
                        },
                        subject: None,
                        reason: None,
                    })
                }
                None => Ok(Command {
                    verb: Verb::ModsList { role: None },
                    subject: None,
                    reason: None,
                }),
                Some(role) => Ok(Command {
                    verb: Verb::ModsList {
                        role: Some((*role).to_owned()),
                    },
                    subject: None,
                    reason: None,
                }),
            }
        }
        other => Err(format!(
            "Unknown command `{other}`. {}\n{}",
            if other == "help" { "" } else { "Try `help`." },
            usage()
        )),
    }
}

fn vote(v: Option<&str>) -> Result<Vote, String> {
    match v {
        Some("up") => Ok(Vote::Up),
        Some("strong-up") => Ok(Vote::StrongUp),
        Some("down") => Ok(Vote::Down),
        Some("strong-down") => Ok(Vote::StrongDown),
        Some("clear") | None => Ok(Vote::Clear),
        Some(other) => Err(format!(
            "vote takes up, strong-up, down, strong-down or clear, not `{other}`."
        )),
    }
}

fn agree(v: Option<&str>) -> Result<Agree, String> {
    match v {
        Some("for") => Ok(Agree::For),
        Some("strong-for") => Ok(Agree::StrongFor),
        Some("against") => Ok(Agree::Against),
        Some("strong-against") => Ok(Agree::StrongAgainst),
        Some("clear") | None => Ok(Agree::Clear),
        Some(other) => Err(format!(
            "agree takes for, strong-for, against, strong-against or clear, not `{other}`."
        )),
    }
}

/// `mod <action> [subject] [reason...]`, `mod tier <tier> [reason...]`.
fn mod_verb(rest: &[&str]) -> Parsed {
    let Some(action) = rest.first().copied() else {
        return Err(format!(
            "mod needs an action (hide, spam, unhide, ban, unban, pin, unpin, lock, unlock, \
             tier, restrict, unresrict, nullify, approve).\n{}",
            usage()
        ));
    };
    // `mod tier frontpage "because"`: the tier is the first argument, not a subject
    let mut args = &rest[1..];
    let verb = match action {
        "hide" => ModVerb::Hide,
        "spam" => ModVerb::Spam,
        "unhide" | "keep" => ModVerb::Unhide,
        "ban" => ModVerb::Ban,
        "unban" => ModVerb::Unban,
        "pin" => ModVerb::Pin,
        "unpin" => ModVerb::Unpin,
        "lock" => ModVerb::Lock,
        "unlock" => ModVerb::Unlock,
        "restrict" => ModVerb::Restrict,
        "unrestrict" => ModVerb::Unrestrict,
        "nullify" => ModVerb::Nullify,
        "approve" => ModVerb::Approve,
        "tier" => {
            let Some(name) = args.first().copied() else {
                return Err("mod tier needs personal, frontpage or curated.".into());
            };
            let tier = match name {
                "personal" => Tier::Personal,
                "frontpage" | "front" => Tier::Frontpage,
                "curated" | "curate" => Tier::Curated,
                other => {
                    return Err(format!(
                        "mod tier takes personal, frontpage or curated, not `{other}`."
                    ));
                }
            };
            args = &args[1..];
            ModVerb::Tier(tier)
        }
        other => {
            return Err(format!(
                "Unknown mod action `{other}`. Try hide, spam, unhide, ban, unban, pin, unpin, \
                 lock, unlock, tier, restrict, unrestrict, nullify or approve."
            ));
        }
    };
    let (subject, reason) = split_subject(args);
    Ok(Command {
        verb: Verb::Mod(verb),
        subject,
        reason,
    })
}

/// An explicit subject is the first argument when it is after the action; a reason is the
/// rest. `mod hide` uses the selection and `mod hide at://...` names a subject.
fn split_subject(args: &[&str]) -> (Option<String>, Option<String>) {
    match args.split_first() {
        Some((first, tail))
            if {
                let f = *first;
                f.starts_with("at://")
                    || f.starts_with("did:")
                    || f.starts_with("tag:")
                    || f.contains('.')
            } =>
        {
            let reason = trim_reason(&tail.join(" "));
            (Some((*first).to_owned()), reason)
        }
        Some((first, tail)) => {
            let whole = if tail.is_empty() {
                (*first).to_owned()
            } else {
                std::iter::once(*first)
                    .chain(tail.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            (None, trim_reason(&whole))
        }
        None => (None, None),
    }
}

/// The reason, without the surrounding quotes a shell user (or a test) writes.
fn trim_reason(s: &str) -> Option<String> {
    let t: String = s.trim().to_owned();
    if t.is_empty() {
        return None;
    }
    let quoted = (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2);
    Some(if quoted {
        t[1..t.len() - 1].to_owned()
    } else {
        t
    })
}

/// The command line's help (shown by `help`, and on a parse error).
pub fn usage() -> String {
    [
        "vote up|strong-up|down|strong-down|clear [subject]",
        "agree for|strong-for|against|strong-against|clear [subject]",
        "react <id> [for|against] [subject]",
        "unreact <id> [subject]",
        "mod hide|spam|unhide|ban|unban|pin|unpin|lock|unlock|restrict|unrestrict|nullify|approve [subject] [reason]",
        "mod tier personal|frontpage|curated [subject] [reason]",
        "notifications [all|unread]",
        "read",
        "read-all",
        "activity",
        "user [actor]",
        "mods [role]",
        "mods add <did|handle> [admin|moderator]",
        "mods remove <did|handle>",
    ]
    .join("\n")
}

/// The karma intent a [`Vote`] names.
pub fn vote_intent(v: Vote) -> lessdugin_core::dto::OpinionIntent {
    use lessdugin_core::dto::OpinionIntent;
    OpinionIntent::Karma {
        value: match v {
            Vote::Up => Some(KarmaValue::Up),
            Vote::StrongUp => Some(KarmaValue::StrongUp),
            Vote::Down => Some(KarmaValue::Down),
            Vote::StrongDown => Some(KarmaValue::StrongDown),
            Vote::Clear => None,
        },
    }
}

/// The agreement intent an [`Agree`] names.
pub fn agree_intent(a: Agree) -> lessdugin_core::dto::OpinionIntent {
    use lessdugin_core::dto::OpinionIntent;
    OpinionIntent::Agreement {
        value: match a {
            Agree::For => Some(AgreementValue::Agree),
            Agree::StrongFor => Some(AgreementValue::StrongAgree),
            Agree::Against => Some(AgreementValue::Disagree),
            Agree::StrongAgainst => Some(AgreementValue::StrongDisagree),
            Agree::Clear => None,
        },
    }
}

/// The `ModActionIntent` a [`ModVerb`] names for `subject` in `forum`, with `reason`.
pub fn mod_intent(
    verb: &ModVerb,
    forum: &str,
    subject: &str,
    reason: Option<String>,
) -> ModActionIntent {
    let assert = |action: ModActionValue| ModActionIntent::Assert {
        forum: forum.to_owned(),
        // the menu's own scope (social.md §4.4); `author`-scoped actions stay on `m`
        scope: lessdugin_core::dto::ModScope::Forum,
        subject: subject.to_owned(),
        action,
        reason: reason.clone(),
        until_ms: None,
    };
    let remove = |key: &str| ModActionIntent::Remove {
        forum: forum.to_owned(),
        scope: lessdugin_core::dto::ModScope::Forum,
        subject: subject.to_owned(),
        action_key: key.to_owned(),
    };
    match verb {
        ModVerb::Hide => assert(ModActionValue::Hide {
            hidden: true,
            spam: false,
            show_reason: true,
        }),
        ModVerb::Spam => assert(ModActionValue::Hide {
            hidden: true,
            spam: true,
            show_reason: true,
        }),
        ModVerb::Unhide => remove("hide"),
        ModVerb::Ban => assert(ModActionValue::Access {
            level: AccessLevel::Banned,
        }),
        ModVerb::Unban => remove("access"),
        ModVerb::Pin => assert(ModActionValue::Pin {
            scope: PinScope::Forum,
            pinned: true,
        }),
        ModVerb::Unpin => remove("pin:forum"),
        ModVerb::Lock => assert(ModActionValue::Lock { locked: true }),
        ModVerb::Unlock => remove("lock"),
        ModVerb::Tier(tier) => assert(ModActionValue::Tier {
            tier: *tier,
            notice: None,
        }),
        ModVerb::Restrict => assert(ModActionValue::Restrict {
            capabilities: vec![Capability::Post, Capability::Comment],
            nullify_votes: false,
        }),
        ModVerb::Nullify => assert(ModActionValue::Restrict {
            capabilities: vec![],
            nullify_votes: true,
        }),
        ModVerb::Unrestrict => remove("restrict"),
        // `approve` is a grant, not a mod action: the caller sends `Effect::Grant` instead
        ModVerb::Approve => remove("access"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_m3_verbs_parse() {
        assert_eq!(parse("vote up").unwrap().verb, Verb::Vote(Vote::Up));
        assert_eq!(
            parse("vote strong-down").unwrap().verb,
            Verb::Vote(Vote::StrongDown)
        );
        assert_eq!(
            parse("agree against").unwrap().verb,
            Verb::Agree(Agree::Against)
        );
        assert_eq!(
            parse("react heart").unwrap().verb,
            Verb::React {
                id: "heart".into(),
                against: false
            }
        );
        assert_eq!(
            parse("react heart against").unwrap().verb,
            Verb::React {
                id: "heart".into(),
                against: true
            }
        );
        assert_eq!(
            parse("unreact heart").unwrap().verb,
            Verb::Unreact { id: "heart".into() }
        );
        assert_eq!(parse("activity").unwrap().verb, Verb::Activity);
        assert_eq!(
            parse("notifications").unwrap().verb,
            Verb::Notifications { all: false }
        );
        assert_eq!(
            parse("notifications all").unwrap().verb,
            Verb::Notifications { all: true }
        );
        assert_eq!(parse("read").unwrap().verb, Verb::Read);
        assert_eq!(parse("read-all").unwrap().verb, Verb::ReadAll);
        assert_eq!(
            parse("user alice").unwrap().verb,
            Verb::User {
                actor: Some("alice".into())
            }
        );
        assert_eq!(parse("mods").unwrap().verb, Verb::ModsList { role: None });
    }

    #[test]
    fn mod_verbs_carry_their_subject_and_reason() {
        let c = parse("mod hide at://did:plc:x/app.bsky.feed.post/1 spam").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Hide));
        assert_eq!(
            c.subject.as_deref(),
            Some("at://did:plc:x/app.bsky.feed.post/1")
        );
        assert_eq!(c.reason.as_deref(), Some("spam"));

        // no subject: the selection
        let c = parse("mod ban bob.example.com because").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Ban));
        assert_eq!(c.subject.as_deref(), Some("bob.example.com"));

        // a quote is stripped
        let c = parse("mod lock \"off topic\"").unwrap();
        assert_eq!(c.subject, None);
        assert_eq!(c.reason.as_deref(), Some("off topic"));

        // the tier is the action's argument, not a subject
        let c = parse("mod tier curated \"great\" ").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Tier(Tier::Curated)));
        assert_eq!(c.subject, None);
        assert_eq!(c.reason.as_deref(), Some("great"));
    }

    #[test]
    fn bad_input_says_what_it_wanted() {
        assert!(parse("").unwrap_err().contains("vote up"));
        assert!(parse("frobnicate").unwrap_err().contains("Unknown command"));
        assert!(parse("vote sideways").unwrap_err().contains("vote takes"));
        assert!(parse("mod").unwrap_err().contains("mod needs an action"));
        assert!(
            parse("mod tier")
                .unwrap_err()
                .contains("personal, frontpage")
        );
        assert!(parse("react").unwrap_err().contains("react id"));
    }
    #[test]
    fn mods_lists_sets_and_removes() {
        assert_eq!(parse("mods").unwrap().verb, Verb::ModsList { role: None });
        assert_eq!(
            parse("mods admin").unwrap().verb,
            Verb::ModsList {
                role: Some("admin".into())
            }
        );
        assert_eq!(
            parse("mods add did:plc:x").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Moderator,
                remove: false
            }
        );
        assert_eq!(
            parse("mods add did:plc:x admin").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Admin,
                remove: false
            }
        );
        assert_eq!(
            parse("mods remove did:plc:x").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Moderator,
                remove: true
            }
        );
        assert!(parse("mods add").unwrap_err().contains("needs a DID"));
        assert!(parse("mods remove").unwrap_err().contains("needs a DID"));
        assert!(parse("mods add did:plc:x owner").is_err());
    }
}
