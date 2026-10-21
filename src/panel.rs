//! Settings panel: drawn by us (not the system menu) so it looks the same on Windows and
//! macOS and matches the dog's pixel-art style. Platform-independent: it draws into a pixmap,
//! knows its own hit areas and turns clicks and drags into [`Action`]s.

use tiny_skia::{
    Color, ColorU8, FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use crate::render::{Renderer, TextStyle};
use crate::settings::{Ground, LIFTS, NOTIFY_SECS, SIZES, SPEEDS, Settings, TEXT_SIZES};

/// Logical width of the panel; height is fixed so switching tabs does not resize it.
pub const WIDTH: f32 = 300.0;
const PAD: f32 = 12.0;
const ROW: f32 = 32.0;
const GAP: f32 = 10.0;
const HEADER: f32 = 30.0;
const TABS: f32 = 48.0;
const QUICK: f32 = 28.0;
/// Area under the tabs: fits the Dog tab (quick actions above a three-row card) and the
/// four-row tabs.
const BODY: f32 = QUICK + GAP + 4.0 * ROW;
const FOOTER: f32 = 24.0;
const RADIUS: f32 = 14.0;
/// The arrow pointing at the menu bar icon (macOS).
const ARROW_H: f32 = 9.0;
const ARROW_W: f32 = 20.0;

const BG: ColorU8 = rgb(0xfb, 0xf7, 0xf1);
const CARD: ColorU8 = rgb(0xff, 0xff, 0xff);
const BORDER: ColorU8 = rgb(0xe8, 0xdf, 0xd2);
const INK: ColorU8 = rgb(0x2b, 0x2a, 0x33);
const MUTED: ColorU8 = rgb(0x76, 0x6e, 0x64);
const ACCENT: ColorU8 = rgb(0xe0, 0x8e, 0x2b);
/// Accent dark enough for text on [`ACCENT_SOFT`].
const ACCENT_INK: ColorU8 = rgb(0x9a, 0x55, 0x12);
const ACCENT_SOFT: ColorU8 = rgb(0xfc, 0xe9, 0xcf);
const TRACK: ColorU8 = rgb(0xe9, 0xe2, 0xd8);
const HOVER: ColorU8 = rgb(0xf6, 0xef, 0xe5);
const DANGER: ColorU8 = rgb(0xc8, 0x4b, 0x3c);
const AWAKE: ColorU8 = rgb(0x4f, 0xa8, 0x6a);

/// The panel's tabs, left to right.
/// Shown on the Dog tab.
#[cfg(target_os = "macos")]
const SHORTCUTS: &str = "Bubble keys: ⌥⌘↩ yes · ⌥⌘1-4 · ⌥⌘⌫ no · ⌥⌘L later";
#[cfg(not(target_os = "macos"))]
const SHORTCUTS: &str = "Ctrl+Alt + Enter yes · 1-4 · Backspace no · L later";

const TAB_NAMES: [&str; 5] = ["Dog", "Walk", "Sound", "History", "Connect"];
const TAB_HISTORY: usize = 3;
/// Quiet hours the "Night" choice sets.
const NIGHT: (&str, &str) = ("22:00", "08:00");

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
    /// The version in the footer: checks for updates (or restarts into an installed one).
    CheckUpdate,
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
    /// A newer version: (version, installed and waiting for a restart).
    pub update: Option<(String, bool)>,
    /// Recent notifications, newest first.
    pub history: Vec<HistoryItem>,
    /// Do-not-disturb is on right now (switched on or in quiet hours).
    pub quiet: bool,
    /// Notifications missed while the dog was hidden (badge on the History tab).
    pub missed: usize,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryItem {
    /// Local time it arrived, "HH:MM".
    pub time: String,
    pub source: Option<String>,
    pub title: String,
    pub outcome: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Tab(usize),
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
    Quiet,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Toggle {
    Sound,
    SoundOnPet,
    QuietFullscreen,
    Wander,
    Mcp,
    Autostart,
    AutoUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Button {
    Test,
    Sleep,
    Hide,
    CopyMcp,
    OpenDir,
    Version,
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
    tab: usize,
    /// Draws an arrow at the top pointing at this x (physical, panel-relative).
    arrow: Option<f32>,
    /// Missed while hidden; shown as a badge on History.
    missed: usize,
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
            tab: 0,
            arrow: None,
            missed: 0,
        }
    }

    pub fn set_ui_scale(&mut self, ui: f32) {
        self.ui = ui;
    }

    /// Selects a tab (0-based); out-of-range values pick the last one.
    pub fn set_tab(&mut self, tab: usize) {
        self.tab = tab.min(TAB_NAMES.len() - 1);
    }

    /// Opened after notifications were missed while hidden: start on History, with a badge
    /// until another tab is chosen.
    pub fn show_missed(&mut self, count: usize) {
        self.tab = TAB_HISTORY;
        self.missed = count;
    }

    /// Height of the area under the tabs (logical): the Dog tab spends part of it on the
    /// quick actions, so every tab is the same height and switching does not resize.
    fn body_height(&self) -> f32 {
        if self.tab == 0 {
            BODY - QUICK - GAP
        } else {
            BODY
        }
    }

    /// Adds an arrow on the top edge pointing up at `x` (physical pixels from the panel's
    /// left edge). The panel grows by the arrow's height; call before [`Panel::size`].
    pub fn set_arrow(&mut self, x: Option<f32>) {
        self.arrow = x;
    }

    /// Height of the arrow strip above the panel body, physical pixels.
    fn top(&self) -> f32 {
        if self.arrow.is_some() {
            ARROW_H * self.ui
        } else {
            0.0
        }
    }

    /// Physical size for a panel that may be at most `max_height` physical pixels tall.
    pub fn size(&self, max_height: u32) -> (u32, u32) {
        let w = (WIDTH * self.ui).ceil() as u32;
        let h = (content_height() * self.ui + self.top()).ceil() as u32;
        (w, h.min(max_height))
    }

    /// Mouse wheel; `delta` in lines (positive = up).
    pub fn scroll_by(&mut self, delta: f32, view_height: u32) {
        let max = (self.content_height - view_height as f32).max(0.0);
        self.scroll = (self.scroll - delta * ROW * self.ui).clamp(0.0, max);
    }

    pub fn draw(&mut self, pm: &mut Pixmap, r: &mut Renderer, v: &View) {
        let u = self.ui;
        self.hits.clear();
        self.slider = None;
        pm.fill(Color::TRANSPARENT);
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        let top = self.top();
        let body = Rect::from_xywh(0.0, top, w, h - top);
        fill_round(pm, body, RADIUS * u, BG);
        if let Some(ax) = self.arrow {
            fill_arrow(pm, ax, top, u, BG);
        }
        stroke_round(
            pm,
            Rect::from_xywh(0.5, top + 0.5, w - 1.0, h - top - 1.0),
            RADIUS * u,
            BORDER,
            1.0,
        );
        if let Some(ax) = self.arrow {
            stroke_arrow(pm, ax, top, u, BORDER);
        }

        let mut y = top - self.scroll + PAD * u;
        let x = PAD * u;
        let inner = w - 2.0 * x;

        y = self.header(pm, r, y, x, inner, v);
        y = self.tabs(pm, r, y, x, inner, self.missed.max(v.missed));
        // The dog's quick actions belong on its own tab.
        if self.tab == 0 {
            y = self.quick_actions(pm, r, y, x, inner, v);
        }
        let body_top = y;
        match self.tab {
            0 => self.tab_dog(pm, r, y, x, inner, &v.settings),
            1 => self.tab_walk(pm, r, y, x, inner, &v.settings),
            2 => self.tab_sound(pm, r, y, x, inner, &v.settings),
            TAB_HISTORY => self.tab_history(pm, r, y, x, inner, v),
            _ => self.tab_connect(pm, r, y, x, inner, v),
        }
        y = body_top + self.body_height() * u + GAP * u;
        y = self.footer(pm, r, y, x, inner, v);
        self.content_height = y + self.scroll - top;
    }

    /// Name, the dog's state as a colored pill, close button.
    fn header(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let name_x = x + 4.0 * u;
        let name = sys(15.0 * u, true, INK);
        let (tw, th) = r.measure("Dost", name);
        r.draw_text(pm, name_x, y + (HEADER * u - th) / 2.0, "Dost", name);

        let (label, dot) = match (v.visible, v.sleeping) {
            (false, true) => ("Hidden · asleep", MUTED),
            (true, true) => ("Sleeping", MUTED),
            (false, false) => ("Hidden", MUTED),
            (true, false) if v.quiet => ("Quiet", ACCENT),
            (true, false) => ("Awake", AWAKE),
        };
        let st = sys(11.5 * u, false, MUTED);
        let (lw, lh) = r.measure(label, st);
        let pill = Rect::from_xywh(
            name_x + tw + 10.0 * u,
            y + (HEADER * u - 20.0 * u) / 2.0,
            lw + 24.0 * u,
            20.0 * u,
        );
        if let Some(p) = pill {
            fill_round(pm, Some(p), 10.0 * u, CARD);
            stroke_round(pm, Some(p), 10.0 * u, BORDER, 1.0);
            fill_round(
                pm,
                Rect::from_xywh(p.left() + 8.0 * u, p.top() + 7.0 * u, 6.0 * u, 6.0 * u),
                3.0 * u,
                dot,
            );
            r.draw_text(
                pm,
                p.left() + 18.0 * u,
                (p.top() + (p.height() - lh) / 2.0).round(),
                label,
                st,
            );
        }

        let close = Rect::from_xywh(
            x + inner - 26.0 * u,
            y + (HEADER * u - 26.0 * u) / 2.0,
            26.0 * u,
            26.0 * u,
        );
        self.button_icon(pm, close, Glyph::Close, Button::Close);
        y + HEADER * u + GAP * u
    }

    /// Icon tabs in a tray, the active one tinted with the accent.
    #[allow(clippy::too_many_arguments)]
    fn tabs(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        missed: usize,
    ) -> f32 {
        let u = self.ui;
        let tray = Rect::from_xywh(x, y, inner, TABS * u);
        fill_round(pm, tray, 12.0 * u, CARD);
        stroke_round(pm, tray, 12.0 * u, BORDER, 1.0);
        let gap = 2.0 * u;
        let tw = (inner - 8.0 * u - gap * (TAB_NAMES.len() - 1) as f32) / TAB_NAMES.len() as f32;
        for (i, name) in TAB_NAMES.iter().enumerate() {
            let Some(cell) = Rect::from_xywh(
                x + 4.0 * u + i as f32 * (tw + gap),
                y + 4.0 * u,
                tw,
                (TABS - 8.0) * u,
            ) else {
                continue;
            };
            let on = self.tab == i;
            let hover = self.hover == Some(Hit::Tab(i));
            if on {
                fill_round(pm, Some(cell), 8.0 * u, ACCENT_SOFT);
            } else if hover {
                fill_round(pm, Some(cell), 8.0 * u, HOVER);
            }
            let color = if on { ACCENT_INK } else { MUTED };
            let glyph = [
                Glyph::Paw,
                Glyph::Steps,
                Glyph::Speaker,
                Glyph::Clock,
                Glyph::Plug,
            ][i];
            draw_glyph(
                pm,
                glyph,
                cell.left() + cell.width() / 2.0,
                cell.top() + 13.0 * u,
                u,
                color,
            );
            let st = sys(10.5 * u, on, color);
            let (lw, _) = r.measure(name, st);
            r.draw_text(
                pm,
                (cell.left() + (cell.width() - lw) / 2.0).round(),
                (cell.top() + 22.0 * u).round(),
                name,
                st,
            );
            self.hits.push((cell, Hit::Tab(i)));
            if i == TAB_HISTORY && missed > 0 {
                let label = if missed > 9 {
                    "9+".to_string()
                } else {
                    missed.to_string()
                };
                let st = sys(9.5 * u, true, rgb(0xff, 0xff, 0xff));
                let (lw, lh) = r.measure(&label, st);
                let bw = (lw + 8.0 * u).max(16.0 * u);
                let badge = Rect::from_xywh(
                    cell.left() + cell.width() / 2.0 + 4.0 * u,
                    cell.top() + 2.0 * u,
                    bw,
                    16.0 * u,
                );
                if let Some(b) = badge {
                    fill_round(pm, Some(b), 8.0 * u, DANGER);
                    r.draw_text(
                        pm,
                        (b.left() + (b.width() - lw) / 2.0).round(),
                        (b.top() + (b.height() - lh) / 2.0).round(),
                        &label,
                        st,
                    );
                }
            }
        }
        y + TABS * u + GAP * u
    }

    fn quick_actions(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let bw = (inner - 2.0 * 8.0 * u) / 3.0;
        let quick = [
            (Button::Test, "Test"),
            (Button::Sleep, if v.sleeping { "Wake" } else { "Sleep" }),
            (Button::Hide, if v.visible { "Hide" } else { "Show" }),
        ];
        for (i, (b, label)) in quick.into_iter().enumerate() {
            let rect = Rect::from_xywh(x + i as f32 * (bw + 8.0 * u), y, bw, QUICK * u);
            self.button(pm, r, rect, label, b, true);
        }
        y + QUICK * u + GAP * u
    }

    fn tab_dog(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        s: &Settings,
    ) {
        let card = self.card_start(y, 3);
        fill_card(pm, card, self.ui);
        let mut cy = y;
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
        // The bubble's keyboard shortcuts, so they can be found.
        let u = self.ui;
        let st = sys(11.0 * u, false, MUTED);
        let (_, th) = r.measure(SHORTCUTS, st);
        r.draw_text(
            pm,
            x + 4.0 * u,
            cy + GAP * u + (ROW * u - GAP * u - th) / 2.0,
            SHORTCUTS,
            st,
        );
    }

    fn tab_walk(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        s: &Settings,
    ) {
        let card = self.card_start(y, 4);
        fill_card(pm, card, self.ui);
        let mut cy = y;
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
        self.choice_row(
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
    }

    fn tab_sound(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        s: &Settings,
    ) {
        let card = self.card_start(y, 5);
        fill_card(pm, card, self.ui);
        let mut cy = y;
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
        let quiet = if s.quiet {
            2
        } else if s.quiet_from != s.quiet_to {
            1
        } else {
            0
        };
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Quiet",
            Field::Quiet,
            &["Off", "Night", "On"],
            Some(quiet),
        );
        self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Quiet in full screen",
            s.quiet_fullscreen,
            Toggle::QuietFullscreen,
        );
    }

    /// The latest notifications: time, source, title and what happened.
    fn tab_history(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) {
        let u = self.ui;
        let card = self.card_start(y, 5);
        fill_card(pm, card, u);
        if v.history.is_empty() {
            let st = sys(12.5 * u, false, MUTED);
            let msg = "Nothing yet. Notifications show up here.";
            let (tw, th) = r.measure(msg, st);
            r.draw_text(
                pm,
                (x + (inner - tw) / 2.0).round(),
                (y + (5.0 * ROW * u - th) / 2.0).round(),
                msg,
                st,
            );
            return;
        }
        for (i, h) in v.history.iter().take(5).enumerate() {
            let ry = y + i as f32 * ROW * u;
            if i > 0
                && let Some(rule) = Rect::from_xywh(x + 12.0 * u, ry, inner - 24.0 * u, 1.0)
            {
                fill_round(pm, Some(rule), 0.0, TRACK);
            }
            let time = sys(11.0 * u, false, MUTED);
            let (tw, _) = r.measure("00:00", time);
            r.draw_text(pm, x + 12.0 * u, ry + 9.0 * u, &h.time, time);
            let tx = x + 12.0 * u + tw + 10.0 * u;
            let outcome = sys(11.0 * u, false, outcome_color(&h.outcome));
            let (ow, _) = r.measure(&h.outcome, outcome);
            let ox = x + inner - 12.0 * u - ow;
            r.draw_text(pm, ox, ry + 9.0 * u, &h.outcome, outcome);
            let label = match &h.source {
                Some(s) => format!("{s} · {}", h.title),
                None => h.title.clone(),
            };
            let st = sys(12.5 * u, false, INK);
            let fitted = fit(r, &label, st, ox - tx - 8.0 * u);
            r.draw_text(pm, tx, ry + 8.0 * u, &fitted, st);
        }
    }

    fn tab_connect(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) {
        let u = self.ui;
        let s = &v.settings;
        let card = self.card_start(y, 4);
        fill_card(pm, card, u);
        let mut cy = y;
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
        self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Update automatically",
            s.auto_update,
            Toggle::AutoUpdate,
        );
    }

    /// Settings file and quit as quiet text links, version on the right.
    fn footer(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let Some(rule) = Rect::from_xywh(x, y - GAP * u / 2.0, inner, 1.0) else {
            return y;
        };
        fill_round(pm, Some(rule), 0.0, BORDER);
        let mut lx = x;
        for (label, b) in [("Settings file", Button::OpenDir), ("Quit", Button::Quit)] {
            let hover = self.hover == Some(Hit::Button(b));
            let color = match (b, hover) {
                (Button::Quit, true) => DANGER,
                (_, true) => INK,
                _ => MUTED,
            };
            let st = sys(12.0 * u, false, color);
            let (tw, th) = r.measure(label, st);
            let Some(rect) = Rect::from_xywh(lx, y, tw + 16.0 * u, FOOTER * u) else {
                continue;
            };
            if hover {
                fill_round(pm, Some(rect), 6.0 * u, HOVER);
            }
            r.draw_text(
                pm,
                rect.left() + 8.0 * u,
                (rect.top() + (rect.height() - th) / 2.0).round(),
                label,
                st,
            );
            self.hits.push((rect, Hit::Button(b)));
            lx = rect.right() + 4.0 * u;
        }
        // The version; a newer one shows up here, and clicking checks / restarts.
        let hover = self.hover == Some(Hit::Button(Button::Version));
        let (version, color) = match &v.update {
            Some((nv, true)) => (format!("v{nv} ready · Restart"), ACCENT_INK),
            Some((nv, false)) => (format!("v{nv} is out"), ACCENT_INK),
            None if hover => ("Check for updates".to_string(), INK),
            None => (format!("v{}", v.version), MUTED),
        };
        let st = sys(11.0 * u, v.update.is_some(), color);
        let (vw, vh) = r.measure(&version, st);
        if let Some(rect) = Rect::from_xywh(x + inner - vw - 12.0 * u, y, vw + 12.0 * u, FOOTER * u)
        {
            if hover {
                fill_round(pm, Some(rect), 6.0 * u, HOVER);
            }
            self.hits.push((rect, Hit::Button(Button::Version)));
        }
        r.draw_text(
            pm,
            x + inner - vw - 6.0 * u,
            (y + (FOOTER * u - vh) / 2.0).round(),
            &version,
            st,
        );
        y + FOOTER * u + PAD * u
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
            Hit::Tab(i) => {
                if self.tab != i {
                    self.tab = i;
                    self.scroll = 0.0;
                    self.missed = 0;
                }
                return None;
            }
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
                Field::Quiet => {
                    s.quiet = i == 2;
                    let (from, to) = if i == 1 { NIGHT } else { (NIGHT.0, NIGHT.0) };
                    s.quiet_from = from.into();
                    s.quiet_to = to.into();
                }
            },
            Hit::Toggle(t) => match t {
                Toggle::Sound => s.sound = !s.sound,
                Toggle::SoundOnPet => s.sound_on_pet = !s.sound_on_pet,
                Toggle::QuietFullscreen => s.quiet_fullscreen = !s.quiet_fullscreen,
                Toggle::Wander => s.wander = !s.wander,
                Toggle::Mcp => s.mcp_enabled = !s.mcp_enabled,
                Toggle::AutoUpdate => s.auto_update = !s.auto_update,
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
                    Button::Version => Action::CheckUpdate,
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

    fn button_icon(&mut self, pm: &mut Pixmap, rect: Option<Rect>, glyph: Glyph, b: Button) {
        let u = self.ui;
        let Some(rect) = rect else { return };
        let hover = self.hover == Some(Hit::Button(b));
        if hover {
            fill_round(pm, Some(rect), 8.0 * u, HOVER);
        }
        draw_glyph(
            pm,
            glyph,
            rect.left() + rect.width() / 2.0,
            rect.top() + rect.height() / 2.0,
            u,
            if hover { INK } else { MUTED },
        );
        self.hits.push((rect, Hit::Button(b)));
    }
}

/// Small line icons, drawn on a 16-unit grid centred on (cx, cy).
#[derive(Clone, Copy)]
enum Glyph {
    Close,
    Paw,
    Steps,
    Speaker,
    Clock,
    Plug,
}

fn draw_glyph(pm: &mut Pixmap, g: Glyph, cx: f32, cy: f32, u: f32, c: ColorU8) {
    let p = |x: f32, y: f32| (cx + x * u, cy + y * u);
    let stroke = |pm: &mut Pixmap, pb: PathBuilder, w: f32| {
        if let Some(path) = pb.finish() {
            let s = Stroke {
                width: w * u,
                line_cap: LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Stroke::default()
            };
            pm.stroke_path(&path, &paint(c), &s, Transform::identity(), None);
        }
    };
    let fill = |pm: &mut Pixmap, pb: PathBuilder| {
        if let Some(path) = pb.finish() {
            pm.fill_path(
                &path,
                &paint(c),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    };
    let oval = |pb: &mut PathBuilder, x: f32, y: f32, rx: f32, ry: f32| {
        let (ox, oy) = p(x - rx, y - ry);
        if let Some(r) = Rect::from_xywh(ox, oy, 2.0 * rx * u, 2.0 * ry * u) {
            pb.push_oval(r);
        }
    };
    match g {
        Glyph::Close => {
            let mut pb = PathBuilder::new();
            for (a, b) in [((-4.5, -4.5), (4.5, 4.5)), ((4.5, -4.5), (-4.5, 4.5))] {
                let (x0, y0) = p(a.0, a.1);
                let (x1, y1) = p(b.0, b.1);
                pb.move_to(x0, y0);
                pb.line_to(x1, y1);
            }
            stroke(pm, pb, 1.6);
        }
        Glyph::Paw => {
            let mut pb = PathBuilder::new();
            oval(&mut pb, 0.0, 3.0, 4.2, 3.4);
            oval(&mut pb, -5.4, -1.2, 1.7, 2.1);
            oval(&mut pb, -2.1, -4.6, 1.7, 2.2);
            oval(&mut pb, 2.1, -4.6, 1.7, 2.2);
            oval(&mut pb, 5.4, -1.2, 1.7, 2.1);
            fill(pm, pb);
        }
        Glyph::Steps => {
            // Two footprints, one ahead of the other.
            let mut pb = PathBuilder::new();
            oval(&mut pb, -3.2, 1.6, 2.4, 3.6);
            oval(&mut pb, -3.2, 6.2, 1.6, 1.3);
            oval(&mut pb, 3.2, -3.4, 2.4, 3.6);
            oval(&mut pb, 3.2, 1.2, 1.6, 1.3);
            fill(pm, pb);
        }
        Glyph::Speaker => {
            let mut pb = PathBuilder::new();
            let pts = [
                (-6.5, -2.2),
                (-3.5, -2.2),
                (0.5, -5.8),
                (0.5, 5.8),
                (-3.5, 2.2),
                (-6.5, 2.2),
            ];
            let (x, y) = p(pts[0].0, pts[0].1);
            pb.move_to(x, y);
            for &(px, py) in &pts[1..] {
                let (x, y) = p(px, py);
                pb.line_to(x, y);
            }
            pb.close();
            fill(pm, pb);
            let mut pb = PathBuilder::new();
            for (r, a) in [(3.4, 0.75), (6.4, 0.8)] {
                let (x0, y0) = p(1.5 + r * f32::cos(-a), r * f32::sin(-a));
                let (xm, ym) = p(1.5 + r / f32::cos(a), 0.0);
                let (x1, y1) = p(1.5 + r * f32::cos(a), r * f32::sin(a));
                pb.move_to(x0, y0);
                pb.quad_to(xm, ym, x1, y1);
            }
            stroke(pm, pb, 1.5);
        }
        Glyph::Clock => {
            let mut pb = PathBuilder::new();
            let (x, y) = p(0.0, 0.0);
            pb.push_circle(x, y, 6.5 * u);
            stroke(pm, pb, 1.6);
            let mut pb = PathBuilder::new();
            let (x0, y0) = p(0.0, -3.8);
            let (x1, y1) = p(0.0, 0.0);
            let (x2, y2) = p(2.8, 1.8);
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            pb.line_to(x2, y2);
            stroke(pm, pb, 1.6);
        }
        Glyph::Plug => {
            let mut pb = PathBuilder::new();
            for px in [-2.5, 2.5] {
                let (x0, y0) = p(px, -7.0);
                let (x1, y1) = p(px, -3.5);
                pb.move_to(x0, y0);
                pb.line_to(x1, y1);
            }
            let (x0, y0) = p(0.0, 3.5);
            let (x1, y1) = p(0.0, 7.0);
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            stroke(pm, pb, 1.6);
            let (bx, by) = p(-5.0, -3.5);
            fill_round(pm, Rect::from_xywh(bx, by, 10.0 * u, 7.5 * u), 2.5 * u, c);
        }
    }
}

/// Outline of the arrow on the panel's top edge: a soft-tipped triangle pointing up at `ax`.
fn arrow_path(ax: f32, top: f32, u: f32) -> Option<tiny_skia::Path> {
    let hw = ARROW_W / 2.0 * u;
    let mut pb = PathBuilder::new();
    pb.move_to(ax - hw, top + 0.5);
    pb.line_to(ax - 2.5 * u, 2.0 * u);
    pb.quad_to(ax, 0.0, ax + 2.5 * u, 2.0 * u);
    pb.line_to(ax + hw, top + 0.5);
    pb.finish()
}

fn fill_arrow(pm: &mut Pixmap, ax: f32, top: f32, u: f32, c: ColorU8) {
    let Some(path) = arrow_path(ax, top, u) else {
        return;
    };
    // Overlap the body by a pixel so no seam shows.
    let mut pb = PathBuilder::new();
    pb.push_path(&path);
    pb.line_to(ax + ARROW_W / 2.0 * u, top + 2.0);
    pb.line_to(ax - ARROW_W / 2.0 * u, top + 2.0);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_arrow(pm: &mut Pixmap, ax: f32, top: f32, u: f32, c: ColorU8) {
    // Cover the body's top border under the arrow, then outline the arrow.
    let hw = ARROW_W / 2.0 * u;
    if let Some(r) = Rect::from_xywh(ax - hw + 1.0, top, 2.0 * hw - 2.0, 1.5) {
        pm.fill_rect(r, &paint(BG), Transform::identity(), None);
    }
    if let Some(path) = arrow_path(ax, top, u) {
        let s = Stroke {
            width: 1.0,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(c), &s, Transform::identity(), None);
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

/// Logical height of the whole panel (without the arrow).
fn content_height() -> f32 {
    PAD + HEADER + GAP + TABS + GAP + BODY + GAP + FOOTER + PAD
}

fn sys(size: f32, bold: bool, color: ColorU8) -> TextStyle {
    TextStyle { size, bold, color }
}

/// Shortens `text` with "…" to fit `max` physical pixels.
fn fit(r: &mut Renderer, text: &str, st: TextStyle, max: f32) -> String {
    if r.measure(text, st).0 <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        if r.measure(&s, st).0 <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
        + "…"
}

fn outcome_color(outcome: &str) -> ColorU8 {
    if outcome.starts_with("answered") || outcome == "finished" || outcome == "opened link" {
        AWAKE
    } else if outcome.starts_with("expired")
        || outcome.starts_with("skipped")
        || outcome.starts_with("missed")
    {
        DANGER
    } else {
        MUTED
    }
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
        let (x, y) = find(&p, Hit::Button(Button::Quit));
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Quit));

        // Sound lives on its own tab: switching tabs is not an action.
        let (x, y) = find(&p, Hit::Tab(2));
        assert_eq!(p.pointer_up(x, y, &v), None);
        p.draw(&mut pm, &mut r, &v);
        assert!(
            !p.hits
                .iter()
                .any(|(_, h)| *h == Hit::Choice(Field::Size, 1))
        );
        let (x, y) = find(&p, Hit::Toggle(Toggle::Sound));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if !s.sound));

        p.set_tab(1);
        p.draw(&mut pm, &mut r, &v);
        // Choosing what is already selected changes nothing.
        let (x, y) = find(&p, Hit::Choice(Field::Speed, 1));
        assert_eq!(p.pointer_up(x, y, &v), None);
    }

    #[test]
    fn every_tab_fits() {
        let mut r = Renderer::new(false, Vec::new());
        for arrow in [None, Some(150.0)] {
            let mut p = Panel::new(1.0);
            p.set_arrow(arrow);
            let (w, h) = p.size(10_000);
            let mut pm = Pixmap::new(w, h).unwrap();
            for tab in 0..TAB_NAMES.len() {
                p.set_tab(tab);
                p.draw(&mut pm, &mut r, &view());
                assert!(p.content_height + p.top() <= h as f32 + 1.0, "tab {tab}");
            }
        }
    }

    #[test]
    fn slider_drags_volume() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        p.set_tab(2);
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
