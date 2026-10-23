//! Times and hashes renders of chosen pages of a PDF at several zooms, single-threaded, for before
//! and after comparisons of renderer changes.
//!
//! For each page and scenario it prints the cold time (a fresh renderer, so image decoding is
//! included), the minimum and median of `ROUNDS` warm renders (decoded images cached, so mostly
//! drawing and resampling), and an FNV-1a hash of every answer's size and pixels. Equal hashes mean
//! equal output. A render that fails stops the run with its error: a failed render is an empty
//! page, so two failing builds would otherwise hash alike.
//!
//! Usage: `page_bench FILE PAGES [ROUNDS] [SCENARIOS]`, with `PAGES` 1-based and comma-separated
//! (`1,4,11`) and `SCENARIOS` a comma-separated subset of the ones below (all by default).
//! Scenarios (device pixels per point):
//! - `thumb`: a 240 × 320 thumbnail at density 2 (preview).
//! - `zoom60`, `zoom100`: whole page at 60 and 96 dpi.
//! - `backdrop`: whole page at 2048 pixels on its longest side (preview).
//! - `fit`: whole page at 2800 pixels wide.
//! - `tiles100`, `tiles400`: four 1024 px tiles at 100% and 400% on a 2× display.

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, Tile};

const TILE: u32 = 1024;
const SCREEN_SCALE: f32 = 96.0 / 72.0 * 2.0;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;
const SCENARIOS: [&str; 7] = ["thumb", "zoom60", "zoom100", "backdrop", "fit", "tiles100", "tiles400"];

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(pages)) = (args.get(1), args.get(2)) else {
        return Err("usage: page_bench FILE PAGES [ROUNDS] [SCENARIOS]".into());
    };
    let pages: Vec<usize> = pages.split(',').map(|p| p.trim().parse::<usize>().map(|p| p.saturating_sub(1))).collect::<Result<_, _>>()?;
    let rounds: usize = match args.get(3) {
        Some(s) => s.parse::<usize>()?.max(1),
        None => 5,
    };
    let only: Option<Vec<&str>> = args.get(4).map(|s| s.split(',').collect());
    let bytes = Arc::new(std::fs::read(path)?);
    let sizes = {
        let pdf = hayro_syntax::Pdf::new_with_password(bytes.clone(), "").map_err(|e| format!("{e:?}"))?;
        pdf.pages().iter().map(|p| p.render_dimensions()).collect::<Vec<_>>()
    };

    println!("page scenario scale requests cold_ms warm_min_ms warm_median_ms hash");
    for &page in &pages {
        let &(width, height) = sizes.get(page).ok_or("no such page")?;
        for scenario in SCENARIOS.into_iter().filter(|s| only.as_ref().is_none_or(|o| o.contains(s))) {
            let requests = scenario_requests(scenario, page, width, height);
            let scale = requests.first().map_or(0.0, |r| r.scale);
            // Cold: a fresh renderer, so the page's images are decoded.
            let mut renderer = PageRenderer::new(bytes.clone(), RenderConfig::default());
            let (cold, hash) = run(&mut renderer, &requests).map_err(|e| format!("page {} {scenario}: {e}", page + 1))?;
            // Warm: the same renderer again, with decoded images cached.
            let mut warm = Vec::with_capacity(rounds);
            for _ in 0..rounds {
                let (time, warm_hash) = run(&mut renderer, &requests).map_err(|e| format!("page {} {scenario}: {e}", page + 1))?;
                if warm_hash != hash {
                    return Err(format!("page {} {scenario}: warm output differs from cold", page + 1).into());
                }
                warm.push(time);
            }
            warm.sort();
            let min = warm.first().copied().unwrap_or_default();
            let median = warm.get(warm.len() / 2).copied().unwrap_or_default();
            println!(
                "{} {scenario} {scale:.3} {} {} {} {} {hash:016x}",
                page + 1,
                requests.len(),
                cold.as_millis(),
                min.as_millis(),
                median.as_millis()
            );
        }
    }
    Ok(())
}

/// The requests of one scenario for `page`, `width` × `height` points.
fn scenario_requests(scenario: &str, page: usize, width: f32, height: f32) -> Vec<RenderRequest> {
    let whole = |scale: f32, preview: bool| RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview };
    let tiles = |scale: f32| -> Vec<RenderRequest> {
        [(0, 0), (TILE, 0), (0, TILE), (TILE, TILE)]
            .into_iter()
            .map(|(x, y)| RenderRequest { tile: Some(Tile { x, y, w: TILE, h: TILE }), ..whole(scale, false) })
            .collect()
    };
    match scenario {
        "thumb" => vec![whole((240.0 / width).min(320.0 / height) * 2.0, true)],
        "zoom60" => vec![whole(60.0 / 72.0, false)],
        "zoom100" => vec![whole(96.0 / 72.0, false)],
        "backdrop" => vec![whole(2048.0 / width.max(height), true)],
        "fit" => vec![whole(2800.0 / width, false)],
        "tiles100" => tiles(SCREEN_SCALE),
        _ => tiles(4.0 * SCREEN_SCALE),
    }
}

/// Renders `requests` in order. Returns the wall time and the output hash, or the first render
/// error.
fn run(renderer: &mut PageRenderer, requests: &[RenderRequest]) -> Result<(Duration, u64), String> {
    let start = Instant::now();
    let mut hash = FNV_OFFSET;
    for req in requests {
        let out = renderer.render(*req);
        if let Some(error) = out.error {
            return Err(format!("render failed: {error}"));
        }
        hash = fnv1a(hash, &out.width.to_le_bytes());
        hash = fnv1a(hash, &out.height.to_le_bytes());
        hash = fnv1a(hash, &out.rgba[..]);
    }
    Ok((start.elapsed(), hash))
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
