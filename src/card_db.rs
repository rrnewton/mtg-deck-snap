//! Card database: loads canonical MTG card names from Scryfall or a Forge cardsfolder.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Canonical card database used for fuzzy-matching OCR / AI-extracted names.
pub struct CardDatabase {
    /// Every canonical card name, stored in its original mixed-case form.
    names: Vec<String>,
    /// Lower-cased set for fast exact lookups.
    lower_set: HashSet<String>,
}

// ── Scryfall helpers ────────────────────────────────────────────────

/// Metadata returned by <https://api.scryfall.com/bulk-data/oracle-cards>.
#[derive(serde::Deserialize)]
struct BulkDataMeta {
    download_uri: String,
}

/// Minimal card object – we only need the `name` field.
#[derive(serde::Deserialize)]
struct ScryfallCard {
    name: String,
    /// Layout helps us decide whether to keep the full double-faced name.
    #[serde(default)]
    layout: String,
}

// All public API is intentional — `contains` is available for downstream use.
#[allow(dead_code)]
impl CardDatabase {
    // ── constructors ────────────────────────────────────────────────

    /// Build a database from a set of names.
    pub fn from_names(names: Vec<String>) -> Self {
        let lower_set: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        Self { names, lower_set }
    }

    /// Load from the local Scryfall cache, downloading if missing or forced.
    pub async fn load_scryfall(force_refresh: bool) -> Result<Self> {
        let cache_path = Self::cache_path()?;

        if !force_refresh && cache_path.exists() {
            let data = std::fs::read_to_string(&cache_path)
                .context("reading cached Scryfall names")?;
            let names: Vec<String> =
                serde_json::from_str(&data).context("parsing cached Scryfall names")?;
            eprintln!("Loaded {} card names from cache", names.len());
            return Ok(Self::from_names(names));
        }

        Self::download_scryfall().await
    }

    /// Download fresh card names from Scryfall and persist to cache.
    async fn download_scryfall() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("mtg-deck-snap/0.1 (https://github.com)")
            .build()?;

        eprintln!("Fetching Scryfall bulk-data metadata…");
        let meta: BulkDataMeta = client
            .get("https://api.scryfall.com/bulk-data/oracle-cards")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        eprintln!("Downloading oracle card data…");
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("valid template"),
        );
        pb.set_message("downloading cards…");

        let body = client
            .get(&meta.download_uri)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        pb.finish_with_message("download complete");

        let cards: Vec<ScryfallCard> =
            serde_json::from_slice(&body).context("parsing Scryfall oracle cards JSON")?;

        // Collect unique names. For double-faced / split cards the `name` field
        // is "Front // Back". We store both the full name and each half so
        // fuzzy-matching can find either.
        let mut name_set: HashSet<String> = HashSet::with_capacity(cards.len() * 2);
        for card in &cards {
            name_set.insert(card.name.clone());
            if card.layout == "transform"
                || card.layout == "modal_dfc"
                || card.layout == "split"
                || card.layout == "adventure"
                || card.layout == "flip"
            {
                for half in card.name.split(" // ") {
                    name_set.insert(half.trim().to_string());
                }
            }
        }

        let mut names: Vec<String> = name_set.into_iter().collect();
        names.sort();

        // Persist to cache
        let cache_path = Self::cache_path()?;
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(&names)?;
        std::fs::write(&cache_path, json)?;
        eprintln!("Cached {} unique card names to {}", names.len(), cache_path.display());

        Ok(Self::from_names(names))
    }

    /// Load names from a Forge-style `cardsfolder/` directory.
    pub fn load_forge(cardsfolder: &Path) -> Result<Self> {
        anyhow::ensure!(
            cardsfolder.is_dir(),
            "cardsfolder path {} is not a directory",
            cardsfolder.display()
        );

        let mut names = Vec::new();
        Self::walk_cardsfolder(cardsfolder, &mut names)?;
        names.sort();
        names.dedup();
        eprintln!("Loaded {} card names from {}", names.len(), cardsfolder.display());
        Ok(Self::from_names(names))
    }

    fn walk_cardsfolder(dir: &Path, out: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                Self::walk_cardsfolder(&entry.path(), out)?;
            } else if ft.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e == "txt")
            {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for line in content.lines() {
                        if let Some(name) = line.strip_prefix("Name=") {
                            let name = name.trim();
                            if !name.is_empty() {
                                out.push(name.to_string());
                            }
                            break; // only first Name= per file
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // ── queries ─────────────────────────────────────────────────────

    /// Exact match (case-insensitive).
    pub fn contains(&self, name: &str) -> bool {
        self.lower_set.contains(&name.to_lowercase())
    }

    /// Return the canonical casing if present.
    pub fn canonical(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.names
            .iter()
            .find(|n| n.to_lowercase() == lower)
            .map(|s| s.as_str())
    }

    /// Number of cards in the database.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Search by substring (case-insensitive). Returns up to `limit` hits.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&str> {
        let q = query.to_lowercase();
        self.names
            .iter()
            .filter(|n| n.to_lowercase().contains(&q))
            .take(limit)
            .map(|s| s.as_str())
            .collect()
    }

    /// Fuzzy-match `query` against the database.
    ///
    /// Returns all candidates with similarity ≥ `threshold`, sorted best-first.
    pub fn fuzzy_match(&self, query: &str, threshold: f64) -> Vec<(String, f64)> {
        let q = query.to_lowercase();
        let mut results: Vec<(String, f64)> = self
            .names
            .iter()
            .filter_map(|canonical| {
                let score = combined_similarity(&q, &canonical.to_lowercase());
                if score >= threshold {
                    Some((canonical.clone(), score))
                } else {
                    None
                }
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    // ── internals ───────────────────────────────────────────────────

    fn cache_path() -> Result<PathBuf> {
        let cache_dir = dirs::cache_dir()
            .context("could not determine cache directory")?
            .join("mtg-deck-snap");
        Ok(cache_dir.join("scryfall-names.json"))
    }
}

/// Combined similarity score: 70 % Jaro-Winkler + 30 % normalised Levenshtein.
fn combined_similarity(a: &str, b: &str) -> f64 {
    let jw = strsim::jaro_winkler(a, b);
    let max_len = a.len().max(b.len());
    let norm_lev = if max_len == 0 {
        1.0
    } else {
        1.0 - (strsim::levenshtein(a, b) as f64 / max_len as f64)
    };
    0.70 * jw + 0.30 * norm_lev
}
