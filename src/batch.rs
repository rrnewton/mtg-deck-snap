//! Batch directory-processing mode.
//!
//! Batch mode walks an **inputs root** (default `examples/inputs`, override with
//! `--inputs-root`) and processes every *deck directory* it finds:
//!
//! * A **deck directory** is any directory that contains exactly one top-level
//!   image file (`jpg`/`jpeg`/`png`/`heic`/`webp`). That image is the deck's
//!   **primary photo**. `.DS_Store` and other non-image top-level files are
//!   ignored, as is any `extra/` subfolder (reserved for a future
//!   multi-image-confidence feature — not consumed yet).
//! * Directories with no top-level image are treated as *containers* (e.g. the
//!   `secrets_of_strixhaven_2026/` batch folder or the inputs root itself) and
//!   are descended into recursively. Because a deck dir stops the descent, an
//!   `extra/` folder — which only ever lives inside a deck dir — is never
//!   visited.
//!
//! Results are written to a SEPARATE **outputs root** (`--outputs-root`, default
//! `examples/outputs`). Each deck dir's path *relative to the inputs root* is
//! MIRRORED into the outputs root, so
//! `examples/inputs/secrets_of_strixhaven_2026/amber_strix1_BG/IMG_4171.jpeg`
//! produces
//! `examples/outputs/secrets_of_strixhaven_2026/amber_strix1_BG/{amber_strix1_BG.dck, metadata.json}`.
//! The deck-list filename is the deck dir's own basename. Output filenames
//! (`.dck` / `metadata.json`) never collide with input filenames (images /
//! `extra/`), so `rsync examples/outputs/ examples/inputs/` would overwrite
//! nothing and reproduce the interleaved view.
//!
//! Processing is **idempotent**: a deck dir whose mirrored output already holds
//! the `.dck` is skipped (and the vision model is NOT called) unless `--force`
//! is given. Batch mode is **non-interactive** (auto-accept) so a run never
//! blocks on stdin.

use crate::card_db::CardDatabase;
use crate::image_proc;
use crate::pipeline;
use crate::vision::{self, VisionBackend};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Image extensions recognised as a directory's primary image.
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "heic", "webp"];

/// Per-deck outcome, used to render the final summary table.
struct DirOutcome {
    /// Path relative to the inputs root (mirrored subpath), for display.
    rel: String,
    status: Status,
    total_cards: usize,
    unique_cards: usize,
    low_confidence: usize,
}

enum Status {
    Ok,
    Skipped,
    Failed(String),
}

/// Run batch processing.
///
/// If `dirs` is non-empty, exactly those deck directories are processed;
/// otherwise `inputs_root` is walked recursively to discover deck dirs.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_batch(
    dirs: Vec<PathBuf>,
    inputs_root: PathBuf,
    outputs_root: PathBuf,
    deck_size: Option<u32>,
    cardsfolder: Option<PathBuf>,
    backend: vision::Backend,
    model: Option<String>,
    multi_pass: bool,
    force: bool,
) -> Result<()> {
    // 1. Determine the set of deck dirs to process.
    let deck_dirs: Vec<PathBuf> = if dirs.is_empty() {
        let mut found = discover_deck_dirs(&inputs_root)?;
        found.sort();
        if found.is_empty() {
            bail!("no deck directories found under {}", inputs_root.display());
        }
        found
    } else {
        dirs
    };

    // 2. Load the card database once, shared across every directory.
    let db = if let Some(cf) = &cardsfolder {
        CardDatabase::load_forge(cf)?
    } else {
        CardDatabase::load_scryfall(false).await?
    };
    eprintln!("Card database: {} names\n", db.len());

    // 3. Build the vision backend once; reused for every dir that needs it.
    //    (Directories that are skipped never touch it.)
    let vision_backend: Box<dyn VisionBackend> = backend.build(model.clone())?;
    eprintln!("Vision backend: {}", backend.label());
    eprintln!("Inputs root:  {}", inputs_root.display());
    eprintln!("Outputs root: {}", outputs_root.display());
    eprintln!("Deck dirs:    {}\n", deck_dirs.len());

    let mut outcomes: Vec<DirOutcome> = Vec::new();

    for (i, deck_dir) in deck_dirs.iter().enumerate() {
        // Mirror the deck dir's path relative to the inputs root.
        let rel = relative_subpath(deck_dir, &inputs_root);
        let rel_display = rel.display().to_string();
        let deck_name = deck_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| deck_dir.display().to_string());

        eprintln!(
            "\n══════════════════════════════════════════════════════════════\n\
             [{}/{}] {}\n\
             ══════════════════════════════════════════════════════════════",
            i + 1,
            deck_dirs.len(),
            deck_dir.display(),
        );

        let outcome = process_one_dir(
            deck_dir,
            &deck_name,
            &outputs_root.join(&rel),
            &db,
            vision_backend.as_ref(),
            deck_size,
            backend,
            model.as_deref(),
            multi_pass,
            force,
        )
        .await;

        match outcome {
            Ok(mut o) => {
                o.rel = rel_display;
                outcomes.push(o);
            }
            Err(e) => {
                // Fail loudly for this dir but keep processing the rest.
                eprintln!("  ✗ FAILED: {:#}", e);
                outcomes.push(DirOutcome {
                    rel: rel_display,
                    status: Status::Failed(format!("{:#}", e)),
                    total_cards: 0,
                    unique_cards: 0,
                    low_confidence: 0,
                });
            }
        }
    }

    print_summary(&outcomes);

    // Non-zero exit if any directory failed, so callers/CI can detect it.
    let failures = outcomes
        .iter()
        .filter(|o| matches!(o.status, Status::Failed(_)))
        .count();
    if failures > 0 {
        bail!("{} of {} deck dir(s) failed", failures, outcomes.len());
    }
    Ok(())
}

/// Process a single deck directory, writing into the already-computed mirrored
/// `out_dir`. Returns its outcome, or an error if the directory could not be
/// processed at all (missing/ambiguous primary image, vision failure, etc.).
#[allow(clippy::too_many_arguments)]
async fn process_one_dir(
    dir: &Path,
    deck_name: &str,
    out_dir: &Path,
    db: &CardDatabase,
    vision_backend: &dyn VisionBackend,
    deck_size: Option<u32>,
    backend: vision::Backend,
    model: Option<&str>,
    multi_pass: bool,
    force: bool,
) -> Result<DirOutcome> {
    if !dir.is_dir() {
        bail!("not a directory: {}", dir.display());
    }

    let dck_path = out_dir.join(format!("{deck_name}.dck"));
    let meta_path = out_dir.join("metadata.json");

    // Idempotency: skip if already processed (unless --force).
    if dck_path.exists() && !force {
        eprintln!("  skip: already processed ({})", dck_path.display());
        return Ok(DirOutcome {
            rel: String::new(),
            status: Status::Skipped,
            total_cards: 0,
            unique_cards: 0,
            low_confidence: 0,
        });
    }

    // Find the single top-level image (ignore extra/ and .DS_Store).
    let primary = find_primary_image(dir)?;
    eprintln!("  primary image: {}", primary.display());
    if dir.join("extra").is_dir() {
        eprintln!(
            "  (note: extra/ present — ignored for now, reserved for future multi-image use)"
        );
    }

    // Load + tile the primary image.
    let tiles = image_proc::load_and_tile(&primary)
        .with_context(|| format!("processing image {}", primary.display()))?;
    eprintln!("  {} tile(s) to analyse", tiles.len());

    // Vision pass 1 — extract raw card names.
    let raw_names = vision_backend
        .extract_card_names(&tiles, deck_size)
        .await
        .context("vision extraction failed")?;
    eprintln!("  AI extracted {} raw card name(s)", raw_names.len());
    if raw_names.is_empty() {
        bail!("vision returned no card names for {}", primary.display());
    }

    // Shared pipeline (non-interactive / auto-accept).
    let result = pipeline::process(
        db,
        &raw_names,
        &tiles,
        Some(vision_backend),
        deck_size,
        true, // non-interactive
        multi_pass,
    )
    .await?;

    let deck = &result.deck;
    if deck.main_deck.is_empty() {
        bail!("no cards matched for {}", primary.display());
    }

    // Write the mirrored output dir: <deck>.dck + metadata.json.
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    deck.save(&dck_path, Some(deck_name))
        .with_context(|| format!("writing {}", dck_path.display()))?;

    let low_confidence = result
        .matches
        .iter()
        .filter(|m| m.confidence <= crate::fuzzy_match::Confidence::Medium)
        .count();

    let metadata = build_metadata(
        deck_name, dir, &primary, backend, model, deck_size, &raw_names, &result,
    );
    let meta_json = serde_json::to_string_pretty(&metadata).context("serializing metadata.json")?;
    std::fs::write(&meta_path, meta_json)
        .with_context(|| format!("writing {}", meta_path.display()))?;

    eprintln!(
        "  ✓ wrote {} cards ({} unique) → {}",
        deck.total_cards(),
        deck.main_deck.len(),
        dck_path.display(),
    );

    Ok(DirOutcome {
        rel: String::new(),
        status: Status::Ok,
        total_cards: deck.total_cards(),
        unique_cards: deck.main_deck.len(),
        low_confidence,
    })
}

/// Compute `deck_dir`'s path relative to `inputs_root`, falling back to the
/// basename if it is not actually under the root.
fn relative_subpath(deck_dir: &Path, inputs_root: &Path) -> PathBuf {
    match deck_dir.strip_prefix(inputs_root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => PathBuf::from(deck_dir.file_name().unwrap_or(deck_dir.as_os_str())),
    }
}

/// Recursively discover deck directories under `root`.
///
/// A directory containing at least one top-level image is a deck dir and stops
/// the descent; a directory with none is a container whose subdirectories are
/// searched. `root` itself is treated as a container.
fn discover_deck_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_for_decks(root, &mut out)?;
    Ok(out)
}

fn walk_for_decks(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    if !top_level_images(dir)?.is_empty() {
        // This is a deck dir; do not descend (skips its extra/).
        out.push(dir.to_path_buf());
        return Ok(());
    }
    // Container: descend into subdirectories in sorted order.
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            subdirs.push(entry.path());
        }
    }
    subdirs.sort();
    for sub in subdirs {
        walk_for_decks(&sub, out)?;
    }
    Ok(())
}

/// Return the sorted list of top-level image files in `dir` (non-recursive;
/// non-image files such as `.DS_Store` and subdirectories are ignored).
fn top_level_images(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut images: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let is_image = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()));
        if is_image {
            images.push(path);
        }
    }
    images.sort();
    Ok(images)
}

/// Find the single top-level image file in `dir`. Errors if there are zero or
/// more than one.
fn find_primary_image(dir: &Path) -> Result<PathBuf> {
    let images = top_level_images(dir)?;
    match images.len() {
        0 => bail!(
            "no top-level image found in {} (looked for {:?})",
            dir.display(),
            IMAGE_EXTS
        ),
        1 => Ok(images.into_iter().next().unwrap()),
        n => bail!(
            "expected exactly one top-level image in {}, found {}: {:?}",
            dir.display(),
            n,
            images
        ),
    }
}

// ── metadata.json ───────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct Metadata {
    deck_name: String,
    input_dir: String,
    primary_image: String,
    backend: String,
    model: Option<String>,
    timestamp: String,
    timestamp_unix: u64,
    deck_size_hint: Option<u32>,
    total_cards: usize,
    unique_cards: usize,
    raw_names: Vec<String>,
    matches: Vec<MetaMatch>,
    deck: Vec<MetaEntry>,
    validation: Vec<MetaWarning>,
    low_confidence_cards: Vec<MetaMatch>,
}

#[derive(serde::Serialize, Clone)]
struct MetaMatch {
    extracted: String,
    canonical: String,
    score: f64,
    confidence: String,
}

#[derive(serde::Serialize)]
struct MetaEntry {
    card_name: String,
    count: u8,
}

#[derive(serde::Serialize)]
struct MetaWarning {
    severity: String,
    message: String,
}

#[allow(clippy::too_many_arguments)]
fn build_metadata(
    deck_name: &str,
    dir: &Path,
    primary: &Path,
    backend: vision::Backend,
    model: Option<&str>,
    deck_size: Option<u32>,
    raw_names: &[String],
    result: &pipeline::DeckResult,
) -> Metadata {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Deduplicate matches for the metadata table (same extracted→canonical pair).
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut meta_matches: Vec<MetaMatch> = Vec::new();
    for m in &result.matches {
        let key = (m.extracted.clone(), m.canonical.clone());
        if !seen.insert(key) {
            continue;
        }
        meta_matches.push(MetaMatch {
            extracted: m.extracted.clone(),
            canonical: m.canonical.clone(),
            score: (m.score * 1000.0).round() / 1000.0,
            confidence: m.confidence.to_string(),
        });
    }

    let low_confidence_cards: Vec<MetaMatch> = meta_matches
        .iter()
        .filter(|m| m.confidence == "low" || m.confidence == "medium")
        .cloned()
        .collect();

    Metadata {
        deck_name: deck_name.to_string(),
        input_dir: dir.display().to_string(),
        primary_image: primary
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        backend: backend.label().to_string(),
        model: model.map(|s| s.to_string()),
        timestamp: iso8601_utc(secs),
        timestamp_unix: secs,
        deck_size_hint: deck_size,
        total_cards: result.deck.total_cards(),
        unique_cards: result.deck.main_deck.len(),
        raw_names: raw_names.to_vec(),
        matches: meta_matches,
        deck: result
            .deck
            .main_deck
            .iter()
            .map(|e| MetaEntry {
                card_name: e.card_name.clone(),
                count: e.count,
            })
            .collect(),
        validation: result
            .warnings
            .iter()
            .map(|w| MetaWarning {
                severity: w.severity.to_string(),
                message: w.message.clone(),
            })
            .collect(),
        low_confidence_cards,
    }
}

/// Format unix seconds as an ISO-8601 UTC timestamp (no external deps).
///
/// Uses Howard Hinnant's civil-from-days algorithm.
fn iso8601_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as i64;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

// ── summary table ───────────────────────────────────────────────────

fn print_summary(outcomes: &[DirOutcome]) {
    eprintln!("\n══════════════════════════════════════════════════════════════");
    eprintln!("  Batch summary");
    eprintln!("══════════════════════════════════════════════════════════════");
    eprintln!(
        "  {:<44} {:>6} {:>7} {:>7}  status",
        "Deck (relative to inputs root)", "#cards", "unique", "low-cf"
    );
    eprintln!("  {}", "─".repeat(84));
    for o in outcomes {
        let status = match &o.status {
            Status::Ok => "ok".to_string(),
            Status::Skipped => "skip".to_string(),
            Status::Failed(e) => format!("FAILED: {e}"),
        };
        eprintln!(
            "  {:<44} {:>6} {:>7} {:>7}  {}",
            pipeline::truncate_str(&o.rel, 44),
            o.total_cards,
            o.unique_cards,
            o.low_confidence,
            status,
        );
    }
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_epoch_zero() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_timestamp() {
        // 2021-01-01T00:00:00Z == 1609459200
        assert_eq!(iso8601_utc(1_609_459_200), "2021-01-01T00:00:00Z");
        // 2009-02-13T23:31:30Z == 1234567890
        assert_eq!(iso8601_utc(1_234_567_890), "2009-02-13T23:31:30Z");
    }

    #[test]
    fn find_primary_image_picks_single_top_level() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("card.jpg"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("extra")).unwrap();
        std::fs::write(dir.path().join("extra").join("other.jpg"), b"y").unwrap();
        std::fs::write(dir.path().join(".DS_Store"), b"z").unwrap();
        let found = find_primary_image(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "card.jpg");
    }

    #[test]
    fn find_primary_image_errors_when_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"z").unwrap();
        assert!(find_primary_image(dir.path()).is_err());
    }

    #[test]
    fn find_primary_image_errors_when_multiple() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.path().join("b.png"), b"y").unwrap();
        assert!(find_primary_image(dir.path()).is_err());
    }

    #[test]
    fn discover_walks_batch_layout_and_ignores_extra() {
        // root/batchA/deck1/{img.jpg, extra/x.jpg}, root/batchA/deck2/{img.png},
        // root/.DS_Store  →  discovers exactly deck1 and deck2 (never extra/).
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        std::fs::write(r.join(".DS_Store"), b"x").unwrap();
        let deck1 = r.join("batchA").join("deck1");
        std::fs::create_dir_all(deck1.join("extra")).unwrap();
        std::fs::write(deck1.join("img.jpg"), b"x").unwrap();
        std::fs::write(deck1.join("extra").join("x.jpg"), b"y").unwrap();
        let deck2 = r.join("batchA").join("deck2");
        std::fs::create_dir_all(&deck2).unwrap();
        std::fs::write(deck2.join("img.png"), b"z").unwrap();

        let mut found = discover_deck_dirs(r).unwrap();
        found.sort();
        assert_eq!(found, vec![deck1.clone(), deck2.clone()]);

        // Relative subpaths mirror the batch/deck hierarchy.
        assert_eq!(
            relative_subpath(&deck1, r),
            PathBuf::from("batchA").join("deck1")
        );
    }
}
