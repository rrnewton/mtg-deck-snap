//! mtg-deck-snap — convert photographs of MTG card spreads into .dck deck files.

mod batch;
mod card_db;
mod dck;
mod fuzzy_match;
mod image_proc;
mod pipeline;
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

        /// Vision backend to use for card-name extraction.
        ///
        /// `claude-cli` (default) shells out to the local `claude` CLI and needs
        /// NO API key — it uses your Claude subscription. `gemini` and `codex`
        /// likewise use their local CLIs / subscriptions. `anthropic-api` calls
        /// the Anthropic HTTP API directly and requires `ANTHROPIC_API_KEY`.
        #[arg(long, value_enum, default_value_t = vision::Backend::ClaudeCli)]
        backend: vision::Backend,

        /// Model override for the selected backend (backend-specific slug).
        /// Omit to use the backend's default model.
        #[arg(long)]
        model: Option<String>,
    },

    /// Batch-process a tree of input deck DIRECTORIES (non-interactive).
    ///
    /// By default the `--inputs-root` (`examples/inputs`) is walked recursively:
    /// every directory holding exactly one top-level image (`extra/` and
    /// `.DS_Store` ignored) is a deck dir. Each deck dir's path relative to the
    /// inputs root is mirrored into `--outputs-root` (`examples/outputs`), where
    /// `<deck-name>.dck` + `metadata.json` are written. Already-processed decks
    /// are skipped unless `--force` is given. Pass explicit `dirs` to process
    /// only those deck directories instead of walking.
    Batch {
        /// Specific deck directories to process (default: walk `--inputs-root`).
        dirs: Vec<PathBuf>,

        /// Root directory to walk for deck dirs.
        #[arg(long, default_value = "examples/inputs")]
        inputs_root: PathBuf,

        /// Root directory for outputs (mirrors each deck dir's relative path).
        #[arg(long, default_value = "examples/outputs")]
        outputs_root: PathBuf,

        /// Expected deck size hint (e.g. 40 for Limited, 60 for Constructed).
        #[arg(long)]
        deck_size: Option<u32>,

        /// Path to a Forge cardsfolder directory (uses Scryfall by default).
        #[arg(long)]
        cardsfolder: Option<PathBuf>,

        /// Vision backend to use for card-name extraction.
        #[arg(long, value_enum, default_value_t = vision::Backend::ClaudeCli)]
        backend: vision::Backend,

        /// Model override for the selected backend (backend-specific slug).
        #[arg(long)]
        model: Option<String>,

        /// Run a second AI pass to re-count card copies.
        #[arg(long)]
        multi_pass: bool,

        /// Reprocess every input dir even if its output already exists.
        #[arg(long)]
        force: bool,
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
            backend,
            model,
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
                backend,
                model,
            )
            .await
        }
        Commands::Batch {
            dirs,
            inputs_root,
            outputs_root,
            deck_size,
            cardsfolder,
            backend,
            model,
            multi_pass,
            force,
        } => {
            batch::cmd_batch(
                dirs,
                inputs_root,
                outputs_root,
                deck_size,
                cardsfolder,
                backend,
                model,
                multi_pass,
                force,
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
    backend: vision::Backend,
    model: Option<String>,
) -> Result<()> {
    // 1. Load card database
    let db = if let Some(cf) = cardsfolder {
        card_db::CardDatabase::load_forge(&cf)?
    } else {
        card_db::CardDatabase::load_scryfall(false).await?
    };
    eprintln!("Card database: {} names\n", db.len());

    // 2. Get raw card names + collect tiles for potential pass 2.
    //    The vision backend is only constructed when we actually have images to
    //    analyse (so `--from-list` runs need neither a CLI nor an API key).
    let mut vision_backend: Option<Box<dyn vision::VisionBackend>> = None;
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

        // AI vision pass 1 — extract card names via the selected backend.
        eprintln!("Vision backend: {}\n", backend.label());
        let be = backend.build(model.clone())?;
        let names = be.extract_card_names(&all_tiles, deck_size).await?;
        vision_backend = Some(be);
        eprintln!("\nAI extracted {} raw card name(s)\n", names.len());
        (names, all_tiles)
    };

    // 3-9. Shared pipeline: match, set-coherence, wizard, multi-pass, validate.
    let result = pipeline::process(
        &db,
        &raw_names,
        &tiles,
        vision_backend.as_deref(),
        deck_size,
        non_interactive,
        multi_pass,
    )
    .await?;

    if !result.proceed {
        eprintln!("Aborted.");
        return Ok(());
    }
    let deck = result.deck;

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
