//! Image loading and tiling for AI vision.

use anyhow::{Context, Result};
use base64::Engine;
use image::GenericImageView;
use std::path::Path;

/// A single image tile ready to send to an AI vision API.
pub struct Tile {
    /// Base64-encoded JPEG data.
    pub base64_jpeg: String,
    /// Human-readable label (e.g. "tile 1/4").
    pub label: String,
    /// Width in pixels.
    #[allow(dead_code)]
    pub width: u32,
    /// Height in pixels.
    #[allow(dead_code)]
    pub height: u32,
}

/// Maximum dimension (px) for a single tile sent to the vision API.
const MAX_TILE_DIM: u32 = 1536;

/// Overlap (px) between adjacent tiles so card titles straddling a boundary
/// appear in full on at least one tile.
const TILE_OVERLAP: u32 = 192;

/// If the longest side is below this threshold, send the whole image as one tile.
const NO_TILE_THRESHOLD: u32 = 2048;

/// Load an image from disk and split it into tiles suitable for AI vision.
pub fn load_and_tile(path: &Path) -> Result<Vec<Tile>> {
    let img = image::open(path)
        .with_context(|| format!("failed to open image {}", path.display()))?;
    let (w, h) = img.dimensions();
    eprintln!(
        "Loaded image {} ({}×{})",
        path.file_name().unwrap_or_default().to_string_lossy(),
        w,
        h,
    );

    // If small enough, send as one tile
    if w.max(h) <= NO_TILE_THRESHOLD {
        let b64 = encode_jpeg(&img)?;
        return Ok(vec![Tile {
            base64_jpeg: b64,
            label: format!("whole image ({}×{})", w, h),
            width: w,
            height: h,
        }]);
    }

    // Otherwise, tile
    let step_x = MAX_TILE_DIM.saturating_sub(TILE_OVERLAP).max(256);
    let step_y = MAX_TILE_DIM.saturating_sub(TILE_OVERLAP).max(256);

    let cols = ((w as f64) / (step_x as f64)).ceil() as u32;
    let rows = ((h as f64) / (step_y as f64)).ceil() as u32;
    let total = cols * rows;
    eprintln!("Splitting into {}×{} = {} tiles", cols, rows, total);

    let mut tiles = Vec::with_capacity(total as usize);
    let mut idx = 0u32;

    for row in 0..rows {
        for col in 0..cols {
            let x0 = (col * step_x).min(w.saturating_sub(MAX_TILE_DIM));
            let y0 = (row * step_y).min(h.saturating_sub(MAX_TILE_DIM));
            let tile_w = MAX_TILE_DIM.min(w - x0);
            let tile_h = MAX_TILE_DIM.min(h - y0);

            let cropped = img.crop_imm(x0, y0, tile_w, tile_h);
            let b64 = encode_jpeg(&cropped)?;
            idx += 1;
            tiles.push(Tile {
                base64_jpeg: b64,
                label: format!("tile {}/{} (x={}, y={}, {}×{})", idx, total, x0, y0, tile_w, tile_h),
                width: tile_w,
                height: tile_h,
            });
        }
    }

    Ok(tiles)
}

/// Encode a `DynamicImage` as JPEG (quality 92) and return base64 string.
fn encode_jpeg(img: &image::DynamicImage) -> Result<String> {
    let rgb = img.to_rgb8();
    let mut buf: Vec<u8> = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    encoder
        .encode_image(&rgb)
        .context("JPEG encoding failed")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}
