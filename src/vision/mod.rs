//! AI vision: extract card names from image tiles.
//!
//! The extraction is pluggable across several *backends* (see [`Backend`]):
//!
//! * [`Backend::AnthropicApi`] — POST base64 tiles to `api.anthropic.com`
//!   directly (needs `ANTHROPIC_API_KEY`).
//! * [`Backend::ClaudeCli`] — shell out to the local `claude` CLI. Works on a
//!   Claude Max/Pro **subscription** with no API key and no per-call billing.
//! * [`Backend::Gemini`] — shell out to the local `gemini` CLI (subscription).
//! * [`Backend::Codex`] — shell out to the local `codex` CLI (subscription).
//!
//! Every backend shares the same prompts (see [`prompt`]) and the same output
//! parsing (see [`parse`]); each only implements the two per-tile calls
//! ([`VisionBackend::extract_tile`] and [`VisionBackend::recount_tile`]). The
//! multi-tile drivers with progress bars are provided as default trait methods
//! so the looping/merging logic lives in exactly one place.

mod anthropic;
mod cli;

pub mod parse;
pub mod prompt;

use crate::image_proc::Tile;
use anyhow::{bail, Result};
use async_trait::async_trait;
use indicatif::{ProgressBar, ProgressStyle};

pub use anthropic::AnthropicApiBackend;
pub use cli::{ClaudeCliBackend, CodexCliBackend, GeminiCliBackend};

/// The selectable vision backends, matching the `--backend` CLI flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    /// Anthropic Claude HTTP API (needs `ANTHROPIC_API_KEY`).
    #[value(name = "anthropic-api")]
    AnthropicApi,
    /// Local `claude` CLI — no API key, uses a Claude subscription.
    #[value(name = "claude-cli")]
    ClaudeCli,
    /// Local `gemini` CLI — no API key, uses a Gemini subscription.
    #[value(name = "gemini")]
    Gemini,
    /// Local `codex` CLI — no API key, uses a Codex subscription.
    #[value(name = "codex")]
    Codex,
}

impl Backend {
    /// Human-readable name for logging.
    pub fn label(self) -> &'static str {
        match self {
            Backend::AnthropicApi => "anthropic-api",
            Backend::ClaudeCli => "claude-cli",
            Backend::Gemini => "gemini",
            Backend::Codex => "codex",
        }
    }

    /// Build the concrete backend implementation for this selection.
    ///
    /// `model` is an optional per-backend model override from `--model`.
    pub fn build(self, model: Option<String>) -> Result<Box<dyn VisionBackend>> {
        Ok(match self {
            Backend::AnthropicApi => Box::new(AnthropicApiBackend::from_env(model)?),
            Backend::ClaudeCli => Box::new(ClaudeCliBackend::new(model)),
            Backend::Gemini => Box::new(GeminiCliBackend::new(model)),
            Backend::Codex => Box::new(CodexCliBackend::new(model)),
        })
    }
}

/// A pluggable vision backend.
///
/// Implementors only provide the two per-tile operations; the multi-tile
/// [`extract_card_names`](VisionBackend::extract_card_names) and
/// [`verify_counts`](VisionBackend::verify_counts) drivers are shared.
#[async_trait]
pub trait VisionBackend: Send + Sync {
    /// Extract raw card names from a single tile (pass 1 — identification).
    async fn extract_tile(&self, tile: &Tile, deck_size_hint: Option<u32>) -> Result<Vec<String>>;

    /// Recount how many copies of each `unique_names` card appear in a single
    /// tile (pass 2 — count verification). Returns `(name, count)` pairs.
    async fn recount_tile(&self, tile: &Tile, unique_names: &[String])
        -> Result<Vec<(String, u8)>>;

    /// Extract card names across every tile, merging the per-tile results.
    async fn extract_card_names(
        &self,
        tiles: &[Tile],
        deck_size_hint: Option<u32>,
    ) -> Result<Vec<String>> {
        let pb = progress_bar(tiles.len() as u64, "green");
        let mut all_names: Vec<String> = Vec::new();
        for tile in tiles {
            pb.set_message(tile.label.clone());
            let names = self.extract_tile(tile, deck_size_hint).await?;
            all_names.extend(names);
            pb.inc(1);
        }
        pb.finish_with_message("done");
        Ok(all_names)
    }

    /// Recount copies of each `unique_names` card across every tile.
    ///
    /// Each tile may only see a subset of cards, so the per-card result is the
    /// maximum count reported across tiles.
    async fn verify_counts(
        &self,
        tiles: &[Tile],
        unique_names: &[String],
    ) -> Result<Vec<(String, u8)>> {
        let pb = progress_bar(tiles.len() as u64, "cyan");
        let mut merged: std::collections::HashMap<String, Vec<u8>> =
            std::collections::HashMap::new();
        for tile in tiles {
            pb.set_message(format!("recount {}", tile.label));
            let counts = self.recount_tile(tile, unique_names).await?;
            for (name, count) in &counts {
                merged.entry(name.clone()).or_default().push(*count);
            }
            pb.inc(1);
        }
        pb.finish_with_message("recount done");

        let result: Vec<(String, u8)> = merged
            .into_iter()
            .map(|(name, counts)| {
                let max = counts.into_iter().max().unwrap_or(0);
                (name, max)
            })
            .collect();
        Ok(result)
    }
}

fn progress_bar(len: u64, color: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "{{spinner:.{color}}} [{{bar:30}}] {{pos}}/{{len}} tiles  {{msg}}"
            ))
            .expect("valid template")
            .progress_chars("█▓░"),
    );
    pb
}

/// Log the raw model output for a tile to stderr (shared by all backends).
pub(crate) fn log_raw_output(kind: &str, label: &str, text: &str) {
    eprintln!("\n── {kind} ({label}) ──");
    for line in text.lines() {
        eprintln!("  {line}");
    }
    eprintln!("── end ──\n");
}

/// Guard against an empty model response (a common CLI-failure signature).
pub(crate) fn require_nonempty(backend: &str, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!(
            "{backend} returned empty output — the CLI may not be installed, \
             signed in, or able to read the image"
        );
    }
    Ok(())
}
