//! Shared deck-building pipeline used by both `scan` and `batch`.
//!
//! Given raw card names (from AI vision or a text list) plus the optional image
//! tiles and vision backend needed for multi-pass recounting, this runs the
//! common stages — fuzzy matching, set-coherence outlier detection, the
//! resolution wizard, optional multi-pass count reconciliation, land-count
//! sanity, and validation — and returns the finished deck together with the
//! match table and validation warnings so callers can render output and/or
//! serialize machine-readable metadata.

use crate::card_db::CardDatabase;
use crate::image_proc::Tile;
use crate::vision::VisionBackend;
use crate::{dck, fuzzy_match, validation, wizard};
use anyhow::Result;

/// The fully-resolved result of running the deck-building pipeline.
pub struct DeckResult {
    /// The finished deck list.
    pub deck: dck::DeckList,
    /// Every fuzzy match, with final (post-resolution) confidence tiers.
    pub matches: Vec<fuzzy_match::MatchResult>,
    /// Validation warnings computed on the resolved deck.
    pub warnings: Vec<validation::Warning>,
    /// Whether the wizard indicated we should proceed to output.
    pub proceed: bool,
}

/// Run the shared matching → wizard → validation pipeline.
///
/// `tiles` and `vision_backend` are only needed when `multi_pass` is set (the
/// second recount pass re-reads the same tiles); pass empties / `None`
/// otherwise.
pub async fn process(
    db: &CardDatabase,
    raw_names: &[String],
    tiles: &[Tile],
    vision_backend: Option<&dyn VisionBackend>,
    deck_size: Option<u32>,
    non_interactive: bool,
    multi_pass: bool,
) -> Result<DeckResult> {
    // 1. Fuzzy-match against card database
    let mut matches = fuzzy_match::match_all(db, raw_names);

    // Print confidence summary
    let exact = matches
        .iter()
        .filter(|m| m.confidence == fuzzy_match::Confidence::Exact)
        .count();
    let high = matches
        .iter()
        .filter(|m| m.confidence == fuzzy_match::Confidence::High)
        .count();
    let med = matches
        .iter()
        .filter(|m| m.confidence == fuzzy_match::Confidence::Medium)
        .count();
    let low = matches
        .iter()
        .filter(|m| m.confidence == fuzzy_match::Confidence::Low)
        .count();
    eprintln!(
        "Match confidence: {} exact, {} high, {} medium, {} low\n",
        exact, high, med, low
    );

    // 2. Print confidence table
    print_confidence_table(&matches);

    // 3. Set-coherence check — flag cards from unexpected sets
    let set_index = CardDatabase::load_set_index()?;
    if set_index.len() > 0 {
        let matched_names: Vec<String> = matches.iter().map(|m| m.canonical.clone()).collect();
        let set_results = set_index.check_coherence(&matched_names);
        let outliers: Vec<_> = set_results.iter().filter(|r| r.is_outlier).collect();

        if !outliers.is_empty() {
            let majority_set = outliers[0].majority_set.as_deref().unwrap_or("unknown");
            eprintln!("\n── Set coherence check ─────────────────────────");
            eprintln!("  Majority set: {}", majority_set);
            for o in &outliers {
                let set_name = o
                    .card_set
                    .as_ref()
                    .map(|s| s.set_name.as_str())
                    .unwrap_or("unknown");
                eprintln!(
                    "  ⚠ \"{}\" is from \"{}\" — possible false positive",
                    o.card_name, set_name
                );

                // Downgrade confidence for outlier matches
                if let Some(m) = matches.iter_mut().find(|m| m.canonical == o.card_name) {
                    if m.confidence != fuzzy_match::Confidence::Exact {
                        eprintln!("    Downgrading confidence: {} → low", m.confidence);
                        m.confidence = fuzzy_match::Confidence::Low;
                    }
                }
            }
            eprintln!();
        }
    }

    // 4. Interactive wizard for ambiguous matches (no-op in non-interactive mode)
    let card_names = wizard::resolve(&mut matches, non_interactive);
    let card_names: Vec<String> = card_names.into_iter().filter(|n| !n.is_empty()).collect();

    // 5. Build deck list
    let mut deck = dck::DeckList::from_card_names(&card_names);

    // 6. Multi-pass count verification
    if multi_pass && !tiles.is_empty() {
        eprintln!("\n── Pass 2: count verification ──\n");
        let be = vision_backend.expect("vision backend is required for multi-pass recount");
        let unique_names: Vec<String> =
            deck.main_deck.iter().map(|e| e.card_name.clone()).collect();
        let recounts = be.verify_counts(tiles, &unique_names).await?;
        reconcile_counts(&mut deck, &recounts);
    }

    // 7. Land count sanity check
    if let Some(expected) = deck_size {
        wizard::resolve_land_counts(&mut deck.main_deck, expected, non_interactive);
    }

    // 8. Validation (computed before capping, matching original scan behaviour)
    let warnings = validation::validate(&deck.main_deck, deck_size);
    wizard::resolve_count_violations(&mut deck.main_deck, non_interactive);
    let proceed = wizard::resolve_warnings(&warnings, non_interactive);

    Ok(DeckResult {
        deck,
        matches,
        warnings,
        proceed,
    })
}

/// Print a table of all matched cards with confidence info.
pub fn print_confidence_table(matches: &[fuzzy_match::MatchResult]) {
    eprintln!("── Match details ──────────────────────────────────────────────────────────────");
    let hdr_name = "Card Name";
    let hdr_ext = "Extracted As";
    let hdr_score = "Score";
    let hdr_conf = "Conf";
    let hdr_qty = "Qty";
    eprintln!(
        "  {:<35} {:<25} {:>5}  {:<8}  {}",
        hdr_name, hdr_ext, hdr_score, hdr_conf, hdr_qty
    );
    let sep = "───";
    eprintln!(
        "  {:<35} {:<25} {:>5}  {:<8}  {}",
        "─".repeat(35),
        "─".repeat(25),
        "─".repeat(5),
        "─".repeat(8),
        sep
    );

    // Build unique entries preserving first-seen order
    let mut printed: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for m in matches {
        let key = (m.canonical.clone(), m.extracted.clone());
        if printed.contains(&key) {
            continue;
        }
        printed.insert(key.clone());

        let qty = matches
            .iter()
            .filter(|m2| m2.canonical == m.canonical && m2.extracted == m.extracted)
            .count();

        let same = m.extracted == m.canonical
            || m.extracted
                .trim_end_matches('?')
                .trim()
                .eq_ignore_ascii_case(&m.canonical);
        let extracted_display = if same {
            "=".to_string()
        } else {
            truncate_str(&m.extracted, 25)
        };

        eprintln!(
            "  {:<35} {:<25} {:>5.2}  {:<8}  {}",
            truncate_str(&m.canonical, 35),
            extracted_display,
            m.score,
            m.confidence,
            qty,
        );
    }
    eprintln!();
}

/// Truncate a string to `max_len` chars, adding "…" if truncated.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

/// Reconcile pass-1 counts with pass-2 recount (conservative — lower wins).
fn reconcile_counts(deck: &mut dck::DeckList, recounts: &[(String, u8)]) {
    use std::collections::HashMap;
    let recount_map: HashMap<String, u8> = recounts
        .iter()
        .map(|(n, c)| (n.to_lowercase(), *c))
        .collect();

    eprintln!("── Reconciling pass 1 vs pass 2 ──\n");

    for entry in &mut deck.main_deck {
        let key = entry.card_name.to_lowercase();
        if let Some(&pass2_count) = recount_map.get(&key) {
            if pass2_count != entry.count {
                eprintln!(
                    "  {} : pass1={}, pass2={} → using {}",
                    entry.card_name,
                    entry.count,
                    pass2_count,
                    pass2_count.min(entry.count),
                );
                entry.count = pass2_count.min(entry.count);
            }
        }
    }

    deck.main_deck.retain(|e| e.count > 0);
    eprintln!();
}
