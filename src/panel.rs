//! Settings panel: drawn by us (not the system menu) so it looks the same on Windows and
//! macOS and matches the dog's pixel-art style. Platform-independent: it draws into a pixmap,
//! knows its own hit areas and turns clicks and drags into [`Action`]s.

use tiny_skia::{Color, ColorU8, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::render::{Renderer, TextStyle};
use crate::settings::{Ground, LIFTS, NOTIFY_SECS, SIZES, SPEEDS, Settings, TEXT_SIZES};

/// Logical width of the panel; height follows the content.
pub const WIDTH: f32 = 300.0;
const PAD: f32 = 12.0;
const ROW: f32 = 32.0;
const HEADER: f32 = 44.0;
const SECTION_GAP: f32 = 8.0;
const SECTION_TITLE: f32 = 22.0;
const RADIUS: f32 = 12.0;

const BG: ColorU8 = rgb(0xfb, 0xf7, 0xf1);
const CARD: ColorU8 = rgb(0xff, 0xff, 0xff);
const BORDER: ColorU8 = rgb(0xe8, 0xdf, 0xd2);
const INK: ColorU8 = rgb(0x2b, 0x2a, 0x33);
const MUTED: ColorU8 = rgb(0x8a, 0x82, 0x78);
const ACCENT: ColorU8 = rgb(0xe0, 0x8e, 0x2b);
const ACCENT_SOFT: ColorU8 = rgb(0xfc, 0xe9, 0xcf);
const TRACK: ColorU8 = rgb(0xe9, 0xe2, 0xd8);
const HOVER: ColorU8 = rgb(0xf6, 0xef, 0xe5);
const DANGER: ColorU8 = rgb(0xc8, 0x4b, 0x3c);

const fn rgb(r: u8, g: u8, b: u8) -> ColorU8 {
    ColorU8::from_rgba(r, g, b, 255)
}

/// What a click or drag on the panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// New settings to apply and save.
    Settings(Settings),
    /// Volume slider moved; `done` when the drag ended (play a test bark then).
    Volume {
        volume: u8,
        done: bool,
    },
    TestNotification,
    ToggleSleep,
    ToggleVisible,
    ToggleAutostart,
    CopyMcpUrl,
    OpenConfigDir,
    Quit,
    Close,
}

/// State the panel shows but does not own.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub settings: Settings,
    pub sleeping: bool,
    pub visible: bool,
    pub autostart: bool,
    /// "Start with Windows" / "Open at login".
    pub autostart_label: &'static str,
    /// MCP status line: address when running, the error otherwise.
    pub mcp_status: String,
    pub mcp_ok: bool,
    pub version: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Choice(Field, usize),
    Toggle(Toggle),
    Slider,
    Button(Button),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Size,
    TextSize,
    Speed,
    Ground,
    Lift,
    Notify,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Toggle {
    Sound,
    SoundOnPet,
    Wander,
    Mcp,
    Autostart,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Button {
    Test,
    Sleep,
    Hide,
    CopyMcp,
    OpenDir,
    Quit,
    Close,
}

pub struct Panel {
    /// Logical-to-physical scale (DPI).
    ui: f32,
    hits: Vec<(Rect, Hit)>,
    slider: Option<Rect>,
    hover: Option<Hit>,
    dragging: bool,
    scroll: f32,
    content_height: f32,
}

impl Panel {
    pub fn new(ui: f32) -> Self {
        Panel {
            ui,
            hits: Vec::new(),
            slider: None,
            hover: None,
            dragging: false,
            scroll: 0.0,
            content_height: 0.0,
        }
    }

    pub fn set_ui_scale(&mut self, ui: f32) {
        self.ui = ui;
    }

    /// Physical size for a panel that may be at most `max_height` physical pixels tall.
    pub fn size(&self, max_height: u32) -> (u32, u32) {
        let w = (WIDTH * self.ui).ceil() as u32;
        let h = (content_height() * self.ui).ceil() as u32;
        (w, h.min(max_height))
    }

    /// Mouse wheel; `delta` in lines (positive = up).
    pub fn scroll_by(&mut self, delta: f32, view_height: u32) {
        let max = (self.content_height - view_height as f32).max(0.0);
        self.scroll = (self.scroll - delta * ROW * self.ui).clamp(0.0, max);
    }

    pub fn draw(&mut self, pm: &mut Pixmap, r: &mut Renderer, v: &View) {
        let u = self.ui;
        let s = &v.settings;
        self.hits.clear();
        self.slider = None;
        pm.fill(Color::TRANSPARENT);
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        fill_round(pm, Rect::from_xywh(0.0, 0.0, w, h), RADIUS * u, BG);
        stroke_round(
            pm,
            Rect::from_xywh(0.5, 0.5, w - 1.0, h - 1.0),
            RADIUS * u,
            BORDER,
            1.0,
        );

        let mut y = -self.scroll;
        let x = PAD * u;
        let inner = w - 2.0 * x;

        // Header: name, state, close button.
        let (tw, _) = r.measure("Dost", sys(15.0 * u, true, INK));
        r.draw_text(
            pm,
            x + 4.0 * u,
            y + 13.0 * u,
            "Dost",
            sys(15.0 * u, true, INK),
        );
        let sub = if v.sleeping {
            "sleeping"
        } else if v.visible {
            "on your taskbar"
        } else {
            "hidden"
        };
        r.draw_text(
            pm,
            x + 4.0 * u + tw + 8.0 * u,
            y + 15.0 * u,
            sub,
            sys(12.0 * u, false, MUTED),
        );
        let close = Rect::from_xywh(w - x - 24.0 * u, y + 10.0 * u, 24.0 * u, 24.0 * u);
        self.button_icon(pm, r, close, "×", Button::Close);
        y += HEADER * u;

        // Quick actions.
        let bw = (inner - 2.0 * 8.0 * u) / 3.0;
        let quick = [
            (Button::Test, "Test"),
            (Button::Sleep, if v.sleeping { "Wake" } else { "Sleep" }),
            (Button::Hide, if v.visible { "Hide" } else { "Show" }),
        ];
        for (i, (b, label)) in quick.into_iter().enumerate() {
            let rect = Rect::from_xywh(x + i as f32 * (bw + 8.0 * u), y, bw, 28.0 * u);
            self.button(pm, r, rect, label, b, false);
        }
        y += (28.0 + SECTION_GAP) * u;

        // Look.
        y = self.section(pm, r, y, "Look");
        let card = self.card_start(y, 3);
        fill_card(pm, card, u);
        let mut cy = card.map_or(y, |c| c.top());
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Size",
            Field::Size,
            &SIZES.map(|(_, l)| l),
            SIZES.iter().position(|(v, _)| *v == s.size),
        );
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Text size",
            Field::TextSize,
            &TEXT_SIZES.map(|(_, l)| l),
            TEXT_SIZES.iter().position(|(v, _)| *v == s.text_size),
        );
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Bubble",
            Field::Notify,
            &NOTIFY_SECS.map(|(_, l)| l),
            NOTIFY_SECS.iter().position(|(v, _)| *v == s.notify_secs),
        );
        y = cy + SECTION_GAP * u;

        // Behaviour.
        y = self.section(pm, r, y, "Behaviour");
        let card = self.card_start(y, 4);
        fill_card(pm, card, u);
        let mut cy = card.map_or(y, |c| c.top());
        cy = self.toggle_row(pm, r, cy, x, inner, "Walk around", s.wander, Toggle::Wander);
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Speed",
            Field::Speed,
            &SPEEDS.map(|(_, l)| l),
            SPEEDS.iter().position(|(v, _)| *v == s.speed),
        );
        let grounds = ["Taskbar", "Screen edge"];
        let gi = match s.ground {
            Ground::Taskbar => 0,
            Ground::Screen => 1,
        };
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Stand on",
            Field::Ground,
            &grounds,
            Some(gi),
        );
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Offset",
            Field::Lift,
            &LIFTS.map(|(_, l)| l),
            LIFTS.iter().position(|(v, _)| *v == s.lift),
        );
        y = cy + SECTION_GAP * u;

        // Sound.
        y = self.section(pm, r, y, "Sound");
        let card = self.card_start(y, 3);
        fill_card(pm, card, u);
        let mut cy = card.map_or(y, |c| c.top());
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Bark on notifications",
            s.sound,
            Toggle::Sound,
        );
        cy = self.slider_row(pm, r, cy, x, inner, s.volume, s.sound);
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Bark when petted",
            s.sound_on_pet,
            Toggle::SoundOnPet,
        );
        y = cy + SECTION_GAP * u;

        // Integrations.
        y = self.section(pm, r, y, "Integrations");
        let card = self.card_start(y, 3);
        fill_card(pm, card, u);
        let mut cy = card.map_or(y, |c| c.top());
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "MCP server",
            s.mcp_enabled,
            Toggle::Mcp,
        );
        let status_color = if v.mcp_ok { MUTED } else { DANGER };
        r.draw_text(
            pm,
            x + 12.0 * u,
            cy + 8.0 * u,
            &v.mcp_status,
            sys(11.5 * u, false, status_color),
        );
        let copy = Rect::from_xywh(x + inner - 62.0 * u, cy + 4.0 * u, 54.0 * u, 24.0 * u);
        self.button(pm, r, copy, "Copy", Button::CopyMcp, true);
        cy += ROW * u;
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            v.autostart_label,
            v.autostart,
            Toggle::Autostart,
        );
        y = cy + SECTION_GAP * u;

        // Footer.
        let bw = (inner - 8.0 * u) / 2.0;
        self.button(
            pm,
            r,
            Rect::from_xywh(x, y, bw, 28.0 * u),
            "Settings file",
            Button::OpenDir,
            true,
        );
        self.button(
            pm,
            r,
            Rect::from_xywh(x + bw + 8.0 * u, y, bw, 28.0 * u),
            "Quit Dost",
            Button::Quit,
            true,
        );
        y += 28.0 * u + 8.0 * u;
        r.draw_text(
            pm,
            x + 4.0 * u,
            y,
            &format!("Dost {}", v.version),
            sys(10.5 * u, false, MUTED),
        );
        y += 18.0 * u;
        self.content_height = y + self.scroll;
    }

    /// Mouse moved to (`x`, `y`), physical pixels relative to the panel. Returns true if the
    /// panel needs a redraw; `Some(action)` while dragging the slider.
    pub fn pointer_move(&mut self, x: f32, y: f32, settings: &Settings) -> (bool, Option<Action>) {
        if self.dragging {
            return (
                true,
                self.slider_value(x).map(|volume| Action::Volume {
                    volume,
                    done: false,
                }),
            )
                .filter_volume(settings);
        }
        let hover = self.hit(x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        (changed, None)
    }

    pub fn pointer_leave(&mut self) -> bool {
        let changed = self.hover.is_some();
        self.hover = None;
        changed
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) -> Option<Action> {
        if self.hit(x, y) == Some(Hit::Slider) {
            self.dragging = true;
            return self.slider_value(x).map(|volume| Action::Volume {
                volume,
                done: false,
            });
        }
        None
    }

    pub fn pointer_up(&mut self, x: f32, y: f32, v: &View) -> Option<Action> {
        if std::mem::take(&mut self.dragging) {
            return self
                .slider_value(x)
                .map(|volume| Action::Volume { volume, done: true });
        }
        let mut s = v.settings.clone();
        match self.hit(x, y)? {
            Hit::Choice(field, i) => match field {
                Field::Size => s.size = SIZES[i].0,
                Field::TextSize => s.text_size = TEXT_SIZES[i].0,
                Field::Speed => s.speed = SPEEDS[i].0,
                Field::Ground => {
                    s.ground = if i == 0 {
                        Ground::Taskbar
                    } else {
                        Ground::Screen
                    }
                }
                Field::Lift => s.lift = LIFTS[i].0,
                Field::Notify => s.notify_secs = NOTIFY_SECS[i].0,
            },
            Hit::Toggle(t) => match t {
                Toggle::Sound => s.sound = !s.sound,
                Toggle::SoundOnPet => s.sound_on_pet = !s.sound_on_pet,
                Toggle::Wander => s.wander = !s.wander,
                Toggle::Mcp => s.mcp_enabled = !s.mcp_enabled,
                Toggle::Autostart => return Some(Action::ToggleAutostart),
            },
            Hit::Slider => return None,
            Hit::Button(b) => {
                return Some(match b {
                    Button::Test => Action::TestNotification,
                    Button::Sleep => Action::ToggleSleep,
                    Button::Hide => Action::ToggleVisible,
                    Button::CopyMcp => Action::CopyMcpUrl,
                    Button::OpenDir => Action::OpenConfigDir,
                    Button::Quit => Action::Quit,
                    Button::Close => Action::Close,
                });
            }
        }
        (s != v.settings).then_some(Action::Settings(s))
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(r, _)| contains(*r, x, y))
            .map(|(_, h)| *h)
    }

    fn slider_value(&self, x: f32) -> Option<u8> {
        let r = self.slider?;
        let t = ((x - r.left()) / r.width()).clamp(0.0, 1.0);
        Some((t * 100.0).round() as u8)
    }

    fn section(&mut self, pm: &mut Pixmap, r: &mut Renderer, y: f32, title: &str) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            PAD * u + 4.0 * u,
            y + 4.0 * u,
            title,
            sys(11.5 * u, true, MUTED),
        );
        y + SECTION_TITLE * u
    }

    fn card_start(&self, y: f32, rows: usize) -> Option<Rect> {
        let u = self.ui;
        let w = WIDTH * u - 2.0 * PAD * u;
        Rect::from_xywh(PAD * u, y, w, rows as f32 * ROW * u)
    }

    #[allow(clippy::too_many_arguments)]
    fn choice_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        label: &str,
        field: Field,
        options: &[&str],
        selected: Option<usize>,
    ) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            label,
            sys(13.0 * u, false, INK),
        );
        // Segmented control on the right; options share the space by text width.
        let seg_x = x + 88.0 * u;
        let seg_w = inner - 88.0 * u - 6.0 * u;
        let seg = Rect::from_xywh(seg_x, y + 5.0 * u, seg_w, 22.0 * u);
        fill_round(pm, seg, 8.0 * u, TRACK);
        let widths: Vec<f32> = options
            .iter()
            .map(|o| r.measure(o, sys(11.5 * u, false, INK)).0 + 10.0 * u)
            .collect();
        let total: f32 = widths.iter().sum();
        let mut ox = seg_x;
        for (i, (o, ow)) in options.iter().zip(&widths).enumerate() {
            let w = ow / total * seg_w;
            let Some(cell) = Rect::from_xywh(ox, y + 5.0 * u, w, 22.0 * u) else {
                continue;
            };
            let on = selected == Some(i);
            let hit = Hit::Choice(field, i);
            if on {
                fill_round(pm, cell.inset(2.0 * u), 6.0 * u, CARD);
                stroke_round(pm, cell.inset(2.0 * u), 6.0 * u, BORDER, 1.0);
            } else if self.hover == Some(hit) {
                fill_round(pm, cell.inset(2.0 * u), 6.0 * u, HOVER);
            }
            let st = sys(11.5 * u, on, if on { INK } else { MUTED });
            let (tw, th) = r.measure(o, st);
            r.draw_text(
                pm,
                (cell.left() + (cell.width() - tw) / 2.0).round(),
                (cell.top() + (cell.height() - th) / 2.0).round(),
                o,
                st,
            );
            self.hits.push((cell, hit));
            ox += w;
        }
        y + ROW * u
    }

    #[allow(clippy::too_many_arguments)]
    fn toggle_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        label: &str,
        on: bool,
        t: Toggle,
    ) -> f32 {
        let u = self.ui;
        let Some(row) = Rect::from_xywh(x, y, inner, ROW * u) else {
            return y + ROW * u;
        };
        if self.hover == Some(Hit::Toggle(t)) {
            fill_round(pm, row.inset(2.0 * u), 10.0 * u, HOVER);
        }
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            label,
            sys(13.0 * u, false, INK),
        );
        let sw = Rect::from_xywh(x + inner - 44.0 * u, y + 7.0 * u, 34.0 * u, 18.0 * u);
        fill_round(pm, sw, 9.0 * u, if on { ACCENT } else { TRACK });
        let knob_x = if on {
            x + inner - 44.0 * u + 18.0 * u
        } else {
            x + inner - 44.0 * u + 2.0 * u
        };
        fill_round(
            pm,
            Rect::from_xywh(knob_x, y + 9.0 * u, 14.0 * u, 14.0 * u),
            7.0 * u,
            CARD,
        );
        self.hits.push((row, Hit::Toggle(t)));
        y + ROW * u
    }

    #[allow(clippy::too_many_arguments)]
    fn slider_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        volume: u8,
        enabled: bool,
    ) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            "Volume",
            sys(13.0 * u, false, if enabled { INK } else { MUTED }),
        );
        let label = format!("{volume}%");
        let st = sys(12.0 * u, false, MUTED);
        let (lw, _) = r.measure("100%", st);
        let (tw, _) = r.measure(&label, st);
        r.draw_text(pm, x + inner - 10.0 * u - tw, y + 9.0 * u, &label, st);
        let track_x = x + 88.0 * u;
        let track_w = inner - 88.0 * u - 20.0 * u - lw;
        let track = Rect::from_xywh(track_x, y + 14.0 * u, track_w, 4.0 * u);
        fill_round(pm, track, 2.0 * u, TRACK);
        let t = volume as f32 / 100.0;
        let color = if enabled { ACCENT } else { MUTED };
        fill_round(
            pm,
            Rect::from_xywh(track_x, y + 14.0 * u, (track_w * t).max(1.0), 4.0 * u),
            2.0 * u,
            color,
        );
        let knob = Rect::from_xywh(
            track_x + track_w * t - 7.0 * u,
            y + 9.0 * u,
            14.0 * u,
            14.0 * u,
        );
        fill_round(pm, knob, 7.0 * u, CARD);
        stroke_round(
            pm,
            knob,
            7.0 * u,
            if self.dragging || self.hover == Some(Hit::Slider) {
                ACCENT
            } else {
                BORDER
            },
            1.5 * u,
        );
        if let Some(area) = Rect::from_xywh(track_x - 8.0 * u, y, track_w + 16.0 * u, ROW * u) {
            self.hits.push((area, Hit::Slider));
            self.slider = Rect::from_xywh(track_x, y, track_w, ROW * u);
        }
        y + ROW * u
    }

    fn button(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        rect: Option<Rect>,
        label: &str,
        b: Button,
        quiet: bool,
    ) {
        let u = self.ui;
        let Some(rect) = rect else { return };
        let hover = self.hover == Some(Hit::Button(b));
        let (bg, fg) = match (quiet, hover) {
            (false, false) => (ACCENT_SOFT, INK),
            (false, true) => (ACCENT, CARD),
            (true, false) => (CARD, INK),
            (true, true) => (HOVER, INK),
        };
        fill_round(pm, Some(rect), 9.0 * u, bg);
        if quiet {
            stroke_round(pm, Some(rect), 9.0 * u, BORDER, 1.0);
        }
        let st = sys(
            13.0 * u,
            !quiet,
            if b == Button::Quit && hover {
                DANGER
            } else {
                fg
            },
        );
        let (tw, th) = r.measure(label, st);
        r.draw_text(
            pm,
            (rect.left() + (rect.width() - tw) / 2.0).round(),
            (rect.top() + (rect.height() - th) / 2.0).round(),
            label,
            st,
        );
        self.hits.push((rect, Hit::Button(b)));
    }

    fn button_icon(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        rect: Option<Rect>,
        label: &str,
        b: Button,
    ) {
        let u = self.ui;
        let Some(rect) = rect else { return };
        if self.hover == Some(Hit::Button(b)) {
            fill_round(pm, Some(rect), 8.0 * u, HOVER);
        }
        let st = sys(18.0 * u, false, MUTED);
        let (tw, th) = r.measure(label, st);
        r.draw_text(
            pm,
            (rect.left() + (rect.width() - tw) / 2.0).round(),
            (rect.top() + (rect.height() - th) / 2.0).round(),
            label,
            st,
        );
        self.hits.push((rect, Hit::Button(b)));
    }
}

trait FilterVolume {
    fn filter_volume(self, s: &Settings) -> Self;
}

impl FilterVolume for (bool, Option<Action>) {
    /// Skip slider events that would not change the volume.
    fn filter_volume(self, s: &Settings) -> Self {
        match self {
            (
                redraw,
                Some(Action::Volume {
                    volume,
                    done: false,
                }),
            ) if volume == s.volume => (redraw, None),
            other => other,
        }
    }
}

/// Logical height of the whole panel.
fn content_height() -> f32 {
    let rows = 3 + 4 + 3 + 3;
    HEADER
        + 28.0
        + SECTION_GAP
        + 4.0 * (SECTION_TITLE + SECTION_GAP)
        + rows as f32 * ROW
        + 28.0
        + 8.0
        + 18.0
}

fn sys(size: f32, bold: bool, color: ColorU8) -> TextStyle {
    TextStyle { size, bold, color }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.left() && x < r.right() && y >= r.top() && y < r.bottom()
}

fn paint(c: ColorU8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.red(), c.green(), c.blue(), c.alpha()));
    p.anti_alias = true;
    p
}

fn round_rect(r: Rect, rad: f32) -> Option<tiny_skia::Path> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = 0.5523 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

fn fill_round(pm: &mut Pixmap, r: Option<Rect>, rad: f32, c: ColorU8) {
    if let Some(p) = r.and_then(|r| round_rect(r, rad)) {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_round(pm: &mut Pixmap, r: Option<Rect>, rad: f32, c: ColorU8, width: f32) {
    if let Some(p) = r.and_then(|r| round_rect(r, rad)) {
        let s = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&p, &paint(c), &s, Transform::identity(), None);
    }
}

fn fill_card(pm: &mut Pixmap, card: Option<Rect>, u: f32) {
    fill_round(pm, card, 12.0 * u, CARD);
    stroke_round(pm, card, 12.0 * u, BORDER, 1.0);
}

trait Inset {
    fn inset(self, d: f32) -> Option<Rect>;
}

impl Inset for Rect {
    fn inset(self, d: f32) -> Option<Rect> {
        Rect::from_ltrb(
            self.left() + d,
            self.top() + d,
            self.right() - d,
            self.bottom() - d,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            autostart_label: "Start with Windows",
            settings: Settings::default(),
            visible: true,
            mcp_ok: true,
            mcp_status: "http://127.0.0.1:7777/mcp".into(),
            version: "test",
            ..View::default()
        }
    }

    fn center(r: Rect) -> (f32, f32) {
        ((r.left() + r.right()) / 2.0, (r.top() + r.bottom()) / 2.0)
    }

    #[test]
    fn clicks_produce_actions() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.draw(&mut pm, &mut r, &v);
        assert!(
            p.content_height <= h as f32 + 1.0,
            "{} > {h}",
            p.content_height
        );

        let find = |p: &Panel, hit: Hit| {
            p.hits
                .iter()
                .find(|(_, h)| *h == hit)
                .map(|(r, _)| center(*r))
                .unwrap()
        };

        let (x, y) = find(&p, Hit::Choice(Field::Size, 1));
        match p.pointer_up(x, y, &v) {
            Some(Action::Settings(s)) => assert_eq!(s.size, SIZES[1].0),
            other => panic!("{other:?}"),
        }
        let (x, y) = find(&p, Hit::Toggle(Toggle::Sound));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if !s.sound));
        let (x, y) = find(&p, Hit::Button(Button::Quit));
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Quit));
        // Choosing what is already selected changes nothing.
        let (x, y) = find(&p, Hit::Choice(Field::Speed, 1));
        assert_eq!(p.pointer_up(x, y, &v), None);
    }

    #[test]
    fn slider_drags_volume() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.draw(&mut pm, &mut r, &v);
        let track = p.slider.unwrap();
        let y = (track.top() + track.bottom()) / 2.0;
        assert_eq!(
            p.pointer_down(track.left(), y),
            Some(Action::Volume {
                volume: 0,
                done: false
            })
        );
        let (_, a) = p.pointer_move(track.right() + 50.0, y, &v.settings);
        assert_eq!(
            a,
            Some(Action::Volume {
                volume: 100,
                done: false
            })
        );
        assert_eq!(
            p.pointer_up(track.left() + track.width() * 0.25, y, &v),
            Some(Action::Volume {
                volume: 25,
                done: true
            })
        );
    }
}
