//! margdawn-gui — a minimal Markdown viewer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod emoji;
mod fonts;
mod md_view;

pub use app::c32;

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("margdawn-gui — minimal Markdown viewer\n\nUSAGE: margdawn-gui [file.md]");
        return;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("margdawn-gui {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let file = args.first().map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([880.0, 720.0])
            .with_min_inner_size([320.0, 240.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    let result = eframe::run_native(
        "margdawn",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, file)))),
    );

    if let Err(e) = result {
        eprintln!("error: {e}");
    }
}

/// 12x12 "M" glyph, scaled 2x into a 32x32 terracotta icon.
const M_GLYPH: [&str; 12] = [
    "X..........X",
    "XX........XX",
    "X.X......X.X",
    "X..X....X..X",
    "X...X..X...X",
    "X....XX....X",
    "X..........X",
    "X..........X",
    "X..........X",
    "X..........X",
    "X..........X",
    "X..........X",
];

pub fn load_icon() -> eframe::egui::IconData {
    let size = 32usize;
    let cr = 7usize; // corner radius
    let bg = (0xD9u8, 0x77u8, 0x57u8); // claude terracotta
    let ink = (0xFAu8, 0xF9u8, 0xF5u8); // cream
    let mut pixels = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            // rounded-square mask
            let px = x as i32;
            let py = y as i32;
            let s = size as i32;
            let c = cr as i32;
            let on_square = {
                let in_x_corner = px <= c || px >= s - 1 - c;
                let in_y_corner = py <= c || py >= s - 1 - c;
                if in_x_corner && in_y_corner {
                    let cx = if px <= s / 2 { c } else { s - 1 - c };
                    let cy = if py <= s / 2 { c } else { s - 1 - c };
                    let dx = (px - cx) * (px - cx);
                    let dy = (py - cy) * (py - cy);
                    dx + dy <= c * c
                } else {
                    true
                }
            };

            // "M" glyph: 12x12 string art, scaled 2x, centered
            let gx = x / 2;
            let gy = y / 2;
            let glyph = (4..16).contains(&gx)
                && (4..16).contains(&gy)
                && M_GLYPH
                    .get(gy - 4)
                    .and_then(|row| row.as_bytes().get(gx - 4))
                    .is_some_and(|&b| b == b'X');

            let (r, g, b) = if !on_square || glyph { ink } else { bg };
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    eframe::egui::IconData {
        width: size as u32,
        height: size as u32,
        rgba: pixels,
    }
}
