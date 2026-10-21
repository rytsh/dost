//! Draws Dost's app icon (a paw on a warm rounded square) at the sizes the platforms want.
//!
//! `cargo run --example icon <dir>` writes `<dir>/icon_<size>.png`; `make icons` turns them
//! into `packaging/macos/AppIcon.icns` and `packaging/windows/dost.ico`.

use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect,
    SpreadMode, Transform,
};

const SIZES: [u32; 10] = [16, 24, 32, 48, 64, 128, 256, 512, 1024, 20];

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).unwrap();
    for size in SIZES {
        icon(size)
            .save_png(format!("{dir}/icon_{size}.png"))
            .unwrap();
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(c);
    p
}

/// A rounded square (macOS "squircle"-ish, by a continuous-corner approximation).
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let k = 0.5523 * r;
    let (l, t, rt, b) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rt - r, t);
    pb.cubic_to(rt - r + k, t, rt, t + r - k, rt, t + r);
    pb.line_to(rt, b - r);
    pb.cubic_to(rt, b - r + k, rt - r + k, b, rt - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - k, b, l, b - r + k, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish().unwrap()
}

/// The paw from the settings panel (glyph units: about -7..7), rotated a little.
fn paw(cx: f32, cy: f32, unit: f32) -> tiny_skia::Path {
    let mut pb = PathBuilder::new();
    for (x, y, rx, ry) in [
        (0.0, 3.0, 4.2, 3.4),
        (-5.4, -1.2, 1.7, 2.1),
        (-2.1, -4.6, 1.7, 2.2),
        (2.1, -4.6, 1.7, 2.2),
        (5.4, -1.2, 1.7, 2.1),
    ] {
        if let Some(r) = Rect::from_xywh(
            cx + (x - rx) * unit,
            cy + (y - ry) * unit,
            2.0 * rx * unit,
            2.0 * ry * unit,
        ) {
            pb.push_oval(r);
        }
    }
    pb.finish().unwrap()
}

fn icon(size: u32) -> Pixmap {
    let s = size as f32;
    let mut pm = Pixmap::new(size, size).unwrap();
    // macOS icons leave a margin around the tile (824 of 1024); small sizes use more of it.
    let margin = if size >= 64 {
        s * 100.0 / 1024.0
    } else {
        s * 0.04
    };
    let tile = s - 2.0 * margin;
    let radius = tile * 0.225;

    if size >= 64 {
        // A soft drop shadow under the tile.
        for (dy, a) in [(0.010, 0.10), (0.018, 0.08), (0.028, 0.05)] {
            let path = rounded(margin, margin + s * dy, tile, tile, radius);
            let p = solid(Color::from_rgba8(0x5a, 0x2e, 0x08, (a * 255.0) as u8));
            pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Warm orange tile, lighter at the top.
    let path = rounded(margin, margin, tile, tile, radius);
    let p = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
            Point::from_xy(0.0, margin),
            Point::from_xy(0.0, margin + tile),
            vec![
                GradientStop::new(0.0, rgb(0xf6, 0xb0, 0x4c)),
                GradientStop::new(1.0, rgb(0xe0, 0x7f, 0x1c)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap(),
        ..Default::default()
    };
    pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);

    // A thin inner highlight along the top edge (large sizes only).
    if size >= 128 {
        let hl = solid(Color::from_rgba8(255, 255, 255, 40));
        let stroke = tiny_skia::Stroke {
            width: s * 0.006,
            ..Default::default()
        };
        let inner = rounded(
            margin + s * 0.003,
            margin + s * 0.003,
            tile - s * 0.006,
            tile - s * 0.006,
            radius,
        );
        pm.stroke_path(&inner, &hl, &stroke, Transform::identity(), None);
    }

    // The paw in cream, with a faint darker shadow below it.
    let unit = tile / 20.0;
    let (cx, cy) = (s / 2.0, s / 2.0 + unit * 0.4);
    if size >= 48 {
        let sh = solid(Color::from_rgba8(0x8a, 0x42, 0x08, 70));
        pm.fill_path(
            &paw(cx, cy + unit * 0.45, unit),
            &sh,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let cream = solid(rgb(0xfd, 0xf6, 0xea));
    pm.fill_path(
        &paw(cx, cy, unit),
        &cream,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    pm
}
