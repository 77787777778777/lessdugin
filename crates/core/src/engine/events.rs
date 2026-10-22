// SPDX-License-Identifier: AGPL-3.0-or-later
//! Events (`engine-api.md` §3.8): `subscribe(cb)`, delivered in order and never re-entrantly.
//!
//! [`EventBus::emit`] queues the event; the first emitter delivers the queue to every
//! subscriber, outside every lock, until it is empty. An event emitted while another one is
//! being delivered (by a callback, or by another thread on native targets) is queued and
//! delivered by the delivering caller after the current one, so callbacks never overlap and see
//! the events in emission order. A callback may call back into the engine (its writes enqueue
//! events; they are delivered after it returns).
//!
//! The outbox reports through [`EngineListener`]: `on_write` becomes `Write { id, state }`
//! followed by `Changed [Outbox]`; `on_applied` (called after the write's final state is saved
//! and reported) drops the cached models of the forums the write touched, then emits
//! `Changed { topics }` (forum, thread, user). Account changes emit `Changed [Accounts]`.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

use crate::dto::{Event, Topic, WriteTicket};
use crate::host::MaybeSendSync;
use crate::outbox::{AppliedRecord, AppliedWrite, ApplyReport, OutboxListener};
use crate::store::schema::{GLOBAL_SCOPE, scope_of};

use super::models::Runtime;

/// An event callback (`Fn(Event)`; `Send + Sync` on native targets).
pub trait EventCallback: Fn(Event) + MaybeSendSync {}

impl<F: Fn(Event) + MaybeSendSync + ?Sized> EventCallback for F {}

/// A subscription handle ([`super::Engine::unsubscribe`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Default)]
struct Queue {
    events: VecDeque<Event>,
    delivering: bool,
}

/// The subscriber list and the delivery queue.
#[derive(Default)]
pub(crate) struct EventBus {
    subs: Mutex<Vec<(u64, Arc<dyn EventCallback>)>>,
    next: AtomicU64,
    queue: Mutex<Queue>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears `delivering` if a callback panics, so the next emitter delivers what is left.
struct Delivering<'a>(&'a Mutex<Queue>);

impl Drop for Delivering<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            lock(self.0).delivering = false;
        }
    }
}

impl EventBus {
    pub(crate) fn subscribe(&self, cb: Arc<dyn EventCallback>) -> SubscriptionId {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        lock(&self.subs).push((id, cb));
        SubscriptionId(id)
    }

    pub(crate) fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut subs = lock(&self.subs);
        let before = subs.len();
        subs.retain(|(i, _)| *i != id.0);
        subs.len() != before
    }

    /// Queues `event` and, unless a delivery is running, delivers the queue.
    pub(crate) fn emit(&self, event: Event) {
        {
            let mut q = lock(&self.queue);
            q.events.push_back(event);
            if q.delivering {
                return;
            }
            q.delivering = true;
        }
        let _guard = Delivering(&self.queue);
        loop {
            let event = {
                let mut q = lock(&self.queue);
                match q.events.pop_front() {
                    Some(e) => e,
                    None => {
                        q.delivering = false;
                        return;
                    }
                }
            };
            let subs: Vec<Arc<dyn EventCallback>> = lock(&self.subs)
                .iter()
                .map(|(_, s)| Arc::clone(s))
                .collect();
            for s in subs {
                s(event.clone());
            }
        }
    }

    /// `Changed { topics }` (sorted, deduplicated; nothing if empty).
    pub(crate) fn changed(&self, topics: impl IntoIterator<Item = Topic>) {
        let topics: BTreeSet<Topic> = topics.into_iter().collect();
        if !topics.is_empty() {
            self.emit(Event::Changed {
                topics: topics.into_iter().collect(),
            });
        }
    }
}

/// The engine's [`OutboxListener`].
pub(crate) struct EngineListener {
    pub(crate) rt: Arc<Runtime>,
}

fn str_at<'v>(v: &'v Value, path: &[&str]) -> Option<&'v str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str()
}

impl EngineListener {
    /// The forum and thread of one applied record: from the value (`forum`, `root`), else from
    /// the fk registry and the cached models (deletes).
    fn topics_of(&self, r: &AppliedRecord, forums: &mut BTreeSet<String>, out: &mut Vec<Topic>) {
        let Ok(uri) = AtUri::parse(r.uri()) else {
            return;
        };
        let coll = uri.collection().as_str();
        let rkey = uri.rkey().as_str();
        let value = match r {
            AppliedRecord::Put { value, .. } => Some(value),
            AppliedRecord::Delete { .. } => None,
        };
        let forum = if coll == nsid::FORUM {
            Some(uri.as_str().to_owned())
        } else if let Some(f) = value.and_then(|v| str_at(v, &["forum"])) {
            Some(f.to_owned())
        } else {
            match scope_of(coll, rkey) {
                GLOBAL_SCOPE => None,
                fk => self.rt.forum_of_fk(fk),
            }
        };
        let thread = if coll == nsid::POST {
            Some(uri.as_str().to_owned())
        } else if coll == nsid::COMMENT {
            value
                .and_then(|v| str_at(v, &["root", "uri"]))
                .map(str::to_owned)
                .or_else(|| self.rt.thread_of(uri.as_str()))
        } else {
            None
        };
        match forum {
            Some(f) => {
                self.rt.register_uri(&f);
                forums.insert(f);
            }
            // a record of a forum this engine never loaded: every model may hold it
            None if scope_of(coll, rkey) != GLOBAL_SCOPE => {
                forums.insert(String::new());
            }
            None => {}
        }
        if let Some(t) = thread {
            out.push(Topic::Thread { uri: t });
        }
    }
}

impl OutboxListener for EngineListener {
    fn on_write(&self, ticket: &WriteTicket) {
        self.rt.bus.emit(Event::Write {
            id: ticket.id.clone(),
            state: ticket.state.clone(),
        });
        // the outbox list changed (a new write, or a write's state)
        self.rt.bus.changed([Topic::Outbox]);
    }

    fn on_applied(&self, write: &AppliedWrite, _report: &ApplyReport) {
        let mut forums: BTreeSet<String> = BTreeSet::new();
        let mut topics = vec![Topic::User {
            did: write.account.to_string(),
        }];
        for r in &write.records {
            self.topics_of(r, &mut forums, &mut topics);
        }
        if forums.remove("") {
            self.rt.invalidate_all();
        }
        for f in &forums {
            self.rt.invalidate(f);
        }
        topics.extend(forums.into_iter().map(|uri| Topic::Forum { uri }));
        self.rt.bus.changed(topics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn delivery_is_ordered_and_not_reentrant() {
        let bus = Arc::new(EventBus::default());
        let seen: Arc<StdMutex<Vec<String>>> = Arc::default();
        let depth = Arc::new(AtomicU64::new(0));
        {
            let (bus2, seen, depth) = (Arc::clone(&bus), Arc::clone(&seen), Arc::clone(&depth));
            bus.subscribe(Arc::new(move |e: Event| {
                assert_eq!(
                    depth.fetch_add(1, Ordering::SeqCst),
                    0,
                    "re-entrant delivery"
                );
                if let Event::Changed { topics } = &e
                    && topics == &[Topic::Outbox]
                {
                    // emitted from inside a callback: delivered after this one returns
                    bus2.changed([Topic::Drafts]);
                }
                seen.lock()
                    .unwrap()
                    .push(serde_json::to_string(&e).unwrap());
                depth.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        let id = {
            let seen = Arc::clone(&seen);
            bus.subscribe(Arc::new(move |_e: Event| {
                seen.lock().unwrap().push("second".into());
            }))
        };
        bus.changed([Topic::Outbox]);
        assert!(bus.unsubscribe(id));
        assert!(!bus.unsubscribe(id));
        bus.changed([Topic::Accounts, Topic::Accounts]);
        bus.changed([]);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            [
                r#"{"kind":"changed","topics":[{"kind":"outbox"}]}"#,
                "second",
                r#"{"kind":"changed","topics":[{"kind":"drafts"}]}"#,
                "second",
                r#"{"kind":"changed","topics":[{"kind":"accounts"}]}"#,
            ]
        );
    }
}
