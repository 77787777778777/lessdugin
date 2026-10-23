// SPDX-License-Identifier: AGPL-3.0-or-later
//! The presentation defaults of the `default/1` react palette (`social.md` §9.1,
//! `docs/spec/reacts-default-1.json`): label, description, glyph and group of every react id, in
//! palette order. Only the set of ids is normative for the fold
//! ([`crate::social::full::reacts::DEFAULT_1`]); views and clients use the rest.

use std::sync::OnceLock;

use serde::Deserialize;

/// One react of the palette.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReactInfo {
    /// The id.
    pub id: String,
    /// The label.
    pub label: String,
    /// One sentence.
    pub description: String,
    /// A single emoji code point, or short ASCII.
    pub glyph: String,
    /// The group id.
    pub group: String,
}

#[derive(Deserialize)]
struct PaletteFile {
    reacts: Vec<ReactInfo>,
}

const JSON: &str = include_str!("../../../../docs/spec/reacts-default-1.json");

/// The `default/1` palette, in palette order.
pub fn default_1() -> &'static [ReactInfo] {
    static P: OnceLock<Vec<ReactInfo>> = OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str::<PaletteFile>(JSON)
            .map(|f| f.reacts)
            .unwrap_or_default()
    })
}

/// The palette entry of `id` with its position.
pub fn react(id: &str) -> Option<(usize, &'static ReactInfo)> {
    default_1().iter().enumerate().find(|(_, r)| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_the_folds() {
        let ids: Vec<&str> = default_1().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, crate::social::full::reacts::DEFAULT_1);
        let (i, crux) = react("crux").unwrap();
        assert_eq!(i, 5);
        assert!(!crux.label.is_empty() && !crux.glyph.is_empty());
        assert!(react("nope").is_none());
    }
}
