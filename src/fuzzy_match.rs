//! Fuzzy matching of AI-extracted card names against the card database.

use crate::card_db::CardDatabase;

/// Confidence tiers for a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Score < 0.70 — probably wrong.
    Low,
    /// Score 0.70 .. 0.90 — plausible but needs confirmation.
    Medium,
    /// Score 0.90 .. 1.00 — almost certainly correct.
    High,
    /// Score == 1.0 — exact match.
    Exact,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::Exact => write!(f, "exact"),
            Confidence::High => write!(f, "high"),
            Confidence::Medium => write!(f, "medium"),
            Confidence::Low => write!(f, "low"),
        }
    }
}

/// Result of matching a single extracted name.
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// The raw name as returned by the AI / OCR.
    pub extracted: String,
    /// The best canonical card name from the database.
    pub canonical: String,
    /// Similarity score (0.0 – 1.0).
    pub score: f64,
    /// Confidence tier derived from the score.
    pub confidence: Confidence,
    /// Runner-up candidates (name, score), best first.
    pub alternatives: Vec<(String, f64)>,
    /// Whether the AI flagged this with a "?" suffix.
    #[allow(dead_code)]
    pub uncertain: bool,
}

/// Match a list of extracted names against the card database.
pub fn match_all(db: &CardDatabase, extracted: &[String]) -> Vec<MatchResult> {
    extracted.iter().map(|raw| match_one(db, raw)).collect()
}

fn match_one(db: &CardDatabase, raw: &str) -> MatchResult {
    let cleaned = raw.trim();
    let uncertain = cleaned.ends_with('?');
    let query = if uncertain {
        cleaned.trim_end_matches('?').trim()
    } else {
        cleaned
    };

    // Fast path: exact match
    if let Some(canonical) = db.canonical(query) {
        return MatchResult {
            extracted: raw.to_string(),
            canonical: canonical.to_string(),
            score: 1.0,
            confidence: Confidence::Exact,
            alternatives: Vec::new(),
            uncertain,
        };
    }

    // Fuzzy search
    let candidates = db.fuzzy_match(query, 0.50);
    let (canonical, score) = candidates
        .first()
        .map(|(name, sc)| (name.clone(), *sc))
        .unwrap_or_else(|| (query.to_string(), 0.0));

    let confidence = if score >= 1.0 {
        Confidence::Exact
    } else if score >= 0.90 {
        Confidence::High
    } else if score >= 0.70 {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    let alternatives: Vec<(String, f64)> = candidates.into_iter().skip(1).take(4).collect();

    MatchResult {
        extracted: raw.to_string(),
        canonical,
        score,
        confidence,
        alternatives,
        uncertain,
    }
}
