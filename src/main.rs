//! mtg-deck-snap — convert photographs of MTG card spreads into .dck deck files.

mod card_db;
mod dck;
mod fuzzy_match;
mod image_proc;
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
        } => cmd_scan(images, from_list, deck_size, output, name, non_interactive, cardsfolder).await,
        Commands::UpdateDb => cmd_update_db().await,
        Commands::ListDb { search, limit } => cmd_list_db(search, limit).await,
    }
}

// ── scan ────────────────────────────────────────────────────────────

async fn cmd_scan(
    images: Vec<PathBuf>,
    from_list: Option<PathBuf>,
    deck_size: Option<u32>,
    output: Option<PathBuf>,
    name: String,
    non_interactive: bool,
    cardsfolder: Option<PathBuf>,
) -> Result<()> {
    // 1. Load card database
    let db = if let Some(cf) = cardsfolder {
        card_db::CardDatabase::load_forge(&cf)?
    } else {
        card_db::CardDatabase::load_scryfall(false).await?
    };
    eprintln!("Card database: {} names\n", db.len());

    // 2. Get raw card names — either from a text file or via AI vision
    let raw_names = if let Some(list_path) = from_list {
        let content = std::fs::read_to_string(&list_path)
            .with_context(|| format!("reading list file {}", list_path.display()))?;
        let names: Vec<String> = content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        eprintln!("Loaded {} raw card name(s) from {}\n", names.len(), list_path.display());
        names
    } else {
        // Load and tile images
        let mut all_tiles = Vec::new();
        for path in &images {
            let tiles = image_proc::load_and_tile(path)
                .with_context(|| format!("processing image {}", path.display()))?;
            all_tiles.extend(tiles);
        }
        eprintln!("{} tile(s) to analyse\n", all_tiles.len());

        // AI vision — extract card names from tiles
        let vision_cfg = vision::VisionConfig::from_env()?;
        let names =
            vision::extract_card_names(&vision_cfg, &all_tiles, deck_size).await?;
        eprintln!("\nAI extracted {} raw card name(s)\n", names.len());
        names
    };

    // 4. Fuzzy-match against card database
    let mut matches = fuzzy_match::match_all(&db, &raw_names);

    // Quick summary
    let exact = matches.iter().filter(|m| m.confidence == fuzzy_match::Confidence::Exact).count();
    let high = matches.iter().filter(|m| m.confidence == fuzzy_match::Confidence::High).count();
    let med = matches.iter().filter(|m| m.confidence == fuzzy_match::Confidence::Medium).count();
    let low = matches.iter().filter(|m| m.confidence == fuzzy_match::Confidence::Low).count();
    eprintln!(
        "Match confidence: {} exact, {} high, {} medium, {} low",
        exact, high, med, low
    );

    // 5. Interactive wizard for ambiguous matches
    let card_names = wizard::resolve(&mut matches, non_interactive);

    // Filter out empty names (skipped cards)
    let card_names: Vec<String> = card_names.into_iter().filter(|n| !n.is_empty()).collect();

    // 6. Build deck list
    let mut deck = dck::DeckList::from_card_names(&card_names);

    // 7. Validation
    let warnings = validation::validate(&deck.main_deck, deck_size);
    wizard::resolve_count_violations(&mut deck.main_deck, non_interactive);

    if !wizard::resolve_warnings(&warnings, non_interactive) {
        eprintln!("Aborted.");
        return Ok(());
    }

    // 8. Output
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
