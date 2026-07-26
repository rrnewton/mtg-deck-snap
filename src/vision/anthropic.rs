//! Anthropic Claude HTTP API backend (the original vision path).
//!
//! POSTs base64-encoded tiles to `api.anthropic.com/v1/messages`. Requires
//! `ANTHROPIC_API_KEY` and bills per call.

use super::{log_raw_output, parse, prompt, VisionBackend};
use crate::image_proc::Tile;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

pub struct AnthropicApiBackend {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicApiBackend {
    /// Build from `ANTHROPIC_API_KEY`, with an optional model override.
    pub fn from_env(model: Option<String>) -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY not set (required for the anthropic-api backend)")?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            client,
        })
    }

    async fn call(&self, tile: &Tile, prompt_text: String) -> Result<String> {
        let request = MessagesRequest {
            model: self.model.clone(),
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
                    ContentBlock::Text { text: prompt_text },
                ],
            }],
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("sending request to Claude API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Claude API error ({status}): {body}");
        }

        let body: MessagesResponse = resp.json().await.context("parsing Claude response")?;
        Ok(body
            .content
            .iter()
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[async_trait]
impl VisionBackend for AnthropicApiBackend {
    async fn extract_tile(&self, tile: &Tile, deck_size_hint: Option<u32>) -> Result<Vec<String>> {
        let text = self.call(tile, prompt::extract(deck_size_hint)).await?;
        log_raw_output("Raw AI output", &tile.label, &text);
        Ok(parse::card_names(&text))
    }

    async fn recount_tile(
        &self,
        tile: &Tile,
        unique_names: &[String],
    ) -> Result<Vec<(String, u8)>> {
        let text = self.call(tile, prompt::recount(unique_names)).await?;
        log_raw_output("Recount AI output", &tile.label, &text);
        Ok(parse::counts(&text))
    }
}

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
