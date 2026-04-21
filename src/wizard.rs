//! Interactive wizard for resolving ambiguous matches and validation issues.

use crate::dck::DeckEntry;
use crate::fuzzy_match::{Confidence, MatchResult};
use crate::validation::{Severity, Warning};
use dialoguer::{Input, Select};
use std::collections::HashSet;

/// Basic land names that are exempt from the 4-of rule and are commonly stacked.
fn basic_land_names() -> HashSet<&'static str> {
    [
        "Plains",
        "Island",
        "Swamp",
        "Mountain",
        "Forest",
        "Wastes",
        "Snow-Covered Plains",
        "Snow-Covered Island",
        "Snow-Covered Swamp",
        "Snow-Covered Mountain",
        "Snow-Covered Forest",
    ]
    .into_iter()
    .collect()
}

/// Run the interactive resolution wizard.
///
/// Mutates `matches` in place, replacing low-confidence canonical names with
/// user-confirmed values. Returns the final list of canonical card names (one
/// per occurrence, including duplicates).
pub fn resolve(matches: &mut [MatchResult], non_interactive: bool) -> Vec<String> {
    if !non_interactive {
        resolve_low_confidence(matches);
    }

    // Build flat name list from confirmed matches
    matches.iter().map(|m| m.canonical.clone()).collect()
}

/// Prompt the user to confirm or correct low-confidence matches.
fn resolve_low_confidence(matches: &mut [MatchResult]) {
    let needs_review: usize = matches
        .iter()
        .filter(|m| m.confidence <= Confidence::Medium)
        .count();

    if needs_review == 0 {
        return;
    }

    eprintln!(
        "\n── {} card(s) need review ──────────────────────\n",
        needs_review
    );

    for m in matches.iter_mut() {
        if m.confidence > Confidence::Medium {
            continue;
        }

        eprintln!(
            "  Extracted: \"{}\"  →  best match: \"{}\" (score: {:.2}, {})",
            m.extracted, m.canonical, m.score, m.confidence,
        );

        // Build selection list
        let mut options: Vec<String> = Vec::new();
        options.push(format!("✓  Accept \"{}\"", m.canonical));
        for (alt, sc) in &m.alternatives {
            options.push(format!("   \"{}\" ({:.2})", alt, sc));
        }
        options.push("✎  Type correct name manually".to_string());
        options.push("✗  Skip this card".to_string());

        let selection = Select::new()
            .with_prompt("Choose")
            .items(&options)
            .default(0)
            .interact_opt();

        match selection {
            Ok(Some(idx)) => {
                if idx == 0 {
                    // Accept current best
                } else if idx <= m.alternatives.len() {
                    let (alt, _) = &m.alternatives[idx - 1];
                    m.canonical = alt.clone();
                    m.confidence = Confidence::Exact; // user confirmed
                } else if idx == m.alternatives.len() + 1 {
                    // Manual entry
                    if let Ok(name) = Input::<String>::new()
                        .with_prompt("Enter correct card name")
                        .interact_text()
                    {
                        m.canonical = name;
                        m.confidence = Confidence::Exact;
                    }
                } else {
                    // Skip
                    m.canonical = String::new();
                }
            }
            _ => {
                // Ctrl-C or error → accept default
            }
        }

        eprintln!();
    }
}

/// Interactive resolution for validation warnings.
///
/// Returns `true` if the user wants to proceed despite warnings.
pub fn resolve_warnings(warnings: &[Warning], non_interactive: bool) -> bool {
    if warnings.is_empty() {
        return true;
    }

    eprintln!("\n── Validation warnings ─────────────────────────\n");
    for w in warnings {
        eprintln!("  [{}] {}", w.severity, w.message);
    }
    eprintln!();

    if non_interactive {
        let has_errors = warnings.iter().any(|w| w.severity == Severity::Error);
        if has_errors {
            eprintln!("  Non-interactive mode: proceeding despite errors.");
        }
        return true;
    }

    let proceed = dialoguer::Confirm::new()
        .with_prompt("Proceed with output?")
        .default(true)
        .interact()
        .unwrap_or(true);

    proceed
}

/// Interactive resolution for 4-of violations.
///
/// Mutates entries in-place, capping counts if the user agrees.
pub fn resolve_count_violations(entries: &mut [DeckEntry], non_interactive: bool) {
    let basics = basic_land_names();

    for entry in entries.iter_mut() {
        if entry.count > 4 && !basics.contains(entry.card_name.as_str()) {
            if non_interactive {
                eprintln!(
                    "  Auto-capping {} × {} → 4",
                    entry.count, entry.card_name
                );
                entry.count = 4;
            } else {
                eprintln!(
                    "\n  ⚠  {} × {} — exceeds the 4-of limit",
                    entry.count, entry.card_name
                );
                let capped = dialoguer::Confirm::new()
                    .with_prompt(format!("Cap \"{}\" at 4 copies?", entry.card_name))
                    .default(true)
                    .interact()
                    .unwrap_or(true);
                if capped {
                    entry.count = 4;
                }
            }
        }
    }
}

/// Land count sanity check.
///
/// If the deck size is known, check whether the number of basic lands makes sense.
/// Stacked/fanned lands are the most common source of miscounts.
pub fn resolve_land_counts(
    entries: &mut [DeckEntry],
    expected_size: u32,
    non_interactive: bool,
) {
    let basics = basic_land_names();

    let land_count: u32 = entries
        .iter()
        .filter(|e| basics.contains(e.card_name.as_str()))
        .map(|e| u32::from(e.count))
        .sum();

    let non_land_count: u32 = entries
        .iter()
        .filter(|e| !basics.contains(e.card_name.as_str()))
        .map(|e| u32::from(e.count))
        .sum();

    let total = land_count + non_land_count;

    // Only flag if we're over the expected count and it's the lands causing it
    if total <= expected_size || land_count == 0 {
        return;
    }

    let suggested_lands = expected_size.saturating_sub(non_land_count);

    // Only flag if the AI's land count is meaningfully different from what
    // we'd expect. A difference of 1 is within normal tolerance.
    if land_count.abs_diff(suggested_lands) <= 1 {
        return;
    }

    eprintln!("\n── Land count check ────────────────────────────\n");
    eprintln!(
        "  AI detected {} basic lands + {} spells = {} total",
        land_count, non_land_count, total
    );
    eprintln!(
        "  Expected deck size: {} → suggests {} basic lands",
        expected_size, suggested_lands
    );

    if non_interactive {
        // In non-interactive mode, adjust land counts proportionally
        eprintln!(
            "  Auto-adjusting land count: {} → {}",
            land_count, suggested_lands
        );
        adjust_land_counts(entries, &basics, suggested_lands);
    } else {
        let options = vec![
            format!(
                "Adjust to {} lands (match expected deck size of {})",
                suggested_lands, expected_size
            ),
            format!("Keep {} lands as detected by AI", land_count),
            "Enter land counts manually".to_string(),
        ];

        let selection = Select::new()
            .with_prompt("How should we handle the land count?")
            .items(&options)
            .default(0)
            .interact_opt();

        match selection {
            Ok(Some(0)) => {
                adjust_land_counts(entries, &basics, suggested_lands);
            }
            Ok(Some(2)) => {
                // Manual: ask for each basic land type
                for entry in entries.iter_mut() {
                    if basics.contains(entry.card_name.as_str()) && entry.count > 0 {
                        if let Ok(new_count) = Input::<u8>::new()
                            .with_prompt(format!(
                                "{} (currently {})",
                                entry.card_name, entry.count
                            ))
                            .default(entry.count)
                            .interact_text()
                        {
                            entry.count = new_count;
                        }
                    }
                }
            }
            _ => {
                // Keep as-is
            }
        }
    }
    eprintln!();
}

/// Proportionally scale basic land counts to hit a target total.
fn adjust_land_counts(
    entries: &mut [DeckEntry],
    basics: &HashSet<&str>,
    target_total: u32,
) {
    let current_total: u32 = entries
        .iter()
        .filter(|e| basics.contains(e.card_name.as_str()))
        .map(|e| u32::from(e.count))
        .sum();

    if current_total == 0 || target_total == 0 {
        return;
    }

    let scale = target_total as f64 / current_total as f64;

    // Proportionally scale, then distribute rounding error
    let mut new_counts: Vec<(usize, f64)> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if basics.contains(entry.card_name.as_str()) && entry.count > 0 {
            new_counts.push((i, f64::from(entry.count) * scale));
        }
    }

    // Floor everything, then distribute remainder to largest fractional parts
    let mut floored: Vec<(usize, u32, f64)> = new_counts
        .iter()
        .map(|(i, v)| (*i, v.floor() as u32, v.fract()))
        .collect();

    let floored_sum: u32 = floored.iter().map(|(_, v, _)| v).sum();
    let mut remainder = target_total.saturating_sub(floored_sum);

    // Sort by fractional part descending to distribute remainder fairly
    floored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    for (idx, count, _) in &mut floored {
        if remainder == 0 {
            break;
        }
        *count += 1;
        remainder -= 1;
        let _ = idx; // used below
    }

    // Apply
    for (idx, count, _) in &floored {
        if let Some(entry) = entries.get_mut(*idx) {
            entry.count = *count as u8;
        }
    }
}
