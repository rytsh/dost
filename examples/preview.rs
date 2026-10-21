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
            ..Default::default()
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

    let item =
        |time: &str, source: Option<&str>, title: &str, outcome: &str| dost::panel::HistoryItem {
            time: time.into(),
            source: source.map(String::from),
            title: title.into(),
            outcome: outcome.into(),
        };
    let history = vec![
        item(
            "14:32",
            Some("OpenCode"),
            "Update the tests too?",
            "answered: Yes",
        ),
        item("14:20", Some("CI"), "Running tests", "finished"),
        item(
            "13:05",
            None,
            "Reminder: stand up and stretch",
            "skipped (quiet)",
        ),
        item("12:48", Some("GitHub"), "PR #128 is ready", "opened link"),
    ];
    for (scale, tab, arrow, name) in [
        (1.0, 0, false, "panel"),
        (2.0, 0, true, "panel-200"),
        (2.0, 1, true, "panel-200-walk"),
        (2.0, 2, true, "panel-200-sound"),
        (2.0, 3, true, "panel-200-history"),
        (2.0, 4, true, "panel-200-connect"),
    ] {
        let mut panel = dost::panel::Panel::new(scale);
        panel.set_tab(tab);
        if arrow {
            panel.set_arrow(Some(dost::panel::WIDTH * scale / 2.0));
        }
        let view = dost::panel::View {
            settings: dost::settings::Settings::default(),
            visible: true,
            mcp_ok: true,
            mcp_status: "http://127.0.0.1:7777/mcp".into(),
            version: env!("CARGO_PKG_VERSION"),
            autostart_label: "Start with Windows",
            history: history.clone(),
            // The Connect tab shows an update waiting for a restart.
            update: (tab == 4).then(|| ("0.2.0".into(), true)),
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
            ..Default::default()
        };
        let s = Scale::new(1.0, 100).with_text(1.2);
        let frame = DogFrame::still(Anim::Bark, 0);
        let (w, h) = r.size(frame.anim, Some(&b), s);
        let mut pm = Pixmap::new(w, h).unwrap();
        r.draw(&mut pm, w as f32 * 0.5, &frame, Some(&b), s);
        pm.save_png(out.replace(".png", "-alpha.png")).unwrap();
    }

    let ip = r.icon_pixmap(64, false, false);
    ip.save_png(out.replace(".png", "-icon.png")).unwrap();
    let ip = r.icon_pixmap(64, true, false);
    ip.save_png(out.replace(".png", "-icon-sleep.png")).unwrap();
    let ip = r.icon_pixmap(64, false, true);
    ip.save_png(out.replace(".png", "-icon-dot.png")).unwrap();

    // MCP bubbles: a question, progress (running / done / failed) and a link.
    {
        // Plain news with more waiting: "Clear all" and "Later".
        let b = Bubble {
            title: "Build finished",
            body: "cargo build --release took 2 min 14 s.",
            source: Some("Terminal"),
            pending: 2,
            snooze: true,
            ..Default::default()
        };
        let s = Scale::new(2.0, 100).with_text(1.2);
        let (w, h) = r.bubble_size(&b, s);
        let mut pm = Pixmap::new(w, h).unwrap();
        r.draw_bubble_alone(&mut pm, w as f32 * 0.7, false, &b, s);
        pm.save_png(out.replace(".png", "-mcp-later.png")).unwrap();
    }
    {
        // On its own under the menu bar icon (shown while the dog is hidden).
        let choices = ["Allow".to_string(), "Always".into(), "Reject".into()];
        let b = Bubble {
            title: "Permission: Refactor auth · api",
            body: "Wants to edit src/auth.rs",
            source: Some("OpenCode"),
            choices: &choices,
            ..Default::default()
        };
        let s = Scale::new(2.0, 100).with_text(1.2);
        let (w, h) = r.bubble_size(&b, s);
        for (down, name) in [(false, "alone"), (true, "alone-down")] {
            let mut pm = Pixmap::new(w, h).unwrap();
            r.draw_bubble_alone(&mut pm, w as f32 * 0.7, down, &b, s);
            let mut canvas = Pixmap::new(w + 40, h + 40).unwrap();
            canvas.fill(Color::from_rgba8(0xd9, 0xdd, 0xe3, 255));
            canvas.draw_pixmap(
                20,
                20,
                pm.as_ref(),
                &Default::default(),
                Transform::identity(),
                None,
            );
            canvas
                .save_png(out.replace(".png", &format!("-mcp-{name}.png")))
                .unwrap();
        }
    }
    {
        use dost::notify::ProgressState;
        let choices2 = ["Yes".to_string(), "Not now".to_string()];
        let choices4 = ["Postgres", "SQLite", "MySQL", "Skip"].map(String::from);
        let bubbles = [
            (
                "ask",
                Bubble {
                    title: "Update the tests too?",
                    body: "The token format changed in 3 places.",
                    source: Some("OpenCode"),
                    choices: &choices2,
                    ..Default::default()
                },
            ),
            (
                "ask4",
                Bubble {
                    title: "",
                    body: "Which database should the new service use?",
                    source: Some("Claude"),
                    choices: &choices4,
                    ..Default::default()
                },
            ),
            (
                "progress",
                Bubble {
                    title: "Running tests",
                    body: "142 / 310",
                    source: Some("CI"),
                    progress: Some((Some(0.46), ProgressState::Running)),
                    ..Default::default()
                },
            ),
            (
                "progress-done",
                Bubble {
                    title: "Build finished",
                    body: "release · 2 m 14 s",
                    progress: Some((None, ProgressState::Done)),
                    ..Default::default()
                },
            ),
            (
                "progress-failed",
                Bubble {
                    title: "Migration failed",
                    body: "Step 4 of 7: column already exists.",
                    progress: Some((Some(0.57), ProgressState::Failed)),
                    ..Default::default()
                },
            ),
            (
                "link",
                Bubble {
                    title: "PR #128 is ready",
                    body: "All checks passed.",
                    source: Some("GitHub"),
                    link: true,
                    ..Default::default()
                },
            ),
        ];
        let s = Scale::new(2.0, 100).with_text(1.2);
        for (name, b) in bubbles {
            let frame = DogFrame::still(Anim::Sit, 0);
            let (w, h) = r.size(frame.anim, Some(&b), s);
            let mut pm = Pixmap::new(w, h).unwrap();
            r.draw(&mut pm, w as f32 * 0.5, &frame, Some(&b), s);
            let mut canvas = Pixmap::new(w, h).unwrap();
            canvas.fill(Color::from_rgba8(0xd9, 0xdd, 0xe3, 255));
            canvas.draw_pixmap(
                0,
                0,
                pm.as_ref(),
                &Default::default(),
                Transform::identity(),
                None,
            );
            canvas
                .save_png(out.replace(".png", &format!("-mcp-{name}.png")))
                .unwrap();
        }
    }
    println!("{out}, {bubble_out}");
}
