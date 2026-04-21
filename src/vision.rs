//! AI vision: extract card names from image tiles via the Anthropic Claude API.

use crate::image_proc::Tile;
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};

// ── API request / response types ────────────────────────────────────

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: Vec<ContentBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ResponseBlock>,
}

#[derive(Deserialize)]
struct ResponseBlock {
    text: Option<String>,
}

// ── public API ──────────────────────────────────────────────────────

/// Configuration for the vision pipeline.
pub struct VisionConfig {
    pub api_key: String,
    pub model: String,
}

impl VisionConfig {
    pub fn from_env() -> Result<Self> {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?;
        Ok(Self {
            api_key,
            model: "claude-sonnet-4-20250514".to_string(),
        })
    }
}

/// Extract card names from a list of tiles (pass 1 — identification).
///
/// Each tile is sent as a separate API call; the results are merged.
pub async fn extract_card_names(
    cfg: &VisionConfig,
    tiles: &[Tile],
    deck_size_hint: Option<u32>,
) -> Result<Vec<String>> {
    let client = build_client()?;

    let pb = ProgressBar::new(tiles.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:30}] {pos}/{len} tiles  {msg}")
            .expect("valid template")
            .progress_chars("█▓░"),
    );

    let mut all_names: Vec<String> = Vec::new();

    for tile in tiles {
        pb.set_message(tile.label.clone());
        let names = extract_single_tile(cfg, &client, tile, deck_size_hint).await?;
        all_names.extend(names);
        pb.inc(1);
    }

    pb.finish_with_message("done");
    Ok(all_names)
}

/// Multi-pass count verification (pass 2).
///
/// Given the tiles and the unique card names detected in pass 1, asks the model
/// to recount how many copies of each card it sees. Returns `(name, count)` pairs.
pub async fn verify_counts(
    cfg: &VisionConfig,
    tiles: &[Tile],
    unique_names: &[String],
) -> Result<Vec<(String, u8)>> {
    let client = build_client()?;

    let pb = ProgressBar::new(tiles.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan} [{bar:30}] {pos}/{len} tiles  {msg}")
            .expect("valid template")
            .progress_chars("█▓░"),
    );

    // Collect per-tile counts and merge
    let mut merged: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();

    for tile in tiles {
        pb.set_message(format!("recount {}", tile.label));
        let counts = recount_tile(cfg, &client, tile, unique_names).await?;
        for (name, count) in &counts {
            merged.entry(name.clone()).or_default().push(*count);
        }
        pb.inc(1);
    }

    pb.finish_with_message("recount done");

    // For each card, take the maximum count reported across tiles
    // (each tile may see only a subset of cards)
    let result: Vec<(String, u8)> = merged
        .into_iter()
        .map(|(name, counts)| {
            let max = counts.into_iter().max().unwrap_or(0);
            (name, max)
        })
        .collect();

    Ok(result)
}

// ── internals ───────────────────────────────────────────────────────

fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("building HTTP client")
}

async fn extract_single_tile(
    cfg: &VisionConfig,
    client: &reqwest::Client,
    tile: &Tile,
    deck_size_hint: Option<u32>,
) -> Result<Vec<String>> {
    let hint = deck_size_hint
        .map(|n| format!(" The full deck is expected to have roughly {} cards total (across all images).", n))
        .unwrap_or_default();

    let prompt = format!(
        "You are analyzing a photograph of Magic: The Gathering cards spread on a table. \
         The player has arranged the cards so that all card TITLES are visible (the full \
         card art/text may be obscured).{hint}\n\n\
         INSTRUCTIONS:\n\
         1. Read each card's TITLE BAR carefully — it is the text at the very top of the card.\n\
         2. Output ONLY card names, one per line, with no numbering, bullets, or commentary.\n\
         3. Include duplicates — if you see two copies of the same card, list the name twice.\n\
         4. For stacked basic lands (Island, Forest, etc.), count the number of cards in the \
            stack by looking at the visible edges/corners. Be precise — do not guess.\n\
         5. If you can only partially read a name, give your best guess followed by a ? suffix.\n\
         6. If a name is completely illegible, skip it.\n\
         7. Pay close attention to similar-looking letters: e vs o, i vs l, t vs f, etc.\n\
         8. MTG card names are proper nouns — capitalize each word."
    );

    let request = MessagesRequest {
        model: cfg.model.clone(),
        max_tokens: 4096,
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: "image/jpeg".to_string(),
                        data: tile.base64_jpeg.clone(),
                    },
                },
                ContentBlock::Text { text: prompt },
            ],
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("sending request to Claude API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Claude API error ({}): {}", status, body);
    }

    let body: MessagesResponse = resp.json().await.context("parsing Claude response")?;

    let text = body
        .content
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    // Log raw AI output for debugging
    eprintln!("\n── Raw AI output ({}) ──", tile.label);
    for line in text.lines() {
        eprintln!("  {}", line);
    }
    eprintln!("── end ──\n");

    let names: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(names)
}

async fn recount_tile(
    cfg: &VisionConfig,
    client: &reqwest::Client,
    tile: &Tile,
    unique_names: &[String],
) -> Result<Vec<(String, u8)>> {
    let card_list = unique_names.join("\n");

    let prompt = format!(
        "You are re-examining a photograph of Magic: The Gathering cards spread on a table.\n\n\
         Here are the card names we detected in a first pass:\n{card_list}\n\n\
         Now count how many PHYSICAL COPIES of each card you can see in THIS image.\n\
         Output ONLY lines in the format:  COUNT CARD_NAME\n\
         For example:\n  2 Lightning Bolt\n  1 Mountain\n\n\
         If a card from the list is not visible in this specific image tile, omit it.\n\
         Do NOT invent cards that are not in the list above."
    );

    let request = MessagesRequest {
        model: cfg.model.clone(),
        max_tokens: 4096,
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: "image/jpeg".to_string(),
                        data: tile.base64_jpeg.clone(),
                    },
                },
                ContentBlock::Text { text: prompt },
            ],
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("sending recount request to Claude API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Claude API recount error ({}): {}", status, body);
    }

    let body: MessagesResponse = resp.json().await.context("parsing Claude recount response")?;

    let text = body
        .content
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    eprintln!("\n── Recount AI output ({}) ──", tile.label);
    for line in text.lines() {
        eprintln!("  {}", line);
    }
    eprintln!("── end recount ──\n");

    let mut counts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Parse "COUNT CARD_NAME"
        if let Some((count_str, card_name)) = line.split_once(' ') {
            if let Ok(count) = count_str.parse::<u8>() {
                let card_name = card_name.trim().to_string();
                if !card_name.is_empty() {
                    counts.push((card_name, count));
                }
            }
        }
    }

    Ok(counts)
}
