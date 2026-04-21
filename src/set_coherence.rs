//! Set-coherence heuristic: detect outlier cards from unexpected sets.
//!
//! In a draft/sealed deck, nearly all cards come from the same set (or a small
//! number of related sets). A matched card from a completely unrelated set is
//! likely a false positive. This module detects and flags such outliers.

use std::collections::HashMap;

/// Per-card set information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardSetInfo {
    /// Scryfall set code (e.g. "tla" for Avatar: The Last Airbender).
    pub set_code: String,
    /// Human-readable set name (e.g. "Avatar: The Last Airbender").
    pub set_name: String,
}

/// Set-coherence index: maps canonical card names (lowercased) to their set info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetIndex {
    cards: HashMap<String, CardSetInfo>,
}

/// Result of the set-coherence check for a single card.
#[derive(Debug, Clone)]
pub struct SetCheckResult {
    pub card_name: String,
    pub card_set: Option<CardSetInfo>,
    pub is_outlier: bool,
    /// The majority set of the deck (if determined).
    pub majority_set: Option<String>,
}

impl SetIndex {
    /// Build from a list of (name, set_code, set_name) tuples.
    pub fn from_entries(entries: Vec<(String, String, String)>) -> Self {
        let mut cards = HashMap::with_capacity(entries.len());
        for (name, set_code, set_name) in entries {
            cards.insert(
                name.to_lowercase(),
                CardSetInfo { set_code, set_name },
            );
        }
        Self { cards }
    }

    /// Look up set info for a card name (case-insensitive).
    pub fn get(&self, card_name: &str) -> Option<&CardSetInfo> {
        self.cards.get(&card_name.to_lowercase())
    }

    /// Determine the majority set among a list of card names.
    ///
    /// Ignores basic lands (they appear in every set). Returns the set code
    /// that appears most frequently, or `None` if no clear majority.
    pub fn majority_set(&self, card_names: &[String]) -> Option<(String, String, usize)> {
        let basics: std::collections::HashSet<&str> = [
            "plains", "island", "swamp", "mountain", "forest", "wastes",
        ]
        .into_iter()
        .collect();

        let mut set_counts: HashMap<String, (String, usize)> = HashMap::new();
        for name in card_names {
            if basics.contains(name.to_lowercase().as_str()) {
                continue;
            }
            if let Some(info) = self.get(name) {
                let entry = set_counts
                    .entry(info.set_code.clone())
                    .or_insert_with(|| (info.set_name.clone(), 0));
                entry.1 += 1;
            }
        }

        set_counts
            .into_iter()
            .max_by_key(|(_, (_, count))| *count)
            .map(|(code, (name, count))| (code, name, count))
    }

    /// Check each card for set coherence against the majority set.
    pub fn check_coherence(&self, card_names: &[String]) -> Vec<SetCheckResult> {
        let majority = self.majority_set(card_names);
        let majority_code = majority.as_ref().map(|(code, _, _)| code.clone());
        let majority_name = majority.as_ref().map(|(_, name, _)| name.clone());

        let basics: std::collections::HashSet<&str> = [
            "plains", "island", "swamp", "mountain", "forest", "wastes",
        ]
        .into_iter()
        .collect();

        card_names
            .iter()
            .map(|name| {
                let card_set = self.get(name).cloned();
                let is_outlier = if basics.contains(name.to_lowercase().as_str()) {
                    false // basic lands are never outliers
                } else if let (Some(ref info), Some(ref maj)) = (&card_set, &majority_code) {
                    info.set_code != *maj
                } else {
                    false // can't determine without set data
                };

                SetCheckResult {
                    card_name: name.clone(),
                    card_set,
                    is_outlier,
                    majority_set: majority_name.clone(),
                }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> SetIndex {
        SetIndex::from_entries(vec![
            ("Meteor Sword".into(), "tla".into(), "Avatar: The Last Airbender".into()),
            ("Otter-Penguin".into(), "tla".into(), "Avatar: The Last Airbender".into()),
            ("Giant Koi".into(), "tla".into(), "Avatar: The Last Airbender".into()),
            ("Turtle-Duck".into(), "tla".into(), "Avatar: The Last Airbender".into()),
            ("Gran-Gran".into(), "tla".into(), "Avatar: The Last Airbender".into()),
            ("Mist Leopard".into(), "m10".into(), "Magic 2010".into()),
            ("Lightning Bolt".into(), "leb".into(), "Limited Edition Beta".into()),
            ("Island".into(), "tla".into(), "Avatar: The Last Airbender".into()),
        ])
    }

    #[test]
    fn test_majority_set_detected() {
        let idx = test_index();
        let names: Vec<String> = vec![
            "Meteor Sword", "Otter-Penguin", "Giant Koi", "Turtle-Duck",
            "Gran-Gran", "Island", "Island",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let (code, name, count) = idx.majority_set(&names).unwrap();
        assert_eq!(code, "tla");
        assert_eq!(name, "Avatar: The Last Airbender");
        assert_eq!(count, 5); // excludes Islands
    }

    #[test]
    fn test_outlier_detection() {
        let idx = test_index();
        let names: Vec<String> = vec![
            "Meteor Sword", "Otter-Penguin", "Giant Koi",
            "Turtle-Duck", "Mist Leopard", "Island",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let results = idx.check_coherence(&names);
        let outliers: Vec<_> = results.iter().filter(|r| r.is_outlier).collect();
        assert_eq!(outliers.len(), 1);
        assert_eq!(outliers[0].card_name, "Mist Leopard");
        assert_eq!(
            outliers[0].card_set.as_ref().unwrap().set_name,
            "Magic 2010"
        );
    }

    #[test]
    fn test_basic_lands_not_outliers() {
        let idx = test_index();
        let names: Vec<String> = vec!["Island", "Meteor Sword", "Otter-Penguin"]
            .into_iter()
            .map(String::from)
            .collect();

        let results = idx.check_coherence(&names);
        assert!(results.iter().all(|r| !r.is_outlier));
    }
}
