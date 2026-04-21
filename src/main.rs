//! mtg-deck-snap — convert photographs of MTG card spreads into .dck deck files.

mod card_db;
mod dck;
mod fuzzy_match;
mod image_proc;
mod set_coherence;
mod validation;
mod vision;
mod wizard;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mtg-deck-snap", version, about = "Photograph → .dck deck list")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan one or more card-spread photos and produce a .dck file.
    Scan {
        /// Image file(s) to scan (JPEG, PNG, etc.)
        #[arg(required_unless_present = "from_list")]
        images: Vec<PathBuf>,

        /// Skip AI vision — read raw card names (one per line) from a text file instead.
        #[arg(long)]
        from_list: Option<PathBuf>,

        /// Expected deck size (e.g. 60 for Standard, 40 for Limited, 100 for Commander).
        #[arg(long)]
        deck_size: Option<u32>,

        /// Output .dck file path (default: stdout).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Deck name written into the [metadata] section.
        #[arg(long, default_value = "Deck")]
        name: String,

        /// Auto-accept best matches without interactive prompts.
        #[arg(long)]
        non_interactive: bool,

        /// Path to a Forge cardsfolder directory (uses Scryfall by default).
        #[arg(long)]
        cardsfolder: Option<PathBuf>,

        /// Run a second AI pass to re-count card copies and reconcile with pass 1.
        #[arg(long)]
        multi_pass: bool,
    },

    /// Download / refresh the Scryfall card-name database.
    UpdateDb,

    /// Search the card-name database (for debugging).
    ListDb {
        /// Substring to search for.
        #[arg(long)]
        search: String,

        /// Maximum results.
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan {
            images,
            from_list,
            deck_size,
            output,
            name,
            non_interactive,
            cardsfolder,
            multi_pass,
        } => {
            cmd_scan(
                images,
                from_list,
                deck_size,
                output,
                name,
                non_interactive,
                cardsfolder,
                multi_pass,
            )
            .await
        }
        Commands::UpdateDb => cmd_update_db().await,
        Commands::ListDb { search, limit } => cmd_list_db(search, limit).await,
    }
}

// ── scan ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn cmd_scan(
    images: Vec<PathBuf>,
    from_list: Option<PathBuf>,
    deck_size: Option<u32>,
    output: Option<PathBuf>,
    name: String,
    non_interactive: bool,
    cardsfolder: Option<PathBuf>,
    multi_pass: bool,
) -> Result<()> {
    // 1. Load card database
    let db = if let Some(cf) = cardsfolder {
        card_db::CardDatabase::load_forge(&cf)?
    } else {
        card_db::CardDatabase::load_scryfall(false).await?
    };
    eprintln!("Card database: {} names\n", db.len());

    // 2. Get raw card names + collect tiles for potential pass 2
    let (raw_names, tiles) = if let Some(list_path) = from_list {
        let content = std::fs::read_to_string(&list_path)
            .with_context(|| format!("reading list file {}", list_path.display()))?;
        let names: Vec<String> = content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        eprintln!(
            "Loaded {} raw card name(s) from {}\n",
            names.len(),
            list_path.display()
        );
        (names, Vec::new())
    } else {
        // Load and tile images
        let mut all_tiles = Vec::new();
        for path in &images {
            let tiles = image_proc::load_and_tile(path)
                .with_context(|| format!("processing image {}", path.display()))?;
            all_tiles.extend(tiles);
        }
        eprintln!("{} tile(s) to analyse\n", all_tiles.len());

        // AI vision pass 1 — extract card names
        let vision_cfg = vision::VisionConfig::from_env()?;
        let names =
            vision::extract_card_names(&vision_cfg, &all_tiles, deck_size).await?;
        eprintln!("\nAI extracted {} raw card name(s)\n", names.len());
        (names, all_tiles)
    };

    // 3. Fuzzy-match against card database
    let mut matches = fuzzy_match::match_all(&db, &raw_names);

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

    // 4. Print confidence table
    print_confidence_table(&matches);

    // 4b. Set-coherence check — flag cards from unexpected sets
    let set_index = card_db::CardDatabase::load_set_index()?;
    if set_index.len() > 0 {
        let matched_names: Vec<String> = matches.iter().map(|m| m.canonical.clone()).collect();
        let set_results = set_index.check_coherence(&matched_names);
        let outliers: Vec<_> = set_results.iter().filter(|r| r.is_outlier).collect();

        if !outliers.is_empty() {
            let majority_set = outliers[0]
                .majority_set
                .as_deref()
                .unwrap_or("unknown");
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
                        eprintln!(
                            "    Downgrading confidence: {} → low",
                            m.confidence
                        );
                        m.confidence = fuzzy_match::Confidence::Low;
                    }
                }
            }
            eprintln!();
        }
    }

    // 5. Interactive wizard for ambiguous matches
    let card_names = wizard::resolve(&mut matches, non_interactive);

    // Filter out empty names (skipped cards)
    let card_names: Vec<String> = card_names.into_iter().filter(|n| !n.is_empty()).collect();

    // 6. Build deck list
    let mut deck = dck::DeckList::from_card_names(&card_names);

    // 7. Multi-pass count verification
    if multi_pass && !tiles.is_empty() {
        eprintln!("\n── Pass 2: count verification ──\n");
        let vision_cfg = vision::VisionConfig::from_env()?;
        let unique_names: Vec<String> = deck
            .main_deck
            .iter()
            .map(|e| e.card_name.clone())
            .collect();

        let recounts = vision::verify_counts(&vision_cfg, &tiles, &unique_names).await?;

        // Reconcile: compare pass-1 counts with pass-2 counts
        reconcile_counts(&mut deck, &recounts);
    }

    // 8. Land count sanity check
    if let Some(expected) = deck_size {
        wizard::resolve_land_counts(&mut deck.main_deck, expected, non_interactive);
    }

    // 9. Validation
    let warnings = validation::validate(&deck.main_deck, deck_size);
    wizard::resolve_count_violations(&mut deck.main_deck, non_interactive);

    if !wizard::resolve_warnings(&warnings, non_interactive) {
        eprintln!("Aborted.");
        return Ok(());
    }

    // 10. Output
    let dck_content = deck.to_dck_format(Some(&name));

    if let Some(path) = output {
        deck.save(&path, Some(&name))?;
        eprintln!(
            "\n✓ Wrote {} cards ({} unique) to {}",
            deck.total_cards(),
            deck.main_deck.len(),
            path.display(),
        );
    } else {
        print!("{}", dck_content);
    }

    Ok(())
}

/// Print a table of all matched cards with confidence info.
fn print_confidence_table(matches: &[fuzzy_match::MatchResult]) {
    // Deduplicate: group by (canonical, extracted) to avoid printing
    // the same exact match dozens of times for basic lands, etc.
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    for m in matches {
        let key = (m.canonical.as_str(), m.extracted.as_str(), "");
        *seen.entry(key).or_insert(0) += 1;
    }

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
    let mut printed: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
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
            || m.extracted.trim_end_matches('?').trim().eq_ignore_ascii_case(&m.canonical);
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
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

/// Reconcile pass-1 counts with pass-2 recount.
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
                    pass2_count.min(entry.count), // conservative: take the lower
                );
                entry.count = pass2_count.min(entry.count);
            }
        }
    }

    // Remove zero-count entries
    deck.main_deck.retain(|e| e.count > 0);
    eprintln!();
}

// ── update-db ───────────────────────────────────────────────────────

async fn cmd_update_db() -> Result<()> {
    card_db::CardDatabase::load_scryfall(true).await?;
    eprintln!("Database updated.");
    Ok(())
}

// ── list-db ─────────────────────────────────────────────────────────

async fn cmd_list_db(search: String, limit: usize) -> Result<()> {
    let db = card_db::CardDatabase::load_scryfall(false).await?;
    let hits = db.search(&search, limit);
    if hits.is_empty() {
        eprintln!("No matches for \"{}\"", search);
    } else {
        for name in &hits {
            println!("{}", name);
        }
        if hits.len() == limit {
            eprintln!("(showing first {} results)", limit);
        }
    }
    Ok(())
}
