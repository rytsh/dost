//! Renders the dog's animations and the bubble to PNG: `cargo run --example preview [out.png]`

use dost::dog::{Anim, DogFrame};
use dost::render::{Bubble, CELL, Renderer, Scale, blit_sprite};
use tiny_skia::{Color, Pixmap, Transform};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "preview.png".into());
    let s = Scale::new(1.0, 100).with_text(1.2);
    // On WSL, also load Windows fonts (for Korean, Hindi, emoji).
    let win_fonts = [
        "malgun.ttf",
        "Nirmala.ttc",
        "seguiemj.ttf",
        "segoeui.ttf",
        "segoeuib.ttf",
    ]
    .iter()
    .filter_map(|f| std::fs::read(format!("/mnt/c/Windows/Fonts/{f}")).ok())
    .collect();
    let mut r = Renderer::new(true, win_fonts);
    let atlas = Pixmap::decode_png(include_bytes!("../assets/dog.png")).unwrap();
    let bg = Color::from_rgba8(0xdd, 0xe4, 0xee, 255);

    // One row per animation; the last column is the first frame facing left (mirroring check).
    let (cw, ch) = ((CELL.0 * s.px) as i32, (CELL.1 * s.px) as i32);
    let mut sheet = Pixmap::new((cw * 9) as u32, (ch * Anim::ALL.len() as i32) as u32).unwrap();
    sheet.fill(bg);
    for (row, anim) in Anim::ALL.iter().enumerate() {
        let ground = ch * (row as i32 + 1) - 4;
        for f in 0..anim.frames() {
            let frame = DogFrame::still(*anim, f);
            blit_sprite(
                &mut sheet,
                &atlas,
                &frame,
                cw * f as i32 + cw / 2,
                ground,
                s.px as i32,
            );
        }
        let mut left = DogFrame::still(*anim, 0);
        left.facing = -1.0;
        blit_sprite(
            &mut sheet,
            &atlas,
            &left,
            cw * 8 + cw / 2,
            ground,
            s.px as i32,
        );
    }
    sheet.save_png(&out).unwrap();

    let bubbles = [
        (
            "bubble",
            "Yeni e-posta",
            "Ayşe: Yarınki toplantı 14:00'e alındı, haberin olsun. ğüşıöç ĞÜŞİÖÇ",
            2,
        ),
        (
            "bubble-intl",
            "多言語 · Мультиязычный",
            "日本語: ビルドが完了しました。中文：构建完成。한국어: 빌드 완료. Ελληνικά, हिन्दी: काम पूरा हुआ। 🐶🎉",
            0,
        ),
        (
            "bubble-rtl",
            "مرحبا · שלום",
            "العربية: اكتمل البناء بنجاح. עברית: הבנייה הסתיימה.",
            0,
        ),
    ];
    let mut bubble_out = String::new();
    for (name, title, body, pending) in bubbles {
        let b = Bubble {
            title,
            body,
            pending,
        };
        let mut frame = DogFrame::still(Anim::Bark, 2);
        frame.facing = -1.0;
        let (w, h) = r.size(frame.anim, Some(&b), s);
        let mut pm = Pixmap::new(w, h).unwrap();
        r.draw(&mut pm, w as f32 * 0.6, &frame, Some(&b), s);
        let mut canvas = Pixmap::new(w, h).unwrap();
        canvas.fill(bg);
        canvas.draw_pixmap(
            0,
            0,
            pm.as_ref(),
            &Default::default(),
            Transform::identity(),
            None,
        );
        bubble_out = out.replace(".png", &format!("-{name}.png"));
        canvas.save_png(&bubble_out).unwrap();
    }

    let frame = DogFrame {
        time: 1.3,
        ..DogFrame::still(Anim::Sleep, 0)
    };
    let (w, h) = r.size(frame.anim, None, s);
    let mut pm = Pixmap::new(w, h).unwrap();
    r.draw(&mut pm, w as f32 * 0.4, &frame, None, s);
    let mut canvas = Pixmap::new(w, h).unwrap();
    canvas.fill(bg);
    canvas.draw_pixmap(
        0,
        0,
        pm.as_ref(),
        &Default::default(),
        Transform::identity(),
        None,
    );
    canvas.save_png(out.replace(".png", "-sleep.png")).unwrap();

    for (scale, name) in [(1.0, "panel"), (1.5, "panel-150")] {
        let mut panel = dost::panel::Panel::new(scale);
        let view = dost::panel::View {
            settings: dost::settings::Settings::default(),
            visible: true,
            mcp_ok: true,
            mcp_status: "http://127.0.0.1:7777/mcp".into(),
            version: env!("CARGO_PKG_VERSION"),
            autostart_label: "Start with Windows",
            ..Default::default()
        };
        let (w, h) = panel.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        panel.draw(&mut pm, &mut r, &view);
        let mut canvas = Pixmap::new(w + 40, h + 40).unwrap();
        canvas.fill(Color::from_rgba8(0x3a, 0x3f, 0x4b, 255));
        canvas.draw_pixmap(
            20,
            20,
            pm.as_ref(),
            &Default::default(),
            Transform::identity(),
            None,
        );
        canvas
            .save_png(out.replace(".png", &format!("-{name}.png")))
            .unwrap();
    }

    // Transparent dog + bubble, for the README screenshot.
    {
        let b = Bubble {
            title: "Question: Refactor auth · api",
            body: "Should I also update the tests for the new token format?",
            pending: 0,
        };
        let s = Scale::new(1.0, 100).with_text(1.2);
        let frame = DogFrame::still(Anim::Bark, 0);
        let (w, h) = r.size(frame.anim, Some(&b), s);
        let mut pm = Pixmap::new(w, h).unwrap();
        r.draw(&mut pm, w as f32 * 0.5, &frame, Some(&b), s);
        pm.save_png(out.replace(".png", "-alpha.png")).unwrap();
    }

    let icon = r.icon_rgba(64);
    let mut ip = Pixmap::new(64, 64).unwrap();
    for (d, s) in ip.pixels_mut().iter_mut().zip(icon.chunks_exact(4)) {
        *d = tiny_skia::ColorU8::from_rgba(s[0], s[1], s[2], s[3]).premultiply();
    }
    ip.save_png(out.replace(".png", "-icon.png")).unwrap();
    println!("{out}, {bubble_out}");
}
