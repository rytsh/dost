//! Draws the background of Dost's dmg window: a line of paw prints from the app (left) to
//! Applications (right), on nothing else.
//!
//! `cargo run --example dmg_background <dir>` writes `<dir>/dmg-background.png` and
//! `<dir>/dmg-background@2x.png`; `make dmg-background` puts them in `packaging/macos/`.
//! The icon positions must match the `dmg` target in the Makefile.

use tiny_skia::{Color, ColorU8, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// Window content size (points) and where Finder puts the two icons' centres.
const SIZE: (f32, f32) = (660.0, 400.0);
const APP: (f32, f32) = (170.0, 190.0);
const APPS: (f32, f32) = (490.0, 190.0);

/// The paw prints on it.
const PRINT: ColorU8 = ColorU8::from_rgba(0xc8, 0x9f, 0x6c, 0xff);

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).unwrap();
    for (k, name) in [(1.0, "dmg-background.png"), (2.0, "dmg-background@2x.png")] {
        draw(k).save_png(format!("{dir}/{name}")).unwrap();
    }
}

fn paint(c: ColorU8) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(Color::from_rgba8(c.red(), c.green(), c.blue(), c.alpha()));
    p
}

fn draw(k: f32) -> Pixmap {
    let (w, h) = ((SIZE.0 * k) as u32, (SIZE.1 * k) as u32);
    let mut pm = Pixmap::new(w, h).unwrap();
    // A straight line of paw prints from the app to Applications, left and right foot in turn.
    let (from, to) = (APP.0 + 72.0, APPS.0 - 72.0);
    let steps = 7;
    for i in 0..steps {
        let x = from + (to - from) * i as f32 / (steps - 1) as f32;
        let side = if i % 2 == 0 { -6.0 } else { 6.0 };
        draw_paw(&mut pm, x * k, (APP.1 + side) * k, 0.0, 1.5 * k);
    }
    pm
}

/// A paw print at (`x`, `y`) (physical), its toes pointing along `angle`.
fn draw_paw(pm: &mut Pixmap, x: f32, y: f32, angle: f32, k: f32) {
    let mut pb = PathBuilder::new();
    // Pad and four toes, toes towards +x (points).
    for (px, py, rx, ry) in [
        (-1.2, 0.0, 2.6, 3.1),
        (2.6, -3.2, 1.1, 1.0),
        (3.6, -1.1, 1.1, 1.0),
        (3.6, 1.1, 1.1, 1.0),
        (2.6, 3.2, 1.1, 1.0),
    ] {
        if let Some(r) = Rect::from_xywh((px - rx) * k, (py - ry) * k, 2.0 * rx * k, 2.0 * ry * k) {
            pb.push_oval(r);
        }
    }
    let t = Transform::from_rotate(angle.to_degrees()).post_translate(x, y);
    pm.fill_path(
        &pb.finish().unwrap(),
        &paint(PRINT),
        FillRule::Winding,
        t,
        None,
    );
}
