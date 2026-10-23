//! Does a tiled page raster come out the same as a whole-page one? Renders the page whole, then
//! again as a grid of tiles stitched back together, and counts the pixels that differ. Seams,
//! double-drawn rows or a missing edge all show up here as a band with a large count; antialiasing
//! along a tile edge shows up as a few pixels per boundary with small differences.

use wordcraft_engine::Session;
use wordcraft_render::{render_area, render_page, RenderOptions};

fn main() {
    let mut s = Session::new(wordcraft_engine::sample::sample_document());
    let l = s.layout();
    let page = &l.pages[1].clone();
    let opts = RenderOptions::default();

    for scale in [4.0f32, 6.67] {
        for tile in [1024usize, 512] {
            let w_px = (page.w * scale).ceil() as usize;
            let h_px = (page.h * scale).ceil() as usize;
            let whole = render_page(&s.doc, page, scale, &opts);
            if whole.width as usize != w_px || whole.height as usize != h_px {
                println!("scale {scale}: whole raster is {}x{}, expected {w_px}x{h_px} — skipped", whole.width, whole.height);
                continue;
            }
            let mut stitched = vec![0u8; whole.pixels.len()];
            let (mut cols, mut rows) = (0, 0);
            for ty in (0..h_px).step_by(tile) {
                rows += 1;
                for tx in (0..w_px).step_by(tile) {
                    cols += 1;
                    let cw = (w_px - tx).min(tile);
                    let ch = (h_px - ty).min(tile);
                    let img = render_area(
                        &s.doc,
                        page,
                        tx as f32 / scale,
                        ty as f32 / scale,
                        (cw as f32 - 0.01).max(0.01) / scale,
                        (ch as f32 - 0.01).max(0.01) / scale,
                        scale,
                        &opts,
                    );
                    let (iw, ih) = (img.width as usize, img.height as usize);
                    for row in 0..ih.min(ch) {
                        let src = row * iw * 4;
                        let dst = ((ty + row) * w_px + tx) * 4;
                        let n = iw.min(cw) * 4;
                        stitched[dst..dst + n].copy_from_slice(&img.pixels[src..src + n]);
                    }
                }
            }
            cols /= rows;
            let mut differing = 0usize;
            let mut worst = 0i32;
            let mut worst_at = (0usize, 0usize);
            for p in 0..w_px * h_px {
                let a = &whole.pixels[p * 4..p * 4 + 4];
                let b = &stitched[p * 4..p * 4 + 4];
                let d = (0..4).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0);
                if d > 1 {
                    differing += 1;
                    if d > worst {
                        worst = d;
                        worst_at = (p % w_px, p / w_px);
                    }
                }
            }
            println!(
                "scale {scale:>4} tiles {tile:>4}: {cols}x{rows} grid, {w_px}x{h_px} px, differing {differing} of {} ({:.4}%), worst |d| {worst} at {:?}",
                w_px * h_px,
                differing as f64 * 100.0 / (w_px * h_px) as f64,
                worst_at
            );
        }
    }
}
