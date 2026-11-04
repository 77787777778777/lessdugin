// SPDX-License-Identifier: AGPL-3.0-or-later
//! The core's in-memory inverted index of one forum (`sync.md` §13).
//!
//! ```text
//! docs    = the listed posts and comments of the model (not under a Missing root)
//! tokens  = fold(title) and fold(P) split at White_Space → the docs holding each token
//! lookup  = ⋂ over the query's folded terms t of ⋃ { docs(token) : t is a substring of token }
//!           ∪ the docs whose P was not known at build time
//! ```
//!
//! A term holds no white space, so every occurrence of it in the folded title or P lies inside
//! one token: the lookup is a superset of the matches (`matches_all`), never missing one, and
//! the engine renders and checks only those candidates. An index belongs to one model build
//! (the [`ForumModel`] `Arc` it was built from, which a `Changed` event or a refold replaces),
//! so it is invalidated with the model and built again, lazily, by the next search. The tokens
//! of an item depend only on its record (uri and CID), so a new build carries over those of the
//! items the forum's previous index already held at the same CID and reads only the others.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::model::ForumModel;

use super::fold;

/// The index of one forum model.
#[derive(Debug)]
pub(crate) struct ForumIndex {
    /// The model it was built from.
    model: Weak<ForumModel>,
    /// The indexed items' uris, by position.
    docs: Vec<String>,
    /// Their CIDs, by position.
    cids: Vec<String>,
    /// Each folded token of a title or P, with the positions of the items holding it, ascending.
    tokens: BTreeMap<String, Vec<u32>>,
    /// Items whose P was not known when the index was built (a spilled body whose source was not
    /// in memory): always candidates.
    unknown: Vec<u32>,
}

impl ForumIndex {
    /// The indexed items that may match every folded term, in index order. Empty `terms` give
    /// every item.
    pub(crate) fn candidates(&self, terms: &[String]) -> Vec<&str> {
        let mut keep = vec![true; self.docs.len()];
        for term in terms {
            let mut found = vec![false; self.docs.len()];
            for (token, docs) in &self.tokens {
                if token.contains(term.as_str()) {
                    for &d in docs {
                        found[d as usize] = true;
                    }
                }
            }
            for &d in &self.unknown {
                found[d as usize] = true;
            }
            for (k, f) in keep.iter_mut().zip(found) {
                *k &= f;
            }
        }
        self.docs
            .iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(d, _)| d.as_str())
            .collect()
    }

    /// The tokens of each item whose P was known, by uri, with its CID.
    pub(crate) fn tokens_by_item(&self) -> BTreeMap<&str, (&str, Vec<&str>)> {
        let mut by: Vec<Vec<&str>> = vec![Vec::new(); self.docs.len()];
        for (token, docs) in &self.tokens {
            for &d in docs {
                by[d as usize].push(token.as_str());
            }
        }
        let mut out: BTreeMap<&str, (&str, Vec<&str>)> = self
            .docs
            .iter()
            .zip(&self.cids)
            .zip(by)
            .map(|((uri, cid), tokens)| (uri.as_str(), (cid.as_str(), tokens)))
            .collect();
        for &d in &self.unknown {
            out.remove(self.docs[d as usize].as_str());
        }
        out
    }
}

/// Builds a [`ForumIndex`] item by item.
#[derive(Default)]
pub(crate) struct Builder {
    docs: Vec<String>,
    cids: Vec<String>,
    tokens: BTreeMap<String, Vec<u32>>,
    unknown: Vec<u32>,
}

impl Builder {
    /// Adds item `uri` at `cid` with its `title` and P (`None`: not known now; always a
    /// candidate).
    pub(crate) fn add(&mut self, uri: &str, cid: &str, title: Option<&str>, p: Option<&str>) {
        let Some(id) = self.push(uri, cid) else {
            return;
        };
        if p.is_none() {
            self.unknown.push(id);
        }
        for text in [title, p].into_iter().flatten() {
            for token in fold(text).split_whitespace() {
                self.token(token, id);
            }
        }
    }

    /// Adds item `uri` at `cid` with the folded `tokens` an earlier index found for it.
    pub(crate) fn add_tokens(&mut self, uri: &str, cid: &str, tokens: &[&str]) {
        let Some(id) = self.push(uri, cid) else {
            return;
        };
        for token in tokens {
            self.token(token, id);
        }
    }

    fn push(&mut self, uri: &str, cid: &str) -> Option<u32> {
        let id = u32::try_from(self.docs.len()).ok()?;
        self.docs.push(uri.to_owned());
        self.cids.push(cid.to_owned());
        Some(id)
    }

    fn token(&mut self, token: &str, id: u32) {
        match self.tokens.get_mut(token) {
            // positions only grow, so the last one says whether this item is there
            Some(docs) if docs.last() == Some(&id) => {}
            Some(docs) => docs.push(id),
            None => {
                self.tokens.insert(token.to_owned(), vec![id]);
            }
        }
    }

    /// The index of `model`.
    pub(crate) fn finish(self, model: &Arc<ForumModel>) -> ForumIndex {
        ForumIndex {
            model: Arc::downgrade(model),
            docs: self.docs,
            cids: self.cids,
            tokens: self.tokens,
            unknown: self.unknown,
        }
    }
}

/// The forums' indexes, at most one per forum, each used only with the model it was built from.
#[derive(Debug, Default)]
pub(crate) struct IndexCache(Mutex<BTreeMap<String, Arc<ForumIndex>>>);

impl IndexCache {
    /// The index of forum `f` built from `model`, else (`Err`) the forum's last index, built
    /// from an earlier model, whose items may be carried over. Indexes of other forums' models
    /// no longer held anywhere are dropped.
    pub(crate) fn get(
        &self,
        f: &str,
        model: &Arc<ForumModel>,
    ) -> Result<Arc<ForumIndex>, Option<Arc<ForumIndex>>> {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        m.retain(|k, ix| k == f || ix.model.strong_count() > 0);
        match m.get(f) {
            Some(ix) if std::ptr::eq(ix.model.as_ptr(), Arc::as_ptr(model)) => Ok(Arc::clone(ix)),
            previous => Err(previous.cloned()),
        }
    }

    /// Keeps `ix` as forum `f`'s index.
    pub(crate) fn put(&self, f: &str, ix: Arc<ForumIndex>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(f.to_owned(), ix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(items: &[(&str, Option<&str>, Option<&str>)]) -> Builder {
        let mut b = Builder::default();
        for (uri, title, p) in items {
            b.add(uri, "cid", *title, *p);
        }
        b
    }

    fn built(b: &Builder) -> ForumIndex {
        ForumIndex {
            model: Weak::new(),
            docs: b.docs.clone(),
            cids: b.cids.clone(),
            tokens: b.tokens.clone(),
            unknown: b.unknown.clone(),
        }
    }

    fn lookup(b: &Builder, q: &str) -> Vec<String> {
        built(b)
            .candidates(&super::super::terms(q))
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn terms_are_substrings_of_tokens_anded_over_title_and_p() {
        let b = index(&[
            ("a", Some("Café notes"), Some("shared text")),
            ("b", None, Some("SHARED words, notes")),
            ("c", Some("x"), None),
            ("d", Some("xİy"), Some("")),
        ]);
        assert_eq!(lookup(&b, "café"), ["a", "c"]);
        assert_eq!(lookup(&b, "hare note"), ["a", "b", "c"]);
        assert_eq!(lookup(&b, "words, NOTES"), ["b", "c"]);
        assert_eq!(lookup(&b, "nowhere"), ["c"]);
        assert_eq!(lookup(&b, "i"), ["c", "d"]);
        // a term never spans the space between two tokens, or the title and P
        assert_eq!(lookup(&b, "notesshared"), ["c"]);
        assert_eq!(b.tokens["shared"], [0, 1]);
    }

    #[test]
    fn carried_tokens_index_like_the_text_they_came_from() {
        let b = index(&[
            ("a", Some("Café notes"), Some("shared text")),
            ("b", None, Some("SHARED words, notes")),
            ("c", Some("x"), None),
        ]);
        let ix = built(&b);
        let by = ix.tokens_by_item();
        // an item whose P was not known has nothing to carry: it is read again
        assert_eq!(by.keys().copied().collect::<Vec<_>>(), ["a", "b"]);
        let mut again = Builder::default();
        for (uri, (cid, tokens)) in &by {
            again.add_tokens(uri, cid, tokens);
        }
        again.add("c", "cid", Some("x"), None);
        assert_eq!(again.tokens, b.tokens);
        for q in ["café", "hare note", "words, NOTES", "nowhere", "i"] {
            assert_eq!(lookup(&again, q), lookup(&b, q), "{q}");
        }
    }
}
