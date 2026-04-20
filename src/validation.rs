//! Deck validation: legality checks and sanity warnings.

use crate::dck::DeckEntry;
use std::collections::HashSet;

/// A single validation warning.
#[derive(Debug, Clone)]
pub struct Warning {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    #[allow(dead_code)]
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Warn => write!(f, "WARN"),
            Severity::Error => write!(f, "ERROR"),
        }
    }
}

/// Cards that are exempt from the 4-of rule.
fn basic_lands() -> HashSet<&'static str> {
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

/// Validate a list of deck entries.
pub fn validate(entries: &[DeckEntry], expected_size: Option<u32>) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let basics = basic_lands();

    // 4-of rule
    for entry in entries {
        if entry.count > 4 && !basics.contains(entry.card_name.as_str()) {
            warnings.push(Warning {
                severity: Severity::Error,
                message: format!(
                    "{} × {} — exceeds the 4-of limit (non-basic card)",
                    entry.count, entry.card_name,
                ),
            });
        }
    }

    // Total card count
    let total: u32 = entries.iter().map(|e| u32::from(e.count)).sum();
    if let Some(expected) = expected_size {
        if total != expected {
            let sev = if total.abs_diff(expected) <= 2 {
                Severity::Warn
            } else {
                Severity::Error
            };
            warnings.push(Warning {
                severity: sev,
                message: format!(
                    "Deck has {} cards but expected {} (difference: {})",
                    total,
                    expected,
                    if total > expected {
                        format!("+{}", total - expected)
                    } else {
                        format!("-{}", expected - total)
                    },
                ),
            });
        }
    }

    // Very small or very large deck warning
    if total < 30 {
        warnings.push(Warning {
            severity: Severity::Warn,
            message: format!("Only {} total cards detected — possible missed cards", total),
        });
    }
    if total > 120 {
        warnings.push(Warning {
            severity: Severity::Warn,
            message: format!(
                "{} total cards — unusually large, possible duplicate reads across tiles",
                total
            ),
        });
    }

    warnings
}
