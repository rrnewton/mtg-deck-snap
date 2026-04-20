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

/// Extract card names from a list of tiles.
///
/// Each tile is sent as a separate API call; the results are merged.
pub async fn extract_card_names(
    cfg: &VisionConfig,
    tiles: &[Tile],
    deck_size_hint: Option<u32>,
) -> Result<Vec<String>> {
    let client = reqwest::Client::new();

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
         List EVERY card name you can read in this image. Output ONLY card names, one per \
         line, with no numbering, bullet points, or extra commentary.\n\
         Include duplicates — if you see two copies of the same card, list the name twice.\n\
         If you can only partially read a name, give your best guess followed by a ? suffix.\n\
         If a name is completely illegible, skip it."
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

    let names: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(names)
}
