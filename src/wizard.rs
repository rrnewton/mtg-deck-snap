//! Interactive wizard for resolving ambiguous matches and validation issues.

use crate::dck::DeckEntry;
use crate::fuzzy_match::{Confidence, MatchResult};
use crate::validation::{Warning, Severity};
use dialoguer::{Input, Select};

/// Run the interactive resolution wizard.
///
/// Mutates `matches` in place, replacing low-confidence canonical names with
/// user-confirmed values. Returns the final list of canonical card names (one
/// per occurrence, including duplicates).
pub fn resolve(
    matches: &mut [MatchResult],
    non_interactive: bool,
) -> Vec<String> {
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
pub fn resolve_count_violations(
    entries: &mut Vec<DeckEntry>,
    non_interactive: bool,
) {
    let basics: std::collections::HashSet<&str> = [
        "Plains", "Island", "Swamp", "Mountain", "Forest", "Wastes",
        "Snow-Covered Plains", "Snow-Covered Island", "Snow-Covered Swamp",
        "Snow-Covered Mountain", "Snow-Covered Forest",
    ]
    .into_iter()
    .collect();

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
