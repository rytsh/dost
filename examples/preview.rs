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
        (2.0, 1, true, "panel-200-look"),
        (2.0, 2, true, "panel-200-play"),
        (2.0, 3, true, "panel-200-sound"),
        (2.0, 4, true, "panel-200-todo"),
        (2.0, 5, true, "panel-200-history"),
        (2.0, 6, true, "panel-200-connect"),
    ] {
        let mut panel = dost::panel::Panel::new(scale);
        panel.set_tab(tab);
        if arrow {
            panel.set_arrow(Some(dost::panel::WIDTH * scale / 2.0));
        }
        let view = dost::panel::View {
            settings: dost::settings::Settings {
                name: "Karabaş".into(),
                birthday: "2024-10-21".into(),
                bark_files: vec!["~/sounds/woof.wav".into()],
                source_colors: [("ci".to_string(), "#2f6fc4".to_string())].into(),
                mcp_cors: dost::mcp::Cors {
                    allow_origins: vec!["https://*.example.com".into()],
                    allow_private_network: true,
                },
                ..Default::default()
            },
            age: "2 years".into(),
            visible: true,
            mcp_ok: true,
            mcp_status: "http://127.0.0.1:7777/mcp".into(),
            version: env!("CARGO_PKG_VERSION"),
            autostart_label: "Start with Windows",
            history: history.clone(),
            todos: todo_lists(),
            // The Connect tab shows an update waiting for a restart.
            update: (tab == 6).then(|| ("0.3.0".into(), true)),
            ..Default::default()
        };
        let (w, h) = panel.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        panel.draw(&mut pm, &mut r, &view);
        let mut shots = vec![(name.to_string(), pm.clone())];
        // Tabs longer than the panel: also scrolled to the end.
        panel.scroll_by(-100.0, h);
        panel.draw(&mut pm, &mut r, &view);
        if pm.data() != shots[0].1.data() {
            shots.push((format!("{name}-end"), pm.clone()));
        }
        for (name, pm) in shots {
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
    }

    // The settings window, in English and Turkish, on a few pages.
    {
        use dost::panel::{Page, Panel, WINDOW_SIZE};
        use dost::sessions::{Session, State};
        let sessions = vec![
            Session {
                id: "1".into(),
                source: "OpenCode".into(),
                title: "Refactor auth".into(),
                project: "api".into(),
                state: State::Waiting,
                detail: "Permission: bash · npm test".into(),
                since: dost::watch::now_unix() - 300,
                updated: dost::watch::now_unix(),
                ..Default::default()
            },
            Session {
                id: "2".into(),
                source: "Claude Code".into(),
                title: "Fix flaky test".into(),
                project: "web".into(),
                state: State::Working,
                detail: "edit · 12 steps".into(),
                since: dost::watch::now_unix() - 1260,
                updated: dost::watch::now_unix(),
                ..Default::default()
            },
            Session {
                id: "3".into(),
                source: "Codex".into(),
                title: "Release notes".into(),
                state: State::Done,
                detail: "Finished in 4 min, your turn.".into(),
                since: dost::watch::now_unix() - 240,
                updated: dost::watch::now_unix(),
                ..Default::default()
            },
        ];
        let now = dost::watch::now_unix();
        let job = |id, name: &str, schedule: &str, action, last: Option<(bool, &str)>, enabled| {
            let j = dost::cron::Job {
                id,
                name: name.into(),
                schedule: schedule.into(),
                action,
                notify: dost::cron::NotifyWhen::Failure,
                enabled,
                source: None,
                last: last.map(|(ok, d)| dost::cron::Run {
                    at: now - 600,
                    ok,
                    detail: d.into(),
                    secs: 0,
                }),
                runs: 3,
            };
            (j, Some(now + 3 * 3600))
        };
        let jobs = vec![
            job(
                1,
                "Standup",
                "0 9 * * 1-5",
                dost::cron::Action::Notify {
                    title: "Standup".into(),
                    message: String::new(),
                    sticky: false,
                },
                None,
                true,
            ),
            job(
                2,
                "api health",
                "*/15 * * * *",
                dost::cron::Action::Http {
                    method: "GET".into(),
                    url: "https://api.example.com/health".into(),
                    headers: Vec::new(),
                    body: None,
                    expect_status: None,
                    contains: None,
                },
                Some((true, "200 OK")),
                true,
            ),
            job(
                3,
                "Nightly backup",
                "30 2 * * *",
                dost::cron::Action::Command {
                    command: "./backup.sh".into(),
                    workdir: None,
                },
                Some((false, "exit 1")),
                false,
            ),
        ];
        for (lang, page, name) in [
            (dost::settings::Lang::En, Page::General, "window-general"),
            (dost::settings::Lang::Tr, Page::General, "window-general-tr"),
            (dost::settings::Lang::Tr, Page::Phone, "window-phone-tr"),
            (dost::settings::Lang::En, Page::Agents, "window-agents"),
            (dost::settings::Lang::Tr, Page::Dog, "window-dog-tr"),
            (dost::settings::Lang::En, Page::Look, "window-look"),
            (dost::settings::Lang::En, Page::Connect, "window-connect"),
            (dost::settings::Lang::En, Page::Schedule, "window-schedule"),
            (
                dost::settings::Lang::Tr,
                Page::Schedule,
                "window-schedule-tr",
            ),
        ] {
            dost::i18n::set(lang);
            let scale = 2.0;
            let mut panel = Panel::new_window(scale);
            panel.set_page(page);
            let mut settings = dost::settings::Settings {
                name: "Karabaş".into(),
                language: lang,
                ..Default::default()
            };
            settings.phone.ntfy.enabled = true;
            settings.phone.ntfy.topic = "dost-k3x9q2m8w1p4z7r5".into();
            settings.phone.telegram.enabled = true;
            settings.phone.telegram.token = "123:abc".into();
            settings.local_mcp.enabled = true;
            settings.local_mcp.root = "~/repo".into();
            settings.local_mcp.allowed_dirs = vec!["~/notes".into()];
            settings.local_mcp.always_allow = vec!["shell:cargo test*".into()];
            let view = dost::panel::View {
                settings,
                age: "2 years".into(),
                visible: true,
                mcp_ok: true,
                mcp_status: "http://127.0.0.1:7777/mcp".into(),
                version: env!("CARGO_PKG_VERSION"),
                autostart_label: "Open at login",
                history: history.clone(),
                todos: todo_lists(),
                sessions: sessions.clone(),
                jobs: jobs.clone(),
                screens: 2,
                special: Some(dost::special::Day::Remembrance),
                ..Default::default()
            };
            let (w, h) = (
                (WINDOW_SIZE.0 * scale) as u32,
                (WINDOW_SIZE.1 * scale) as u32,
            );
            let mut pm = Pixmap::new(w, h).unwrap();
            panel.draw(&mut pm, &mut r, &view);
            pm.save_png(out.replace(".png", &format!("-{name}.png")))
                .unwrap();
        }
        dost::i18n::set(dost::settings::Lang::En);
    }

    // Special days: the ribbon (10 November) and the flag (national holidays), in a few poses.
    {
        let s = Scale::new(1.0, 300);
        let poses = [
            (Anim::Idle, 0, 1.0),
            (Anim::Sit, 0, 1.0),
            (Anim::Walk, 2, -1.0),
            (Anim::Sleep, 0, 1.0),
        ];
        let (cw, ch) = ((CELL.0 * s.px) as i32, (CELL.1 * s.px) as i32);
        let mut sheet = Pixmap::new((cw * poses.len() as i32) as u32, (ch * 2) as u32).unwrap();
        sheet.fill(bg);
        for (row, pin) in [dost::special::Pin::Ribbon, dost::special::Pin::Flag]
            .into_iter()
            .enumerate()
        {
            r.set_pin(Some(pin));
            for (i, (anim, f, facing)) in poses.iter().enumerate() {
                let frame = DogFrame {
                    facing: *facing,
                    ..DogFrame::still(*anim, *f)
                };
                let (w, h) = r.size(frame.anim, None, s);
                let mut pm = Pixmap::new(w, h).unwrap();
                r.draw(&mut pm, w as f32 / 2.0, &frame, None, s);
                sheet.draw_pixmap(
                    cw * i as i32 + cw / 2 - w as i32 / 2,
                    ch * (row as i32 + 1) - h as i32,
                    pm.as_ref(),
                    &Default::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        r.set_pin(None);
        sheet.save_png(out.replace(".png", "-pins.png")).unwrap();
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

    // Looks: every fur and collar colour, every outfit, facing both ways, and the bone carried.
    {
        use dost::look::{Collar, Fur, Outfit, Season, Theme, Wear};
        let looks = [
            (Fur::Golden, Collar::Red, Wear::None, None),
            (Fur::Golden, Collar::None, Wear::None, None),
            (Fur::Cream, Collar::None, Wear::None, None),
            (Fur::Chocolate, Collar::Green, Wear::Beanie, None),
            (Fur::Black, Collar::Pink, Wear::None, None),
            (
                Fur::Grey,
                Collar::Purple,
                Wear::None,
                Some(Season::Birthday),
            ),
            (Fur::Red, Collar::Black, Wear::None, Some(Season::Halloween)),
            (
                Fur::Golden,
                Collar::Red,
                Wear::None,
                Some(Season::Christmas),
            ),
        ];
        let anims = [
            (Anim::Idle, 0, 1.0, false),
            (Anim::Walk, 3, -1.0, true),
            (Anim::Sit, 1, 1.0, false),
            (Anim::Bark, 2, -1.0, false),
            (Anim::Sad, 2, 1.0, false),
            (Anim::Sleep, 0, 1.0, false),
            (Anim::Held, 1, 1.0, false),
            (Anim::Happy, 0, 1.0, false),
            (Anim::Happy, 2, -1.0, false),
        ];
        let pad = 30;
        let (lw, lh) = (cw + 2 * pad, ch + 2 * pad);
        let mut sheet = Pixmap::new(
            (lw * anims.len() as i32) as u32,
            (lh * looks.len() as i32) as u32,
        )
        .unwrap();
        sheet.fill(bg);
        for (row, (fur, collar, wear, season)) in looks.into_iter().enumerate() {
            r.set_look(
                fur,
                collar,
                Outfit::new(wear, season),
                Theme::Classic,
                Vec::new(),
            );
            for (col, (anim, f, facing, carry)) in anims.into_iter().enumerate() {
                let frame = DogFrame {
                    facing,
                    carry,
                    ..DogFrame::still(anim, f)
                };
                let mut pm = Pixmap::new(lw as u32, lh as u32).unwrap();
                let (w, h) = r.size(anim, None, s);
                let _ = (w, h);
                // Draw through the renderer so worn items are included.
                let mut tmp = Pixmap::new(lw as u32, (lh + 40) as u32).unwrap();
                r.draw(&mut tmp, (lw / 2) as f32, &frame, None, s);
                pm.draw_pixmap(
                    0,
                    -40,
                    tmp.as_ref(),
                    &Default::default(),
                    Transform::identity(),
                    None,
                );
                sheet.draw_pixmap(
                    col as i32 * lw,
                    row as i32 * lh,
                    pm.as_ref(),
                    &Default::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        sheet.save_png(out.replace(".png", "-looks.png")).unwrap();

        // Bubble themes and source colours.
        let mut row: Vec<Pixmap> = Vec::new();
        for (theme, _) in Theme::ALL {
            r.set_look(
                Fur::Golden,
                Collar::Red,
                Outfit::default(),
                theme,
                Vec::new(),
            );
            let b = Bubble {
                title: "Build failed",
                body: "3 errors in src/auth.rs",
                source: Some(if theme == Theme::Classic {
                    "CI"
                } else {
                    "OpenCode"
                }),
                snooze: true,
                ..Default::default()
            };
            let s2 = Scale::new(2.0, 100).with_text(1.2);
            let frame = DogFrame::still(Anim::Sad, 0);
            let (w, h) = r.size(frame.anim, Some(&b), s2);
            let mut pm = Pixmap::new(w, h).unwrap();
            pm.fill(Color::from_rgba8(0xd9, 0xdd, 0xe3, 255));
            let mut fg = Pixmap::new(w, h).unwrap();
            r.draw(&mut fg, w as f32 * 0.5, &frame, Some(&b), s2);
            pm.draw_pixmap(
                0,
                0,
                fg.as_ref(),
                &Default::default(),
                Transform::identity(),
                None,
            );
            row.push(pm);
        }
        let w: u32 = row.iter().map(|p| p.width()).sum();
        let h = row.iter().map(|p| p.height()).max().unwrap();
        let mut sheet = Pixmap::new(w, h).unwrap();
        let mut x = 0;
        for p in &row {
            sheet.draw_pixmap(
                x,
                0,
                p.as_ref(),
                &Default::default(),
                Transform::identity(),
                None,
            );
            x += p.width() as i32;
        }
        sheet.save_png(out.replace(".png", "-themes.png")).unwrap();

        // The toy, turned.
        let px = 3;
        let size = r.toy_size(dost::look::Item::Bone, px);
        let mut sheet = Pixmap::new(size * 4, size).unwrap();
        sheet.fill(bg);
        for q in 0..4u8 {
            let mut pm = Pixmap::new(size, size).unwrap();
            r.draw_toy(&mut pm, dost::look::Item::Bone, q, px);
            sheet.draw_pixmap(
                (q as u32 * size) as i32,
                0,
                pm.as_ref(),
                &Default::default(),
                Transform::identity(),
                None,
            );
        }
        sheet.save_png(out.replace(".png", "-toy.png")).unwrap();
        r.set_look(
            Fur::Golden,
            Collar::Red,
            Outfit::default(),
            Theme::Classic,
            Vec::new(),
        );
    }

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

fn todo_lists() -> Vec<dost::todo::List> {
    use dost::todo::{Item, List};
    let item = |id, text: &str, done, remind_at| Item {
        id,
        text: text.into(),
        done,
        remind_at,
        ..Item::default()
    };
    vec![
        List {
            id: 1,
            name: "Release 0.3".into(),
            items: vec![
                item(2, "Bump the version", true, None),
                item(3, "Build the macOS app and the dmg", false, None),
                item(4, "Write the release notes", false, Some(4_102_477_200)),
            ],
        },
        List {
            id: 5,
            name: "Todo".into(),
            items: vec![item(6, "Buy dog food", false, None)],
        },
    ]
}
