//! What the draw-level visibility test in `render` saves on a page dense with lines.
//!
//! Builds a table, counts what the page's display list holds, then times one 1024×1024 px tile at
//! 500% at three positions down the page. The A/B is the same program on two builds: `main`, and
//! `main` with `crates/render/src/lib.rs` checked out from the commit before the test. Same
//! binary, same machine, same document — a cross-day baseline drifts more than this effect.

use std::time::Instant;

use wordcraft_doc::{Block, Cell, Document, Table};
use wordcraft_engine::Session;
use wordcraft_layout::display::{page_display, Draw};
use wordcraft_render::{render_area, render_page, RenderOptions};

fn table_doc(rows: usize, cols: usize) -> Document {
    let mut d = Document::new();
    let mut t = Table::new(rows, cols, 468.0);
    for r in 0..rows {
        for c in 0..cols {
            t.rows[r].cells[c] = Cell::with_text(&format!("R{} C{} 12.5", r + 1, c + 1));
        }
    }
    d.body = vec![std::sync::Arc::new(Block::Table(t))];
    d
}

fn counts(items: &[Draw]) -> (usize, usize, usize, usize, usize) {
    let (mut line, mut shape, mut path, mut glyphs) = (0, 0, 0, 0);
    for it in items {
        match it {
            Draw::Line { .. } => line += 1,
            Draw::Shape { .. } => shape += 1,
            Draw::Path { .. } => path += 1,
            Draw::Glyphs { .. } => glyphs += 1,
            _ => {}
        }
    }
    (line, shape, path, glyphs, items.len())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Lines whose own extent (grown the way `draw` grows it) misses the region — the ones the
/// visibility test can leave out.
fn lines_outside(items: &[Draw], r: (f64, f64, f64, f64)) -> usize {
    let mut n = 0;
    for it in items {
        if let Draw::Line { x0, y0, x1, y1, width, .. } = it {
            let pad = (*width as f64).max(1.0) * 4.0 + 8.0;
            let b = (
                (*x0 as f64).min(*x1 as f64) - pad,
                (*y0 as f64).min(*y1 as f64) - pad,
                (*x0 as f64).max(*x1 as f64) + pad,
                (*y0 as f64).max(*y1 as f64) + pad,
            );
            let hits = b.0 <= r.2 && b.2 >= r.0 && b.1 <= r.3 && b.3 >= r.1;
            if !hits {
                n += 1;
            }
        }
    }
    n
}

fn main() {
    // `sample` measures the built-in handbook instead of the table, for the comparison.
    let doc = match std::env::args().nth(1).as_deref() {
        Some("sample") => wordcraft_engine::sample::sample_document(),
        Some("table") | None => table_doc(40, 8),
        Some(other) => panic!("unknown document {other}: use table or sample"),
    };
    let mut s = Session::new(doc);
    let l = s.layout();
    println!("pages: {}", l.pages.len());

    let mut best = (0usize, 0usize);
    for (i, p) in l.pages.iter().enumerate() {
        let items = page_display(&s.doc, p, &Default::default());
        let (line, shape, path, glyphs, total) = counts(&items);
        println!("page {i}: {:.0}x{:.0} pt, {total} draws — {line} line, {shape} shape, {path} path, {glyphs} glyphs", p.w, p.h);
        if line > best.1 {
            best = (i, line);
        }
    }
    let page = &l.pages[best.0].clone();
    let items = page_display(&s.doc, page, &Default::default());

    let scale = 5.0f32;
    let opts = RenderOptions::default();
    let tile_px = 1024.0f64;
    let side_pt = (tile_px / scale as f64) as f32;

    let t0 = Instant::now();
    let _ = render_page(&s.doc, page, scale, &opts);
    println!("\nwhole page at {scale}x: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);

    println!("\n{:<8} {:>6} {:>14} {:>10}", "tile", "lines", "outside region", "ms");
    for (label, y) in [
        ("top", 0.0f32),
        ("quarter", page.h * 0.25),
        ("mid", page.h * 0.5),
        ("bottom", (page.h - side_pt).max(0.0)),
    ] {
        // `render_region` grows the page rect by 40 pt to catch items that reach in. Same here.
        let r = (-40.0f64, y as f64 - 40.0, side_pt as f64 + 40.0, y as f64 + side_pt as f64 + 40.0);
        let outside = lines_outside(&items, r);
        for _ in 0..2 {
            let _ = render_area(&s.doc, page, 0.0, y, side_pt, side_pt, scale, &opts);
        }
        let mut runs = Vec::new();
        for _ in 0..9 {
            let t = Instant::now();
            let img = render_area(&s.doc, page, 0.0, y, side_pt, side_pt, scale, &opts);
            std::hint::black_box(&img);
            runs.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{label:<8} {:>6} {:>14} {:>10.2}", best.1, outside, median(runs));
    }
}
