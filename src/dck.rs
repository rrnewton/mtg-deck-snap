//! .dck file generation (Forge INI-style format).

use std::collections::BTreeMap;
use std::path::Path;

/// A single deck entry: count + card name.
#[derive(Debug, Clone)]
pub struct DeckEntry {
    pub card_name: String,
    pub count: u8,
}

/// A complete deck list.
#[derive(Debug, Clone)]
pub struct DeckList {
    pub main_deck: Vec<DeckEntry>,
    pub sideboard: Vec<DeckEntry>,
}

impl DeckList {
    /// Build a `DeckList` from a flat list of card names (one per occurrence).
    ///
    /// Consolidates duplicates and sorts by count descending, then alphabetical.
    pub fn from_card_names(names: &[String]) -> Self {
        let mut counts: BTreeMap<String, u8> = BTreeMap::new();
        for name in names {
            let entry = counts.entry(name.clone()).or_insert(0);
            *entry = entry.saturating_add(1);
        }

        let mut entries: Vec<DeckEntry> = counts
            .into_iter()
            .map(|(card_name, count)| DeckEntry { card_name, count })
            .collect();

        // Sort: highest count first, then alphabetical
        entries.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.card_name.cmp(&b.card_name))
        });

        DeckList {
            main_deck: entries,
            sideboard: Vec::new(),
        }
    }

    /// Total number of cards in the main deck.
    pub fn total_cards(&self) -> usize {
        self.main_deck.iter().map(|e| e.count as usize).sum()
    }

    /// Format as .dck file content.
    pub fn to_dck_format(&self, name: Option<&str>) -> String {
        let mut content = String::new();

        content.push_str("[metadata]\n");
        content.push_str(&format!("Name={}\n", name.unwrap_or("Deck")));

        content.push_str("\n[Main]\n");
        for entry in &self.main_deck {
            content.push_str(&format!("{} {}\n", entry.count, entry.card_name));
        }

        if !self.sideboard.is_empty() {
            content.push_str("\n[Sideboard]\n");
            for entry in &self.sideboard {
                content.push_str(&format!("{} {}\n", entry.count, entry.card_name));
            }
        }

        content
    }

    /// Write the deck to a `.dck` file on disk.
    pub fn save(&self, path: &Path, name: Option<&str>) -> anyhow::Result<()> {
        let content = self.to_dck_format(name);
        std::fs::write(path, &content)?;
        Ok(())
    }
}
