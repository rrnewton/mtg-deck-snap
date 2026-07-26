//! Local-CLI vision backends: `claude`, `gemini`, and `codex`.
//!
//! These shell out to a locally installed agent CLI instead of calling an HTTP
//! API, so a user with a Claude Max / Gemini / Codex **subscription** can run
//! the pipeline with **no API key and no per-call billing**.
//!
//! Each tile's JPEG is written to a short-lived temp file and passed to the CLI
//! by path (the temp file is removed automatically when it drops).

use super::{log_raw_output, parse, prompt, require_nonempty, VisionBackend};
use crate::image_proc::Tile;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;

/// Write a tile's JPEG bytes to a temp `.jpg` file for a CLI to read.
fn tile_to_tempfile(tile: &Tile) -> Result<NamedTempFile> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&tile.base64_jpeg)
        .context("decoding tile JPEG")?;
    let mut f = tempfile::Builder::new()
        .prefix("mtg-deck-snap-tile-")
        .suffix(".jpg")
        .tempfile()
        .context("creating temp file for tile image")?;
    use std::io::Write;
    f.write_all(&bytes)
        .context("writing tile image to temp file")?;
    f.flush().ok();
    Ok(f)
}

/// Run a CLI, returning its stdout. Fails loudly (never silently) on a spawn
/// error or a non-zero exit, surfacing stderr for debugging.
async fn run_capture(backend: &str, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| {
            format!("failed to spawn `{program}` — is the {backend} CLI installed and on PATH?")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{backend} CLI (`{program}`) exited with {}: {}{}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ── claude CLI ──────────────────────────────────────────────────────

/// Shell out to `claude -p --output-format json`, passing the image as an
/// `@path` reference embedded in the prompt. Uses a Claude subscription when
/// `ANTHROPIC_API_KEY` is unset — no billing.
pub struct ClaudeCliBackend {
    model: Option<String>,
}

impl ClaudeCliBackend {
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    async fn call(&self, tile: &Tile, prompt_text: &str) -> Result<String> {
        let img = tile_to_tempfile(tile)?;
        let full_prompt = format!("{prompt_text} @{}", img.path().display());
        let mut args: Vec<&str> = vec!["-p", "--output-format", "json"];
        if let Some(m) = &self.model {
            args.push("--model");
            args.push(m);
        }
        args.push(&full_prompt);
        let raw = run_capture("claude-cli", "claude", &args).await?;
        require_nonempty("claude-cli", &raw)?;
        Ok(parse::claude_json_result(&raw))
    }
}

#[async_trait]
impl VisionBackend for ClaudeCliBackend {
    async fn extract_tile(&self, tile: &Tile, deck_size_hint: Option<u32>) -> Result<Vec<String>> {
        let text = self.call(tile, &prompt::extract(deck_size_hint)).await?;
        log_raw_output("Raw AI output [claude-cli]", &tile.label, &text);
        Ok(parse::card_names(&text))
    }

    async fn recount_tile(
        &self,
        tile: &Tile,
        unique_names: &[String],
    ) -> Result<Vec<(String, u8)>> {
        let text = self.call(tile, &prompt::recount(unique_names)).await?;
        log_raw_output("Recount AI output [claude-cli]", &tile.label, &text);
        Ok(parse::counts(&text))
    }
}

// ── gemini CLI ──────────────────────────────────────────────────────

/// Shell out to `gemini -y -p`, passing the image as an `@path` reference in
/// the prompt. `-y` auto-approves the CLI's file-read tool so the run is
/// non-interactive.
pub struct GeminiCliBackend {
    model: Option<String>,
}

impl GeminiCliBackend {
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    async fn call(&self, tile: &Tile, prompt_text: &str) -> Result<String> {
        let img = tile_to_tempfile(tile)?;
        let full_prompt = format!("{prompt_text} @{}", img.path().display());
        let mut args: Vec<&str> = vec!["-y", "-o", "text"];
        if let Some(m) = &self.model {
            args.push("-m");
            args.push(m);
        }
        args.push("-p");
        args.push(&full_prompt);
        let raw = run_capture("gemini", "gemini", &args).await?;
        require_nonempty("gemini", &raw)?;
        Ok(raw)
    }
}

#[async_trait]
impl VisionBackend for GeminiCliBackend {
    async fn extract_tile(&self, tile: &Tile, deck_size_hint: Option<u32>) -> Result<Vec<String>> {
        let text = self.call(tile, &prompt::extract(deck_size_hint)).await?;
        log_raw_output("Raw AI output [gemini]", &tile.label, &text);
        Ok(parse::card_names(&text))
    }

    async fn recount_tile(
        &self,
        tile: &Tile,
        unique_names: &[String],
    ) -> Result<Vec<(String, u8)>> {
        let text = self.call(tile, &prompt::recount(unique_names)).await?;
        log_raw_output("Recount AI output [gemini]", &tile.label, &text);
        Ok(parse::counts(&text))
    }
}

// ── codex CLI ───────────────────────────────────────────────────────

/// Shell out to `codex exec`, attaching the image with `-i/--image` and
/// collecting the model's final message via `--output-last-message`.
///
/// `codex exec` accepts image input, so this is a real backend (not a stub).
pub struct CodexCliBackend {
    model: Option<String>,
}

impl CodexCliBackend {
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    async fn call(&self, tile: &Tile, prompt_text: &str) -> Result<String> {
        let img = tile_to_tempfile(tile)?;
        let last_msg = NamedTempFile::new().context("creating codex output file")?;
        let img_path = img.path().display().to_string();
        let out_path = last_msg.path().display().to_string();

        // `-i/--image` is variadic, so the positional prompt must come BEFORE
        // it or the prompt would be swallowed as another image path.
        let mut args: Vec<&str> = vec![
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "--skip-git-repo-check",
            "-o",
            &out_path,
        ];
        if let Some(m) = &self.model {
            args.push("-m");
            args.push(m);
        }
        args.push(prompt_text);
        args.push("-i");
        args.push(&img_path);

        // codex prints its transcript to stdout; the clean final message is
        // written to the `-o` file, which we read instead of parsing stdout.
        run_capture("codex", "codex", &args).await?;
        let text = read_output_file(last_msg.path())?;
        require_nonempty("codex", &text)?;
        Ok(text)
    }
}

fn read_output_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).context("reading codex output-last-message file")
}

#[async_trait]
impl VisionBackend for CodexCliBackend {
    async fn extract_tile(&self, tile: &Tile, deck_size_hint: Option<u32>) -> Result<Vec<String>> {
        let text = self.call(tile, &prompt::extract(deck_size_hint)).await?;
        log_raw_output("Raw AI output [codex]", &tile.label, &text);
        Ok(parse::card_names(&text))
    }

    async fn recount_tile(
        &self,
        tile: &Tile,
        unique_names: &[String],
    ) -> Result<Vec<(String, u8)>> {
        let text = self.call(tile, &prompt::recount(unique_names)).await?;
        log_raw_output("Recount AI output [codex]", &tile.label, &text);
        Ok(parse::counts(&text))
    }
}
