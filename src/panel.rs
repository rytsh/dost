//! Settings panel: drawn by us (not the system menu) so it looks the same on Windows and
//! macOS and matches the dog's pixel-art style. Platform-independent: it draws into a pixmap,
//! knows its own hit areas and turns clicks and drags into [`Action`]s.

use tiny_skia::{
    Color, ColorU8, FillRule, LineCap, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke,
    Transform,
};

use crate::i18n::{self, t};
use crate::look::{Collar, Fur, Item, Outfit, SOURCE_PALETTE, Theme, Wear, parse_hex};
use crate::remote::PhoneWhen;
use crate::render::{Renderer, TextStyle};
use crate::sessions::{Session, State as SessionState};
use crate::settings::{
    Ground, Lang, NAME_MAX, NOTIFY_STEPS, SPEEDS, Settings, TEXT_SIZES, clean_name, parse_hhmm,
};

/// Logical width of the panel; height is fixed so switching tabs does not resize it.
pub const WIDTH: f32 = 340.0;
const PAD: f32 = 12.0;
const ROW: f32 = 32.0;
const GAP: f32 = 10.0;
const HEADER: f32 = 30.0;
const TABS: f32 = 48.0;
const QUICK: f32 = 28.0;
/// Area under the tabs: room for the longest tab (Sound, with a bark file), so nothing scrolls
/// unless a list gets long or the screen is short. The Dog tab puts its quick actions above its
/// card.
const BODY: f32 = 14.0 * ROW;
/// Small grey title above a card.
const SECTION: f32 = 24.0;
/// The toy's tile on the Play tab.
const TILE: f32 = 64.0;
/// Longest text in a field other than the name.
const TEXT_MAX: usize = 300;
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

/// Shown on the Dog tab.
#[cfg(target_os = "macos")]
const SHORTCUTS: &str = "⌥⌘↩ yes · ⌥⌘1-4 · ⌥⌘⌫ no · ⌥⌘L later";
#[cfg(not(target_os = "macos"))]
const SHORTCUTS: &str = "Ctrl+Alt + Enter yes · 1-4 · Backspace no · L later";

/// A page of settings: a tab of the panel, or an entry in the settings window's sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Dog,
    Look,
    Play,
    Sound,
    Phone,
    Agents,
    Todo,
    History,
    Connect,
    /// Answering a question in words (opened from the bubble's "Reply…").
    Reply,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Dog => "Dog",
            Page::Look => "Look",
            Page::Play => "Play",
            Page::Sound => "Sound",
            Page::Phone => "Phone",
            Page::Agents => "Agents",
            Page::Todo => "Todo",
            Page::History => "History",
            Page::Connect => "Connect",
            Page::Reply => "Reply",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Page::General => Glyph::Gear,
            Page::Dog => Glyph::Paw,
            Page::Look => Glyph::Bow,
            Page::Play => Glyph::Bone,
            Page::Sound => Glyph::Speaker,
            Page::Phone => Glyph::Phone,
            Page::Agents => Glyph::Agents,
            Page::Todo => Glyph::Check,
            Page::History => Glyph::Clock,
            Page::Connect => Glyph::Plug,
            Page::Reply => Glyph::Agents,
        }
    }
}

/// The panel's tabs, left to right.
const TABS_POPOVER: [Page; 7] = [
    Page::Dog,
    Page::Look,
    Page::Play,
    Page::Sound,
    Page::Agents,
    Page::Todo,
    Page::History,
];
/// The settings window's sidebar: groups of pages.
const SIDEBAR: [(&str, &[Page]); 4] = [
    (
        "Basics",
        &[Page::General, Page::Dog, Page::Look, Page::Play],
    ),
    ("Notifications", &[Page::Sound, Page::Phone]),
    ("Work", &[Page::Agents, Page::Todo, Page::History]),
    ("Connect", &[Page::Connect]),
];
#[cfg(test)]
const TAB_LOOK: usize = 1;
#[cfg(test)]
const TAB_PLAY: usize = 2;
#[cfg(test)]
const TAB_SOUND: usize = 3;
#[cfg(test)]
const TAB_TODO: usize = 5;
const TAB_HISTORY: usize = 6;
/// The settings window: logical size it opens at, and the sidebar's width.
pub const WINDOW_SIZE: (f32, f32) = (780.0, 580.0);
const SIDEBAR_W: f32 = 210.0;
/// Width of the label column in rows (panel, window).
const LABEL_W: f32 = 88.0;
const LABEL_W_WINDOW: f32 = 190.0;
/// Height of a session's row on the Agents page.
const SESSION_ROW: f32 = 46.0;

const fn rgb(r: u8, g: u8, b: u8) -> ColorU8 {
    ColorU8::from_rgba(r, g, b, 255)
}

/// What a click or drag on the panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// New settings to apply and save.
    Settings(Box<Settings>),
    /// Volume slider moved; `done` when the drag ended (play a test bark then).
    Volume {
        volume: u8,
        done: bool,
    },
    TestNotification,
    /// Throws the bone for the dog.
    Fetch,
    /// The Bone button was dragged out of the panel: the toy follows the mouse until let go.
    TakeToy,
    ToggleSleep,
    ToggleVisible,
    ToggleAutostart,
    CopyMcpUrl,
    OpenConfigDir,
    /// A change to the todo lists.
    Todo(crate::todo::Edit),
    /// Opens the big settings window.
    OpenSettings,
    /// An answer in words to question `id`.
    Reply {
        id: u64,
        text: String,
    },
    /// Sends a test message to the phone.
    PhoneTest,
    /// A new random ntfy topic.
    NewTopic,
    CopyText(String),
    OpenUrl(String),
    /// Forgets finished agent sessions.
    ClearSessions,
    RemoveSession(String),
    /// The version in the footer: checks for updates (or restarts into an installed one).
    CheckUpdate,
    Quit,
    Close,
}

/// State the panel shows but does not own.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub settings: Settings,
    /// How long the dog has been with the user ("2 years").
    pub age: String,
    /// What the dog wears today (the preview on the Look tab).
    pub outfit: Outfit,
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
    pub todos: Vec<crate::todo::List>,
    /// What agents are doing.
    pub sessions: Vec<Session>,
    /// A phone channel's last problem.
    pub phone_status: Option<String>,
    /// How many screens there are.
    pub screens: usize,
    /// Today is a special day (when they are on).
    pub special: Option<crate::special::Day>,
    /// The minute of silence is on.
    pub silence: bool,
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
    Tab(Page),
    /// A session on the Agents page (index): opens it; and its ✕.
    Session(usize),
    RemoveSession(usize),
    Choice(Field, usize),
    Toggle(Toggle),
    Slider,
    Button(Button),
    /// A text field.
    Text(TextField),
    Fur(usize),
    Collar(usize),
    /// The ✕ on an entry of a list.
    Remove(List, usize),
    /// The colour dot of a source colour: picks the next colour.
    SourceColor(usize),
    /// The bone on the Play tab: drag it out, or click to throw.
    ToyTile,
    /// A todo item's check box (item id).
    TodoCheck(u64),
    /// The ✕ of a todo item (item id).
    TodoRemove(u64),
    /// A list's "Clear done" and ✕ (list id).
    TodoClear(u64),
    TodoRemoveList(u64),
}

/// Fields the user types into.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TextField {
    Name,
    Birthday,
    QuietFrom,
    QuietTo,
    Lift,
    McpAddr,
    /// New entries for the lists.
    AddOrigin,
    AddBark,
    AddSource,
    /// A new item for a todo list (list id).
    AddTodo(u64),
    NewList,
    NtfyServer,
    NtfyTopic,
    NtfyToken,
    TelegramToken,
    /// The answer to a question, in words.
    Reply,
}

impl TextField {
    /// Shown as dots unless edited.
    fn secret(self) -> bool {
        matches!(self, TextField::NtfyToken | TextField::TelegramToken)
    }
}

/// Lists edited in the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
enum List {
    Origins,
    Barks,
    Sources,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Size,
    TextSize,
    Speed,
    Ground,
    Notify,
    Wear,
    Theme,
    /// Which web pages may use the MCP server.
    Browsers,
    Language,
    PhoneWhen,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Toggle {
    Sound,
    SoundOnPet,
    QuietNow,
    QuietFullscreen,
    Wander,
    Mcp,
    PrivateNetwork,
    Autostart,
    CheckUpdates,
    AutoUpdate,
    Seasonal,
    EventSounds,
    Toy,
    SpecialDays,
    Siren,
    Ntfy,
    Telegram,
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
    AllSettings,
    PhoneTest,
    NewTopic,
    CopyTopic,
    ReplySend,
    ReplyCancel,
    ClearSessions,
}

/// A key typed while a text field is being edited.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    /// Text typed or pasted (one or more characters).
    Text(String),
    Backspace,
    /// Enter: save.
    Enter,
    /// Esc: cancel.
    Escape,
}

/// The text field being edited.
#[derive(Clone, Debug)]
struct Editing {
    field: TextField,
    text: String,
    /// When editing started (the caret blinks from then).
    since: std::time::Instant,
}

pub struct Panel {
    /// Logical-to-physical scale (DPI).
    ui: f32,
    hits: Vec<(Rect, Hit)>,
    slider: Option<Rect>,
    hover: Option<Hit>,
    dragging: bool,
    /// How far the tab's body is scrolled (physical).
    scroll: f32,
    /// Height of the tab's body content (physical), for the scroll limit.
    body_content: f32,
    /// Height of the visible part of the body at the last draw (physical).
    view_h: f32,
    page: Page,
    /// The big settings window (sidebar, wide rows) rather than the small panel.
    window: bool,
    /// Where the page's cards go (physical x and width), set while drawing.
    cx: f32,
    cw: f32,
    /// The question being answered in words: (id, title, question), and the page to go back to.
    reply: Option<(u64, String, String, Page)>,
    /// Draws an arrow at the top pointing at this x (physical, panel-relative).
    arrow: Option<f32>,
    /// Missed while hidden; shown as a badge on History.
    missed: usize,
    editing: Option<Editing>,
    /// Mouse down on the toy's tile (where): dragging it away takes the toy out.
    toy_press: Option<(f32, f32)>,
    /// "Custom" was picked for Browsers: show the list of pages even while it is empty.
    custom_origins: bool,
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
            body_content: 0.0,
            view_h: 0.0,
            page: Page::Dog,
            window: false,
            cx: 0.0,
            cw: 0.0,
            reply: None,
            arrow: None,
            missed: 0,
            editing: None,
            toy_press: None,
            custom_origins: false,
        }
    }

    /// The big settings window: a sidebar of pages and wide rows; sized by the window.
    pub fn new_window(ui: f32) -> Self {
        let mut p = Panel::new(ui);
        p.window = true;
        p.page = Page::General;
        p
    }

    pub fn is_window(&self) -> bool {
        self.window
    }

    /// Opens the page that answers question `id` in words, with the field ready for typing.
    pub fn start_reply(&mut self, id: u64, title: &str, question: &str) {
        let back = if self.page == Page::Reply {
            Page::Dog
        } else {
            self.page
        };
        self.reply = Some((id, title.to_string(), question.to_string(), back));
        self.page = Page::Reply;
        self.scroll = 0.0;
        self.editing = Some(Editing {
            field: TextField::Reply,
            text: String::new(),
            since: std::time::Instant::now(),
        });
    }

    fn end_reply(&mut self) {
        if let Some((.., back)) = self.reply.take() {
            self.page = back;
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|e| e.field == TextField::Reply)
        {
            self.editing = None;
        }
    }

    /// A text field takes the keyboard (the shell sends keys to [`Panel::key`]).
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// A key while editing a field. Returns the new settings when it is saved, and whether
    /// the panel needs a redraw.
    pub fn key(&mut self, key: Key, v: &View) -> (bool, Option<Action>) {
        let Some(e) = self.editing.as_mut() else {
            return (false, None);
        };
        let max = match e.field {
            TextField::Name => NAME_MAX,
            TextField::Reply => 2000,
            _ => TEXT_MAX,
        };
        match key {
            Key::Text(t) => {
                for c in t.chars().filter(|c| !c.is_control()) {
                    if e.text.chars().count() < max {
                        e.text.push(c);
                    }
                }
            }
            Key::Backspace => {
                e.text.pop();
            }
            Key::Escape if e.field == TextField::Reply => self.end_reply(),
            Key::Escape => self.editing = None,
            Key::Enter => return (true, self.finish_editing(v)),
        }
        if let Some(e) = self.editing.as_mut() {
            e.since = std::time::Instant::now();
        }
        (true, None)
    }

    /// Ends editing; the new settings if the field changed them. Text that does not fit the
    /// field (a time that is not HH:MM...) is dropped.
    pub fn finish_editing(&mut self, v: &View) -> Option<Action> {
        let e = self.editing.take()?;
        let mut s = v.settings.clone();
        let t = e.text.trim();
        match e.field {
            TextField::Name => s.name = clean_name(t),
            TextField::Birthday => {
                if t.is_empty() || crate::look::parse_date(t).is_some() {
                    s.birthday = t.to_string();
                }
            }
            TextField::QuietFrom | TextField::QuietTo => {
                if let Some(m) = parse_hhmm(t) {
                    let hhmm = format!("{:02}:{:02}", m / 60, m % 60);
                    if e.field == TextField::QuietFrom {
                        s.quiet_from = hhmm;
                    } else {
                        s.quiet_to = hhmm;
                    }
                }
            }
            TextField::Lift => {
                if let Ok(n) = t.replace('−', "-").parse::<i32>() {
                    s.lift = n.clamp(-20, 2000);
                }
            }
            TextField::McpAddr => {
                let port_ok = t
                    .rsplit_once(':')
                    .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok());
                if port_ok {
                    s.mcp_addr = t.to_string();
                }
            }
            TextField::AddOrigin => {
                let o = t.trim_end_matches('/');
                if !o.is_empty() {
                    let list = &mut s.mcp_cors.allow_origins;
                    list.retain(|x| x != "*");
                    if !list.iter().any(|x| x == o) {
                        list.push(o.to_string());
                    }
                }
            }
            TextField::AddTodo(list) => {
                return (!t.is_empty()).then(|| {
                    Action::Todo(crate::todo::Edit::Add {
                        list: list.to_string(),
                        text: t.to_string(),
                    })
                });
            }
            TextField::NewList => {
                return (!t.is_empty())
                    .then(|| Action::Todo(crate::todo::Edit::NewList(t.to_string())));
            }
            TextField::NtfyServer => {
                let u = t.trim_end_matches('/');
                if u.starts_with("https://") || u.starts_with("http://") {
                    s.phone.ntfy.server = u.to_string();
                }
            }
            TextField::NtfyTopic => {
                let ok = t
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                if ok && t.len() <= 64 {
                    s.phone.ntfy.topic = t.to_string();
                    if !t.is_empty() {
                        s.phone.ntfy.enabled = true;
                    }
                }
            }
            TextField::NtfyToken => s.phone.ntfy.token = t.to_string(),
            TextField::TelegramToken => {
                if t != s.phone.telegram.token {
                    s.phone.telegram.token = t.to_string();
                    // Another bot: pair again.
                    s.phone.telegram.chat_id.clear();
                    if !t.is_empty() {
                        s.phone.telegram.enabled = true;
                    }
                }
            }
            TextField::Reply => {
                let id = self.reply.as_ref().map(|r| r.0);
                if t.is_empty() {
                    // Nothing typed yet: keep the field open.
                    self.editing = Some(e);
                    return None;
                }
                let text = t.to_string();
                self.end_reply();
                return id.map(|id| Action::Reply { id, text });
            }
            TextField::AddBark => {
                // Paths copied from a file manager may come quoted.
                let p = t.trim_matches(|c| c == '"' || c == '\'');
                if !p.is_empty() && !s.bark_files.iter().any(|x| x == p) {
                    s.bark_files.push(p.to_string());
                }
            }
            TextField::AddSource => {
                let k = t.to_lowercase();
                if !k.is_empty() && !s.source_colors.contains_key(&k) {
                    let used: Vec<String> = s.source_colors.values().cloned().collect();
                    let c = SOURCE_PALETTE
                        .iter()
                        .map(|c| crate::look::to_hex(*c))
                        .find(|h| !used.contains(h))
                        .unwrap_or_else(|| crate::look::to_hex(SOURCE_PALETTE[0]));
                    s.source_colors.insert(k, c);
                }
            }
        }
        (s != v.settings).then(|| Action::Settings(Box::new(s)))
    }

    /// The text a field starts with when clicked.
    fn field_value(f: TextField, s: &Settings) -> String {
        match f {
            TextField::Name => s.name.clone(),
            TextField::Birthday => s.birthday.clone(),
            TextField::QuietFrom => s.quiet_from.clone(),
            TextField::QuietTo => s.quiet_to.clone(),
            TextField::Lift => s.lift.to_string(),
            TextField::McpAddr => s.mcp_addr.clone(),
            TextField::NtfyServer => s.phone.ntfy.server.clone(),
            TextField::NtfyTopic => s.phone.ntfy.topic.clone(),
            TextField::NtfyToken => s.phone.ntfy.token.clone(),
            TextField::TelegramToken => s.phone.telegram.token.clone(),
            TextField::Reply => String::new(),
            TextField::AddOrigin
            | TextField::AddBark
            | TextField::AddSource
            | TextField::AddTodo(_)
            | TextField::NewList => String::new(),
        }
    }

    pub fn set_ui_scale(&mut self, ui: f32) {
        self.ui = ui;
    }

    /// Selects a tab of the panel (0-based); out-of-range values pick the last one.
    pub fn set_tab(&mut self, tab: usize) {
        self.set_page(TABS_POPOVER[tab.min(TABS_POPOVER.len() - 1)]);
    }

    pub fn set_page(&mut self, page: Page) {
        if self.page == Page::Reply && page != Page::Reply {
            self.end_reply();
        }
        self.page = page;
        self.scroll = 0.0;
    }

    pub fn page(&self) -> Page {
        self.page
    }

    /// Opened after notifications were missed while hidden: start on History, with a badge
    /// until another tab is chosen.
    pub fn show_missed(&mut self, count: usize) {
        self.set_tab(TAB_HISTORY);
        self.missed = count;
    }

    /// Height of the area under the tabs (logical): the Dog tab spends part of it on the
    /// quick actions, so every tab is the same height and switching does not resize.
    fn body_height(&self) -> f32 {
        if self.page == Page::Dog {
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
        if self.window {
            let (w, h) = WINDOW_SIZE;
            return (
                (w * self.ui).ceil() as u32,
                ((h * self.ui).ceil() as u32).min(max_height),
            );
        }
        let w = (WIDTH * self.ui).ceil() as u32;
        let h = (content_height() * self.ui + self.top()).ceil() as u32;
        (w, h.min(max_height))
    }

    /// Mouse wheel over the panel; `delta` in lines (positive = up). Scrolls the tab's body.
    pub fn scroll_by(&mut self, delta: f32, _view_height: u32) {
        let view = if self.view_h > 0.0 {
            self.view_h
        } else {
            self.body_height() * self.ui
        };
        let max = (self.body_content - view).max(0.0);
        self.scroll = (self.scroll - delta * ROW * self.ui).clamp(0.0, max);
    }

    pub fn draw(&mut self, pm: &mut Pixmap, r: &mut Renderer, v: &View) {
        if self.window {
            return self.draw_window(pm, r, v);
        }
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

        let mut y = top + PAD * u;
        let x = PAD * u;
        let inner = w - 2.0 * x;
        self.cx = x;
        self.cw = inner;

        y = self.header(pm, r, y, x, inner, v);
        y = self.tabs(pm, r, y, x, inner, self.missed.max(v.missed));
        // The dog's quick actions belong on its own tab.
        if self.page == Page::Dog {
            y = self.quick_actions(pm, r, y, x, inner, v);
        }
        let body_h = (self.body_height() * u).round();
        let view_h = body_h.min((h - y - (GAP + FOOTER + PAD) * u).max(0.0));
        self.body(pm, r, v, x, y, inner, view_h);
        // On a short screen the body is cut to what fits; the footer stays right under it.
        y += view_h + GAP * u;
        self.footer(pm, r, y, x, inner, v);
    }

    /// The settings window: a sidebar of pages on the left, the page on the right.
    fn draw_window(&mut self, pm: &mut Pixmap, r: &mut Renderer, v: &View) {
        let u = self.ui;
        self.hits.clear();
        self.slider = None;
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        pm.fill(color8(BG));
        // Sidebar.
        let sw = (SIDEBAR_W * u).round();
        fill_round(pm, Rect::from_xywh(0.0, 0.0, sw, h), 0.0, SIDEBAR_BG);
        fill_round(pm, Rect::from_xywh(sw, 0.0, 1.0, h), 0.0, BORDER);
        let mut y = 18.0 * u;
        let x = 12.0 * u;
        // The dog and its name.
        let name = sys(15.0 * u, true, INK);
        r.draw_dog_icon(
            pm,
            if v.sleeping {
                crate::dog::Anim::Sleep
            } else {
                crate::dog::Anim::Sit
            },
            (x + 4.0 * u) as i32,
            y as i32,
            (30.0 * u) as u32,
        );
        let shown = fit(r, &v.settings.name, name, sw - 70.0 * u);
        r.draw_text(pm, x + 44.0 * u, y + 2.0 * u, &shown, name);
        let (label, dot) = status_label(v);
        let st = sys(11.0 * u, false, MUTED);
        fill_round(
            pm,
            Rect::from_xywh(x + 44.0 * u, y + 24.0 * u, 6.0 * u, 6.0 * u),
            3.0 * u,
            dot,
        );
        r.draw_text(pm, x + 54.0 * u, y + 20.0 * u, t(label), st);
        y += 52.0 * u;
        let missed = self.missed.max(v.missed);
        let waiting = v
            .sessions
            .iter()
            .filter(|s| s.state == SessionState::Waiting)
            .count();
        for (group, pages) in SIDEBAR {
            r.draw_text(
                pm,
                x + 8.0 * u,
                y + 4.0 * u,
                t(group),
                sys(11.0 * u, true, MUTED),
            );
            y += 24.0 * u;
            for &page in pages {
                let Some(row) = Rect::from_xywh(x, y, sw - 2.0 * x, 30.0 * u) else {
                    continue;
                };
                let on = self.page == page
                    || (self.page == Page::Reply
                        && self.reply.as_ref().is_some_and(|r| r.3 == page));
                let hit = Hit::Tab(page);
                if on {
                    fill_round(pm, Some(row), 8.0 * u, ACCENT_SOFT);
                } else if self.hover == Some(hit) {
                    fill_round(pm, Some(row), 8.0 * u, HOVER);
                }
                let c = if on { ACCENT_INK } else { INK };
                draw_glyph(
                    pm,
                    page.glyph(),
                    row.left() + 18.0 * u,
                    row.top() + row.height() / 2.0,
                    u,
                    if on { ACCENT_INK } else { MUTED },
                );
                r.draw_text(
                    pm,
                    row.left() + 36.0 * u,
                    row.top() + 6.0 * u,
                    t(page.title()),
                    sys(13.0 * u, on, c),
                );
                let badge = match page {
                    Page::History => missed,
                    Page::Agents => waiting,
                    _ => 0,
                };
                if badge > 0 {
                    draw_badge(
                        pm,
                        r,
                        row.right() - 14.0 * u,
                        row.top() + 15.0 * u,
                        badge,
                        u,
                    );
                }
                self.hits.push((row, hit));
                y += 32.0 * u;
            }
            y += 8.0 * u;
        }
        // Footer of the sidebar: settings file, quit and the version.
        let fy = h - 64.0 * u;
        let mut ly = fy;
        for (label, b) in [
            ("Open settings file", Button::OpenDir),
            ("Quit", Button::Quit),
        ] {
            let hover = self.hover == Some(Hit::Button(b));
            let color = match (b, hover) {
                (Button::Quit, true) => DANGER,
                (_, true) => INK,
                _ => MUTED,
            };
            let st = sys(12.0 * u, false, color);
            let (tw, th) = r.measure(t(label), st);
            if let Some(rect) = Rect::from_xywh(x, ly, tw + 16.0 * u, 22.0 * u) {
                if hover {
                    fill_round(pm, Some(rect), 6.0 * u, HOVER);
                }
                r.draw_text(
                    pm,
                    rect.left() + 8.0 * u,
                    (rect.top() + (rect.height() - th) / 2.0).round(),
                    t(label),
                    st,
                );
                self.hits.push((rect, Hit::Button(b)));
            }
            ly += 24.0 * u;
        }
        self.version_link(pm, r, sw - 12.0 * u, fy + 24.0 * u, v);

        // The page.
        let px = sw + 28.0 * u;
        let pw = (w - px - 28.0 * u).min(560.0 * u);
        let title_y = 22.0 * u;
        let title = match (self.page, &self.reply) {
            (Page::Reply, Some((_, title, ..))) if !title.is_empty() => title.clone(),
            (p, _) => t(p.title()).to_string(),
        };
        r.draw_text(pm, px, title_y, &title, sys(22.0 * u, true, INK));
        let by = title_y + 46.0 * u;
        self.cx = px;
        self.cw = pw;
        let view_h = (h - by - 12.0 * u).max(1.0);
        self.body(pm, r, v, px, by, pw, view_h);
    }

    /// The page's body: drawn on its own canvas, scrolled, then copied into its place.
    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        v: &View,
        x: f32,
        y: f32,
        inner: f32,
        view_h: f32,
    ) {
        self.view_h = view_h;
        let Some(mut canvas) = Pixmap::new(pm.width(), (view_h.max(1.0)) as u32) else {
            return;
        };
        let first = self.hits.len();
        let end = {
            let cy = -self.scroll;
            let c = &mut canvas;
            match self.page {
                Page::General => self.page_general(c, r, cy, x, inner, v),
                Page::Dog => self.tab_dog(c, r, cy, x, inner, v),
                Page::Look => self.tab_look(c, r, cy, x, inner, v),
                Page::Play => self.tab_play(c, r, cy, x, inner, v),
                Page::Sound => self.tab_sound(c, r, cy, x, inner, &v.settings),
                Page::Phone => self.page_phone(c, r, cy, x, inner, v),
                Page::Agents => self.page_agents(c, r, cy, x, inner, v),
                Page::Todo => self.tab_todo(c, r, cy, x, inner, v),
                Page::History => self.tab_history(c, r, cy, x, inner, v),
                Page::Connect => self.tab_connect(c, r, cy, x, inner, v),
                Page::Reply => self.page_reply(c, r, cy, x, inner, v),
            }
        };
        self.body_content = end + self.scroll;
        let max = (self.body_content - view_h).max(0.0);
        if self.scroll > max {
            // The content got shorter (a list entry removed): draw again in range.
            self.scroll = max;
            self.hits.truncate(first);
            return self.draw(pm, r, v);
        }
        // Hits are in the canvas: move them into the panel, and drop what is scrolled out.
        let moved: Vec<(Rect, Hit)> = self.hits.drain(first..).collect();
        for (rect, hit) in moved {
            let Some(clipped) = Rect::from_ltrb(
                rect.left(),
                rect.top().max(0.0),
                rect.right(),
                rect.bottom().min(view_h),
            ) else {
                continue;
            };
            if let Some(r) = Rect::from_xywh(
                clipped.left(),
                clipped.top() + y,
                clipped.width(),
                clipped.height(),
            ) {
                self.hits.push((r, hit));
            }
        }
        if let Some(s) = self.slider {
            self.slider = Rect::from_xywh(s.left(), s.top() + y, s.width(), s.height());
        }
        pm.draw_pixmap(
            0,
            y.round() as i32,
            canvas.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        self.scrollbar(pm, x + inner + 4.0 * self.ui, y, view_h);
    }

    /// A thin bar on the right while the tab's body is longer than its room.
    fn scrollbar(&self, pm: &mut Pixmap, x: f32, y: f32, view_h: f32) {
        if self.body_content <= view_h + 1.0 {
            return;
        }
        let u = self.ui;
        let k = view_h / self.body_content;
        let bar_h = (view_h * k).max(20.0 * u);
        let max = self.body_content - view_h;
        let by = y + (view_h - bar_h) * (self.scroll / max).clamp(0.0, 1.0);
        fill_round(pm, Rect::from_xywh(x, by, 3.0 * u, bar_h), 1.5 * u, TRACK);
    }

    /// Name, the dog's state as a colored pill and a gear (the settings window).
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
        // Room for the pill and the buttons.
        let shown = fit(r, &v.settings.name, name, inner - 180.0 * u);
        let (tw, th) = r.measure(&shown, name);
        r.draw_text(pm, name_x, y + (HEADER * u - th) / 2.0, &shown, name);

        let (label, dot) = status_label(v);
        let label = t(label);
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

        let gear = Rect::from_xywh(
            x + inner - 26.0 * u,
            y + (HEADER * u - 26.0 * u) / 2.0,
            26.0 * u,
            26.0 * u,
        );
        self.button_icon(pm, gear, Glyph::Gear, Button::AllSettings);
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
        let n = TABS_POPOVER.len();
        let tw = (inner - 8.0 * u - gap * (n - 1) as f32) / n as f32;
        for (i, page) in TABS_POPOVER.iter().enumerate() {
            let Some(cell) = Rect::from_xywh(
                x + 4.0 * u + i as f32 * (tw + gap),
                y + 4.0 * u,
                tw,
                (TABS - 8.0) * u,
            ) else {
                continue;
            };
            let on = self.page == *page
                || (self.page == Page::Reply && self.reply.as_ref().is_some_and(|r| r.3 == *page));
            let hover = self.hover == Some(Hit::Tab(*page));
            if on {
                fill_round(pm, Some(cell), 8.0 * u, ACCENT_SOFT);
            } else if hover {
                fill_round(pm, Some(cell), 8.0 * u, HOVER);
            }
            let color = if on { ACCENT_INK } else { MUTED };
            draw_glyph(
                pm,
                page.glyph(),
                cell.left() + cell.width() / 2.0,
                cell.top() + 13.0 * u,
                u,
                color,
            );
            let name = t(page.title());
            let st = sys(if i18n::is_turkish() { 9.0 } else { 10.0 } * u, on, color);
            let shown = fit(r, name, st, cell.width() - 2.0 * u);
            let (lw, _) = r.measure(&shown, st);
            r.draw_text(
                pm,
                (cell.left() + (cell.width() - lw) / 2.0).round(),
                (cell.top() + 22.0 * u).round(),
                &shown,
                st,
            );
            self.hits.push((cell, Hit::Tab(*page)));
            if *page == Page::History && missed > 0 {
                draw_badge(
                    pm,
                    r,
                    cell.left() + cell.width() / 2.0 + 12.0 * u,
                    cell.top() + 10.0 * u,
                    missed,
                    u,
                );
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
        let bw = (inner - 2.0 * 6.0 * u) / 3.0;
        let quick = [
            (Button::Test, "Test"),
            (Button::Sleep, if v.sleeping { "Wake" } else { "Sleep" }),
            (Button::Hide, if v.visible { "Hide" } else { "Show" }),
        ];
        for (i, (b, label)) in quick.into_iter().enumerate() {
            let rect = Rect::from_xywh(x + i as f32 * (bw + 6.0 * u), y, bw, QUICK * u);
            self.button(pm, r, rect, label, b, true);
        }
        y + QUICK * u + GAP * u
    }

    /// A small grey title above a card.
    fn section(&mut self, pm: &mut Pixmap, r: &mut Renderer, y: f32, x: f32, title: &str) -> f32 {
        let u = self.ui;
        let title = t(title).to_uppercase();
        r.draw_text(
            pm,
            x + 4.0 * u,
            y + 5.0 * u,
            &title,
            sys(11.0 * u, true, MUTED),
        );
        y + SECTION * u
    }

    /// A card of `rows` rows at `y`; returns where its rows start.
    fn card(&mut self, pm: &mut Pixmap, y: f32, rows: usize) -> f32 {
        fill_card(pm, self.card_start(y, rows), self.ui);
        y
    }

    /// One line of grey help text.
    fn note(&mut self, pm: &mut Pixmap, r: &mut Renderer, y: f32, x: f32, text: &str) -> f32 {
        let u = self.ui;
        let st = sys(11.0 * u, false, MUTED);
        let shown = fit(r, t(text), st, self.cw - 8.0 * u);
        r.draw_text(pm, x + 4.0 * u, y + 4.0 * u, &shown, st);
        y + 22.0 * u
    }

    /// Size, text size, how long bubbles stay, the bubble's keyboard shortcuts and walking.
    fn tab_dog(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let s = &v.settings;
        let mut y = y;
        if self.window {
            y = self.quick_actions(pm, r, y, x, inner, v);
        }
        let mut cy = self.card(pm, y, 3);
        let size = format!("{}%", s.size);
        cy = self.stepper_row(pm, r, cy, x, inner, "Size", &size, Field::Size);
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
        let bubble = match s.notify_secs {
            0 => t("Until clicked").to_string(),
            n if n % 60 == 0 => format!("{} min", n / 60),
            n => format!("{n} s"),
        };
        cy = self.stepper_row(pm, r, cy, x, inner, "Bubble stays", &bubble, Field::Notify);
        // The bubble's keyboard shortcuts, so they can be found.
        let u = self.ui;
        let st = sys(11.0 * u, false, MUTED);
        let keys = format!("{}: {SHORTCUTS}", t("Bubble keys"));
        let keys = fit(r, &keys, st, inner - 8.0 * u);
        let (_, th) = r.measure(&keys, st);
        r.draw_text(
            pm,
            x + 4.0 * u,
            cy + GAP * u + (ROW * u - GAP * u - th) / 2.0,
            &keys,
            st,
        );
        cy += ROW * u;
        cy = self.section(pm, r, cy, x, "Walking");
        self.walking(pm, r, cy, x, inner, s)
    }

    /// Name and birthday, colours, what it wears, holiday outfits, the bubble theme and the
    /// colours of sources.
    fn tab_look(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let mut cy = self.card(pm, y, 7);
        let age = if v.age == "new here" || i18n::is_turkish() {
            t(&v.age).to_string()
        } else {
            format!("{} old", v.age)
        };
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Name",
            TextField::Name,
            s,
            Some(&age),
            "",
        );
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Birthday",
            TextField::Birthday,
            s,
            None,
            "YYYY-MM-DD",
        );
        cy = self.swatch_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Fur",
            &Fur::ALL.map(Fur::swatch),
            Fur::ALL.iter().position(|f| *f == s.fur),
            Hit::Fur,
        );
        cy = self.swatch_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Collar",
            &Collar::ALL.map(Collar::swatch),
            Collar::ALL.iter().position(|c| *c == s.collar),
            Hit::Collar,
        );
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Wears",
            Field::Wear,
            &Wear::ALL.map(|(_, l)| l),
            Wear::ALL.iter().position(|(w, _)| *w == s.wear),
        );
        let holiday = match v.outfit.hat {
            Some(Item::PartyHat) => "Holiday outfits · birthday today!",
            Some(Item::WitchHat) => "Holiday outfits · Halloween",
            Some(Item::SantaHat) => "Holiday outfits · Christmas",
            _ => "Holiday outfits",
        };
        cy = self.toggle_row(pm, r, cy, x, inner, holiday, s.seasonal, Toggle::Seasonal);
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Bubble",
            Field::Theme,
            &Theme::ALL.map(|(_, l)| l),
            Theme::ALL.iter().position(|(t, _)| *t == s.theme),
        );
        cy += GAP * u;
        cy = self.section(pm, r, cy, x, "Source colours");
        let entries: Vec<(String, String)> = s
            .source_colors
            .iter()
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        let start = self.card(pm, cy, entries.len() + 1);
        cy = start;
        for (i, (name, hex)) in entries.iter().enumerate() {
            let color = parse_hex(hex).unwrap_or(MUTED);
            cy = self.list_row(
                pm,
                r,
                cy,
                x,
                inner,
                name,
                Some((color, i)),
                List::Sources,
                i,
            );
        }
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Add",
            TextField::AddSource,
            s,
            None,
            "a source, e.g. CI",
        );
        self.note(pm, r, cy, x, "Click a colour to change it.")
    }

    /// The toy: a tile to drag it out of the panel (or click to throw it), whether it is out.
    fn tab_play(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let mut cy = self.section(pm, r, y, x, "Toys");
        let card_h = TILE + 16.0;
        if let Some(card) = Rect::from_xywh(x, cy, inner, card_h * u) {
            fill_card(pm, Some(card), u);
        }
        // The bone's tile.
        let item = v.outfit.toy.unwrap_or(Item::Bone);
        if let Some(tile) = Rect::from_xywh(x + 8.0 * u, cy + 8.0 * u, TILE * u, TILE * u) {
            let hover = self.hover == Some(Hit::ToyTile);
            fill_round(
                pm,
                Some(tile),
                10.0 * u,
                if hover { ACCENT_SOFT } else { HOVER },
            );
            stroke_round(
                pm,
                Some(tile),
                10.0 * u,
                if hover { ACCENT } else { BORDER },
                1.0,
            );
            let px = (((TILE - 16.0) * u) / r.toy_size(item, 1) as f32)
                .floor()
                .max(1.0) as u32;
            let size = r.toy_size(item, px);
            if let Some(mut icon) = Pixmap::new(size, size) {
                r.draw_toy(&mut icon, item, 0, px);
                let ix = (tile.left() + (tile.width() - size as f32) / 2.0).round() as i32;
                let iy = (tile.top() + (tile.height() - size as f32) / 2.0).round() as i32;
                pm.draw_pixmap(
                    ix,
                    iy,
                    icon.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
            self.hits.push((tile, Hit::ToyTile));
        }
        let tx = x + (TILE + 20.0) * u;
        let name = if item == Item::Pumpkin {
            "Pumpkin"
        } else {
            "Bone"
        };
        r.draw_text(pm, tx, cy + 12.0 * u, t(name), sys(13.0 * u, true, INK));
        let help = if s.toy {
            "Drag it out, or click to throw."
        } else {
            "Put away. Drag it out to play."
        };
        r.draw_text(pm, tx, cy + 32.0 * u, t(help), sys(11.0 * u, false, MUTED));
        r.draw_text(
            pm,
            tx,
            cy + 48.0 * u,
            t("Right-click it to put it away."),
            sys(11.0 * u, false, MUTED),
        );
        cy += card_h * u + GAP * u;
        let start = self.card(pm, cy, 1);
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            "Out to play with",
            s.toy,
            Toggle::Toy,
        );
        cy
    }

    /// Walking around, speed, what it stands on and the offset.
    fn walking(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        s: &Settings,
    ) -> f32 {
        let mut cy = self.card(pm, y, 4);
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
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Offset",
            TextField::Lift,
            s,
            Some("px"),
            "0",
        );
        self.note(pm, r, cy, x, "Offset raises the dog (−20 to 2000).")
    }

    fn tab_sound(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        s: &Settings,
    ) -> f32 {
        let u = self.ui;
        let mut cy = self.card(pm, y, 4);
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
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Happy, sad and asking voices",
            s.event_sounds,
            Toggle::EventSounds,
        );
        cy += GAP * u;
        cy = self.section(pm, r, cy, x, "Quiet");
        let start = self.card(pm, cy, 4);
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            "Quiet now",
            s.quiet,
            Toggle::QuietNow,
        );
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "From",
            TextField::QuietFrom,
            s,
            None,
            "22:00",
        );
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "To",
            TextField::QuietTo,
            s,
            None,
            "08:00",
        );
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Quiet in full screen",
            s.quiet_fullscreen,
            Toggle::QuietFullscreen,
        );
        cy = self.note(pm, r, cy, x, "Same From and To: no quiet hours.");
        cy += GAP * u;
        cy = self.section(pm, r, cy, x, "Bark sounds");
        let start = self.card(pm, cy, s.bark_files.len() + 1);
        cy = start;
        for (i, f) in s.bark_files.iter().enumerate() {
            let name = std::path::Path::new(f)
                .file_name()
                .map_or(f.clone(), |n| n.to_string_lossy().into_owned());
            cy = self.list_row(pm, r, cy, x, inner, &name, None, List::Barks, i);
        }
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Add",
            TextField::AddBark,
            s,
            None,
            "path to a .wav file",
        );
        self.note(pm, r, cy, x, "16-bit WAV files; one is picked at random.")
    }

    /// Todo lists: each a title (open count, Clear done, ✕) over a card of items with a
    /// check box, the reminder and a ✕, and a field to add one; a field for a new list below.
    fn tab_todo(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let mut cy = y;
        if v.todos.is_empty() {
            cy = self.note(
                pm,
                r,
                cy,
                x,
                "No lists yet. Make one below, or ask an agent:",
            );
            cy = self.note(pm, r, cy, x, "“make a todo list for the release”.");
            cy += GAP * u / 2.0;
        }
        for l in &v.todos {
            // Title: name and open count on the left, Clear done and ✕ on the right.
            let open = if i18n::is_turkish() { "açık" } else { "open" };
            let title = format!("{} · {} {open}", l.name.to_uppercase(), l.open_count());
            let st = sys(11.0 * u, true, MUTED);
            let shown = fit(r, &title, st, inner - 120.0 * u);
            r.draw_text(pm, x + 4.0 * u, cy + 5.0 * u, &shown, st);
            let mut bx = x + inner;
            let rm = Rect::from_xywh(bx - 22.0 * u, cy, 22.0 * u, 20.0 * u);
            if let Some(rect) = rm {
                let hit = Hit::TodoRemoveList(l.id);
                if self.hover == Some(hit) {
                    fill_round(pm, Some(rect), 6.0 * u, HOVER);
                }
                let c = if self.hover == Some(hit) {
                    DANGER
                } else {
                    MUTED
                };
                draw_glyph(
                    pm,
                    Glyph::Close,
                    rect.left() + rect.width() / 2.0,
                    rect.top() + rect.height() / 2.0,
                    u * 0.7,
                    c,
                );
                self.hits.push((rect, hit));
                bx = rect.left() - 4.0 * u;
            }
            if l.items.iter().any(|i| i.done) {
                let st = sys(11.0 * u, false, MUTED);
                let label = t("Clear done");
                let (tw, _) = r.measure(label, st);
                if let Some(rect) = Rect::from_xywh(bx - tw - 12.0 * u, cy, tw + 12.0 * u, 20.0 * u)
                {
                    let hit = Hit::TodoClear(l.id);
                    let color = if self.hover == Some(hit) {
                        fill_round(pm, Some(rect), 6.0 * u, HOVER);
                        INK
                    } else {
                        MUTED
                    };
                    r.draw_text(
                        pm,
                        rect.left() + 6.0 * u,
                        cy + 3.0 * u,
                        label,
                        sys(11.0 * u, false, color),
                    );
                    self.hits.push((rect, hit));
                }
            }
            cy += SECTION * u;
            let start = self.card(pm, cy, l.items.len() + 1);
            cy = start;
            for it in &l.items {
                cy = self.todo_row(pm, r, cy, x, inner, it);
            }
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Add",
                TextField::AddTodo(l.id),
                s,
                None,
                "a new item",
            );
            cy += GAP * u;
        }
        let start = self.card(pm, cy, 1);
        cy = self.text_row(
            pm,
            r,
            start,
            x,
            inner,
            "New list",
            TextField::NewList,
            s,
            None,
            "e.g. Release 0.3",
        );
        self.note(pm, r, cy, x, "Agents can add items and reminders over MCP.")
    }

    /// A todo item: check box, the text (struck through when done), the reminder, a ✕.
    fn todo_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        it: &crate::todo::Item,
    ) -> f32 {
        let u = self.ui;
        let check = Hit::TodoCheck(it.id);
        if let Some(row) = Rect::from_xywh(x, y, inner - 36.0 * u, ROW * u) {
            if self.hover == Some(check) {
                fill_round(pm, row.inset(2.0 * u), 10.0 * u, HOVER);
            }
            self.hits.push((row, check));
        }
        let b = 16.0 * u;
        let bx = Rect::from_xywh(x + 12.0 * u, y + (ROW * u - b) / 2.0, b, b);
        if it.done {
            fill_round(pm, bx, 5.0 * u, ACCENT);
            if let Some(b) = bx {
                draw_glyph(
                    pm,
                    Glyph::Check,
                    b.left() + b.width() / 2.0,
                    b.top() + b.height() / 2.0,
                    u * 0.75,
                    CARD,
                );
            }
        } else {
            fill_round(pm, bx, 5.0 * u, CARD);
            stroke_round(pm, bx, 5.0 * u, BORDER, 1.5 * u);
        }
        let tx = x + 12.0 * u + b + 10.0 * u;
        let mut right = x + inner - 40.0 * u;
        if let Some(at) = it.remind_at.filter(|_| !it.done) {
            let st = sys(11.0 * u, false, ACCENT_INK);
            let when = crate::todo::local_time(at);
            let (tw, _) = r.measure(&when, st);
            r.draw_text(pm, right - tw, y + 9.0 * u, &when, st);
            right -= tw + 8.0 * u;
        }
        let st = sys(12.5 * u, false, if it.done { MUTED } else { INK });
        let shown = fit(r, &it.text, st, right - tx);
        let (tw, th) = r.measure(&shown, st);
        let ty = y + 8.0 * u;
        r.draw_text(pm, tx, ty, &shown, st);
        if it.done {
            fill_round(
                pm,
                Rect::from_xywh(tx, (ty + th / 2.0).round(), tw, (1.2 * u).max(1.0)),
                0.0,
                MUTED,
            );
        }
        let remove = Hit::TodoRemove(it.id);
        if let Some(rect) = Rect::from_xywh(x + inner - 32.0 * u, y + 4.0 * u, 24.0 * u, 24.0 * u) {
            if self.hover == Some(remove) {
                fill_round(pm, Some(rect), 7.0 * u, HOVER);
            }
            let c = if self.hover == Some(remove) {
                DANGER
            } else {
                MUTED
            };
            draw_glyph(
                pm,
                Glyph::Close,
                rect.left() + rect.width() / 2.0,
                rect.top() + rect.height() / 2.0,
                u * 0.8,
                c,
            );
            self.hits.push((rect, remove));
        }
        y + ROW * u
    }

    /// Language, the special days, which screen the dog lives on, and the app.
    fn page_general(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let mut cy = self.card(pm, y, 3);
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Language",
            Field::Language,
            &Lang::ALL.map(|(_, l)| l),
            Lang::ALL.iter().position(|(l, _)| *l == s.language),
        );
        let screens: Vec<String> = std::iter::once(t("Any").to_string())
            .chain((1..=v.screens.clamp(1, 4)).map(|i| {
                if i == 1 {
                    t("Main").to_string()
                } else {
                    i.to_string()
                }
            }))
            .collect();
        let screens: Vec<&str> = screens.iter().map(String::as_str).collect();
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Screen",
            Field::Screen,
            &screens,
            Some((s.screen as usize).min(screens.len() - 1)),
        );
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
        cy = self.note(
            pm,
            r,
            cy,
            x,
            if i18n::is_turkish() {
                "Ekran: köpeğin yaşadığı ekran; Herhangi: nereye bırakırsan."
            } else {
                "Screen: where the dog lives; Any: wherever you put it."
            },
        );
        cy += GAP * u;
        cy = self.section(pm, r, cy, x, "Special days");
        let start = self.card(pm, cy, 2);
        let label = match (v.silence, v.special) {
            (true, _) => {
                if i18n::is_turkish() {
                    "Özel günler · saygı duruşu"
                } else {
                    "Special days · minute of silence"
                }
            }
            (_, Some(crate::special::Day::Remembrance)) => {
                if i18n::is_turkish() {
                    "Özel günler · bugün 10 Kasım"
                } else {
                    "Special days · 10 November today"
                }
            }
            (_, Some(crate::special::Day::National)) => {
                if i18n::is_turkish() {
                    "Özel günler · bayram"
                } else {
                    "Special days · a holiday today"
                }
            }
            _ => "Special days",
        };
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            label,
            s.special_days,
            Toggle::SpecialDays,
        );
        let siren = if i18n::is_turkish() {
            "09:05'te siren"
        } else {
            "Siren at 09:05"
        };
        cy = self.toggle_row(pm, r, cy, x, inner, siren, s.siren, Toggle::Siren);
        let lines: &[&str] = if i18n::is_turkish() {
            &[
                "10 Kasım: gün boyu siyah kurdele; 09:05'te köpek durur,",
                "bir dakika sessiz kalır, sonra Atatürk'ü anan bir balon.",
                "Bayramlarda (23 Nisan, 19 Mayıs, 30 Ağustos, 29 Ekim)",
                "tasmada küçük bir bayrak.",
            ]
        } else {
            &[
                "10 November: a black ribbon all day; at 09:05 the dog",
                "stands still for a minute of silence, then remembers Atatürk.",
                "National holidays (23 April, 19 May, 30 August,",
                "29 October): a little flag on the collar.",
            ]
        };
        for l in lines {
            cy = self.note(pm, r, cy, x, l) - 6.0 * u;
        }
        cy += 6.0 * u + GAP * u;
        if self.window {
            cy = self.section(pm, r, cy, x, "App");
            let start = self.card(pm, cy, 2);
            cy = self.toggle_row(
                pm,
                r,
                start,
                x,
                inner,
                "Check for updates",
                s.check_updates,
                Toggle::CheckUpdates,
            );
            cy = self.toggle_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Install updates automatically",
                s.auto_update,
                Toggle::AutoUpdate,
            );
        }
        cy
    }

    /// Notifications on the phone: when, ntfy and Telegram.
    fn page_phone(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let tr = i18n::is_turkish();
        let mut cy = self.card(pm, y, 1);
        cy = self.choice_row(
            pm,
            r,
            cy,
            x,
            inner,
            "When",
            Field::PhoneWhen,
            &PHONE_WHEN.map(|(_, l)| l),
            PHONE_WHEN.iter().position(|(w, _)| *w == s.phone.when),
        );
        cy = self.note(
            pm,
            r,
            cy,
            x,
            if tr {
                "Uzaktayken: 5 dk boşta ya da ekran kilitliyken."
            } else {
                "Away: idle for 5 minutes, or the screen is locked."
            },
        );
        cy += GAP * u / 2.0;

        cy = self.section(pm, r, cy, x, "ntfy");
        let n = &s.phone.ntfy;
        let rows = if n.enabled { 5 } else { 1 };
        let start = self.card(pm, cy, rows);
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            if tr {
                "ntfy ile gönder"
            } else {
                "Send with ntfy"
            },
            n.enabled,
            Toggle::Ntfy,
        );
        if n.enabled {
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Server",
                TextField::NtfyServer,
                s,
                None,
                "https://ntfy.sh",
            );
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Topic",
                TextField::NtfyTopic,
                s,
                None,
                "dost-…",
            );
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Token",
                TextField::NtfyToken,
                s,
                None,
                if tr { "isteğe bağlı" } else { "optional" },
            );
            let bw = (inner - 3.0 * 8.0 * u) / 3.0;
            for (i, (b, label)) in [
                (
                    Button::CopyTopic,
                    if tr {
                        "Bağlantıyı kopyala"
                    } else {
                        "Copy link"
                    },
                ),
                (Button::NewTopic, if tr { "Yeni konu" } else { "New topic" }),
                (Button::PhoneTest, "Send a test"),
            ]
            .into_iter()
            .enumerate()
            {
                let rect = Rect::from_xywh(
                    x + 8.0 * u + i as f32 * (bw + 4.0 * u),
                    cy + 4.0 * u,
                    bw,
                    24.0 * u,
                );
                self.button(pm, r, rect, label, b, true);
            }
            cy += ROW * u;
            cy = self.note(
                pm,
                r,
                cy,
                x,
                if tr {
                    "ntfy uygulamasında bu konuya abone ol; sorular buton olarak gelir."
                } else {
                    "Subscribe to the topic in the ntfy app; questions come with buttons."
                },
            );
        }
        cy += GAP * u / 2.0;

        cy = self.section(pm, r, cy, x, "Telegram");
        let tg = &s.phone.telegram;
        let rows = if tg.enabled { 3 } else { 1 };
        let start = self.card(pm, cy, rows);
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            if tr {
                "Telegram botu ile gönder"
            } else {
                "Send with a Telegram bot"
            },
            tg.enabled,
            Toggle::Telegram,
        );
        if tg.enabled {
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Token",
                TextField::TelegramToken,
                s,
                None,
                "123456:ABC…",
            );
            let (state, c) = if tg.token.is_empty() {
                (t("A bot token from @BotFather.").to_string(), MUTED)
            } else if tg.chat_id.is_empty() {
                (
                    if tr {
                        "Eşleştirmek için bota /start yaz.".to_string()
                    } else {
                        "Send /start to the bot to pair.".to_string()
                    },
                    ACCENT_INK,
                )
            } else if tr {
                (format!("Eşleşti · sohbet {}", tg.chat_id), AWAKE)
            } else {
                (format!("Paired · chat {}", tg.chat_id), AWAKE)
            };
            let st = sys(11.5 * u, false, c);
            let shown = fit(r, &state, st, inner - 140.0 * u);
            r.draw_text(pm, x + 12.0 * u, cy + 8.0 * u, &shown, st);
            let test = Rect::from_xywh(x + inner - 118.0 * u, cy + 4.0 * u, 110.0 * u, 24.0 * u);
            self.button(pm, r, test, "Send a test", Button::PhoneTest, true);
            cy += ROW * u;
            cy = self.note(
                pm,
                r,
                cy,
                x,
                if tr {
                    "Sorulara butonla ya da mesaja yanıt yazarak cevap ver."
                } else {
                    "Answer questions with the buttons, or reply to the message."
                },
            );
        }
        if let Some(e) = &v.phone_status {
            let st = sys(11.5 * u, false, DANGER);
            let shown = fit(r, e, st, inner - 8.0 * u);
            r.draw_text(pm, x + 4.0 * u, cy + 6.0 * u, &shown, st);
            cy += 24.0 * u;
        }
        cy
    }

    /// What the agents are doing: one row per session, waiting ones first.
    fn page_agents(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let tr = i18n::is_turkish();
        if v.sessions.is_empty() {
            let mut cy = self.note(
                pm,
                r,
                y,
                x,
                "No agents yet. They show up here while they work.",
            );
            cy = self.note(
                pm,
                r,
                cy,
                x,
                if tr {
                    "OpenCode eklentisi, Claude Code ve Codex kancaları"
                } else {
                    "The OpenCode plugin, the Claude Code and Codex hooks"
                },
            );
            return self.note(
                pm,
                r,
                cy,
                x,
                if tr {
                    "ve MCP `session` aracı oturumları bildirir."
                } else {
                    "and the MCP `session` tool report sessions."
                },
            );
        }
        let rows = v.sessions.len();
        let h = rows as f32 * SESSION_ROW * u;
        if let Some(card) = Rect::from_xywh(self.cx, y, self.cw, h) {
            fill_card(pm, Some(card), u);
        }
        let now = crate::watch::now_unix();
        for (i, sess) in v.sessions.iter().enumerate() {
            let ry = y + i as f32 * SESSION_ROW * u;
            if i > 0
                && let Some(rule) = Rect::from_xywh(x + 12.0 * u, ry, inner - 24.0 * u, 1.0)
            {
                fill_round(pm, Some(rule), 0.0, TRACK);
            }
            let hit = Hit::Session(i);
            if let Some(row) = Rect::from_xywh(x, ry, inner - 36.0 * u, SESSION_ROW * u) {
                if sess.open.is_some() && self.hover == Some(hit) {
                    fill_round(pm, row.inset(2.0 * u), 10.0 * u, HOVER);
                }
                self.hits.push((row, hit));
            }
            let c = match sess.state {
                SessionState::Working => ACCENT,
                SessionState::Waiting => DANGER,
                SessionState::Done => AWAKE,
                SessionState::Failed => DANGER,
                SessionState::Idle => MUTED,
            };
            let d = 8.0 * u;
            fill_round(
                pm,
                Rect::from_xywh(x + 14.0 * u, ry + 11.0 * u, d, d),
                d / 2.0,
                c,
            );
            if sess.state == SessionState::Failed {
                fill_round(
                    pm,
                    Rect::from_xywh(x + 16.0 * u, ry + 13.0 * u, d - 4.0 * u, d - 4.0 * u),
                    d / 2.0,
                    CARD,
                );
            }
            let tx = x + 30.0 * u;
            let label = match (&sess.title, &sess.project) {
                (t, p) if t.is_empty() && p.is_empty() => sess.source.clone(),
                (t, p) if p.is_empty() => format!("{} · {t}", sess.source),
                (t, p) if t.is_empty() => format!("{} · {p}", sess.source),
                (t, p) => format!("{t} · {p}"),
            };
            let secs = now.saturating_sub(sess.since);
            let state = format!(
                "{} · {}",
                t(sess.state.label()),
                crate::watch::human_secs(secs)
            );
            let st_state = sys(11.0 * u, sess.state == SessionState::Waiting, c);
            let (sw, _) = r.measure(&state, st_state);
            let right = x + inner - 40.0 * u;
            r.draw_text(pm, right - sw, ry + 6.0 * u, &state, st_state);
            let st = sys(12.5 * u, true, INK);
            let shown = fit(r, &label, st, right - sw - tx - 10.0 * u);
            r.draw_text(pm, tx, ry + 5.0 * u, &shown, st);
            let detail = if sess.detail.is_empty() {
                sess.source.clone()
            } else {
                sess.detail.clone()
            };
            let st = sys(11.0 * u, false, MUTED);
            let shown = fit(r, &detail, st, right - tx);
            r.draw_text(pm, tx, ry + 25.0 * u, &shown, st);
            let remove = Hit::RemoveSession(i);
            if let Some(rect) =
                Rect::from_xywh(x + inner - 32.0 * u, ry + 11.0 * u, 24.0 * u, 24.0 * u)
            {
                if self.hover == Some(remove) {
                    fill_round(pm, Some(rect), 7.0 * u, HOVER);
                }
                draw_glyph(
                    pm,
                    Glyph::Close,
                    rect.left() + rect.width() / 2.0,
                    rect.top() + rect.height() / 2.0,
                    u * 0.8,
                    if self.hover == Some(remove) {
                        DANGER
                    } else {
                        MUTED
                    },
                );
                self.hits.push((rect, remove));
            }
        }
        let mut cy = y + h + GAP * u;
        if v.sessions
            .iter()
            .any(|s| !matches!(s.state, SessionState::Working | SessionState::Waiting))
        {
            let rect = Rect::from_xywh(x, cy, 120.0 * u, 26.0 * u);
            self.button(pm, r, rect, "Clear done", Button::ClearSessions, true);
            cy += 34.0 * u;
        }
        cy
    }

    /// Answering a question in words: the question, a field and Send / Cancel.
    fn page_reply(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let Some((_, title, question, _)) = self.reply.clone() else {
            return y;
        };
        let mut cy = y;
        if !self.window && !title.is_empty() {
            let st = sys(13.0 * u, true, INK);
            let shown = fit(r, &title, st, inner - 8.0 * u);
            r.draw_text(pm, x + 4.0 * u, cy + 2.0 * u, &shown, st);
            cy += 22.0 * u;
        }
        // The question, wrapped by words into lines that fit.
        let st = sys(12.5 * u, false, INK);
        let mut lines: Vec<String> = Vec::new();
        for para in question.lines() {
            let mut line = String::new();
            for word in para.split_whitespace() {
                let next = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if r.measure(&next, st).0 > inner - 8.0 * u && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                } else {
                    line = next;
                }
            }
            lines.push(line);
        }
        for l in lines.iter().take(8) {
            r.draw_text(pm, x + 4.0 * u, cy, l, st);
            cy += 18.0 * u;
        }
        cy += GAP * u;
        let start = self.card(pm, cy, 1);
        cy = self.text_row(
            pm,
            r,
            start,
            x,
            inner,
            "Answer",
            TextField::Reply,
            &v.settings,
            None,
            if i18n::is_turkish() {
                "kendi sözlerinle…"
            } else {
                "in your own words…"
            },
        );
        cy += GAP * u;
        let bw = 96.0 * u;
        let send = Rect::from_xywh(x + inner - bw, cy, bw, 28.0 * u);
        self.button(pm, r, send, "Send", Button::ReplySend, false);
        let cancel = Rect::from_xywh(x + inner - 2.0 * bw - 8.0 * u, cy, bw, 28.0 * u);
        self.button(pm, r, cancel, "Cancel", Button::ReplyCancel, true);
        cy + 36.0 * u
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
    ) -> f32 {
        let u = self.ui;
        let rows = if self.window {
            v.history.len().clamp(5, 50)
        } else {
            5
        };
        let card = self.card_start(y, rows);
        fill_card(pm, card, u);
        if v.history.is_empty() {
            let st = sys(12.5 * u, false, MUTED);
            let msg = t("Nothing yet. Notifications show up here.");
            let (tw, th) = r.measure(msg, st);
            r.draw_text(
                pm,
                (x + (inner - tw) / 2.0).round(),
                (y + (rows as f32 * ROW * u - th) / 2.0).round(),
                msg,
                st,
            );
            return y + rows as f32 * ROW * u;
        }
        for (i, h) in v.history.iter().take(rows).enumerate() {
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
        y + rows as f32 * ROW * u
    }

    /// The MCP server (address, which web pages may use it) and the app (login, updates).
    fn tab_connect(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        v: &View,
    ) -> f32 {
        let u = self.ui;
        let s = &v.settings;
        let start = self.card(pm, y, 3);
        let mut cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            "MCP server",
            s.mcp_enabled,
            Toggle::Mcp,
        );
        cy = self.text_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Address",
            TextField::McpAddr,
            s,
            None,
            "127.0.0.1:7777",
        );
        let status_color = if v.mcp_ok { MUTED } else { DANGER };
        let st = sys(11.5 * u, false, status_color);
        let shown = fit(r, &v.mcp_status, st, inner - 90.0 * u);
        r.draw_text(pm, x + 12.0 * u, cy + 8.0 * u, &shown, st);
        let copy = Rect::from_xywh(x + inner - 62.0 * u, cy + 4.0 * u, 54.0 * u, 24.0 * u);
        self.button(pm, r, copy, "Copy", Button::CopyMcp, true);
        cy += ROW * u;
        cy += GAP * u;

        // Web pages that may call the server from a browser (CORS).
        cy = self.section(pm, r, cy, x, "Browsers");
        let cors = &s.mcp_cors;
        let any = cors.allow_origins.iter().any(|o| o == "*");
        let custom = !any && (self.custom_origins || !cors.allow_origins.is_empty());
        let selected = if any {
            1
        } else if custom {
            2
        } else {
            0
        };
        let list: Vec<String> = if custom {
            cors.allow_origins.clone()
        } else {
            Vec::new()
        };
        let rows = 2 + if custom { list.len() + 1 } else { 0 };
        let start = self.card(pm, cy, rows);
        cy = self.choice_row(
            pm,
            r,
            start,
            x,
            inner,
            "Allow",
            Field::Browsers,
            &["Local", "Any", "Custom"],
            Some(selected),
        );
        if custom {
            for (i, o) in list.iter().enumerate() {
                cy = self.list_row(pm, r, cy, x, inner, o, None, List::Origins, i);
            }
            cy = self.text_row(
                pm,
                r,
                cy,
                x,
                inner,
                "Add",
                TextField::AddOrigin,
                s,
                None,
                "https://*.example.com",
            );
        }
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Private network access",
            cors.allow_private_network,
            Toggle::PrivateNetwork,
        );
        cy = self.note(pm, r, cy, x, "Pages on localhost can always connect.");
        cy += GAP * u;

        cy = self.section(pm, r, cy, x, "App");
        let start = self.card(pm, cy, 3);
        cy = self.toggle_row(
            pm,
            r,
            start,
            x,
            inner,
            v.autostart_label,
            v.autostart,
            Toggle::Autostart,
        );
        cy = self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Check for updates",
            s.check_updates,
            Toggle::CheckUpdates,
        );
        self.toggle_row(
            pm,
            r,
            cy,
            x,
            inner,
            "Install updates automatically",
            s.auto_update,
            Toggle::AutoUpdate,
        )
    }

    /// A row with a text field: shows the value, click to edit (Enter saves, Esc cancels).
    /// `suffix`: grey text right of the field; `hint`: shown while the field is empty.
    #[allow(clippy::too_many_arguments)]
    fn text_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        label: &str,
        f: TextField,
        s: &Settings,
        suffix: Option<&str>,
        hint: &str,
    ) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            t(label),
            sys(13.0 * u, false, INK),
        );
        let st_suffix = sys(11.0 * u, false, MUTED);
        let sw = suffix.map_or(0.0, |t| r.measure(t, st_suffix).0 + 10.0 * u);
        if let Some(t) = suffix {
            r.draw_text(
                pm,
                x + inner - 10.0 * u - (sw - 10.0 * u),
                y + 9.0 * u,
                t,
                st_suffix,
            );
        }
        let fx = x + self.label_w();
        let fw = inner - self.label_w() - 6.0 * u - sw;
        let Some(field) = Rect::from_xywh(fx, y + 4.0 * u, fw, 24.0 * u) else {
            return y + ROW * u;
        };
        let editing = self.editing.as_ref().filter(|e| e.field == f);
        let hit = Hit::Text(f);
        let (bg, border) = match (editing, self.hover == Some(hit)) {
            (Some(_), _) => (CARD, ACCENT),
            (None, true) => (HOVER, BORDER),
            (None, false) => (CARD, BORDER),
        };
        fill_round(pm, Some(field), 7.0 * u, bg);
        stroke_round(
            pm,
            Some(field),
            7.0 * u,
            border,
            if editing.is_some() { 1.5 * u } else { 1.0 },
        );
        let mut value = Self::field_value(f, s);
        if f.secret() && editing.is_none() && !value.is_empty() {
            value = "•".repeat(value.chars().count().min(16));
        }
        let hint = t(hint);
        let text = editing.map_or(value.as_str(), |e| e.text.as_str());
        let max = fw - 16.0 * u;
        let (shown, st) = if text.is_empty() && editing.is_none() {
            (
                fit(r, hint, sys(12.0 * u, false, MUTED), max),
                sys(12.0 * u, false, MUTED),
            )
        } else {
            let st = sys(12.5 * u, false, INK);
            // While editing, show the end of the text (where the caret is).
            let shown = if editing.is_some() {
                fit_end(r, text, st, max)
            } else {
                fit(r, text, st, max)
            };
            (shown, st)
        };
        let (tw, th) = r.measure(&shown, st);
        let ty = (field.top() + (field.height() - th) / 2.0).round();
        r.draw_text(pm, fx + 8.0 * u, ty, &shown, st);
        if let Some(e) = editing
            && (e.since.elapsed().as_millis() / 530).is_multiple_of(2)
        {
            let tw = if text.is_empty() { 0.0 } else { tw };
            let cx = (fx + 8.0 * u + tw + 1.0 * u).round();
            fill_round(
                pm,
                Rect::from_xywh(cx, field.top() + 5.0 * u, (1.2 * u).max(1.0), 14.0 * u),
                0.0,
                INK,
            );
        }
        self.hits.push((field, hit));
        y + ROW * u
    }

    /// An entry of a list: optional colour dot (click: next colour), the text, a ✕ to remove.
    #[allow(clippy::too_many_arguments)]
    fn list_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        text: &str,
        dot: Option<(ColorU8, usize)>,
        list: List,
        i: usize,
    ) -> f32 {
        let u = self.ui;
        let mut tx = x + 12.0 * u;
        if let Some((c, idx)) = dot {
            let d = 14.0 * u;
            if let Some(cell) = Rect::from_xywh(tx - 3.0 * u, y + 6.0 * u, d + 6.0 * u, d + 6.0 * u)
            {
                if self.hover == Some(Hit::SourceColor(idx)) {
                    fill_round(pm, Some(cell), (d + 6.0 * u) / 2.0, HOVER);
                }
                self.hits.push((cell, Hit::SourceColor(idx)));
            }
            let dot = Rect::from_xywh(tx, y + 9.0 * u, d, d);
            fill_round(pm, dot, d / 2.0, c);
            tx += d + 10.0 * u;
        }
        let st = sys(12.5 * u, false, INK);
        let shown = fit(r, text, st, x + inner - 40.0 * u - tx);
        r.draw_text(pm, tx, y + 8.0 * u, &shown, st);
        let remove = Rect::from_xywh(x + inner - 32.0 * u, y + 4.0 * u, 24.0 * u, 24.0 * u);
        let hit = Hit::Remove(list, i);
        if let Some(rect) = remove {
            if self.hover == Some(hit) {
                fill_round(pm, Some(rect), 7.0 * u, HOVER);
            }
            draw_glyph(
                pm,
                Glyph::Close,
                rect.left() + rect.width() / 2.0,
                rect.top() + rect.height() / 2.0,
                u * 0.8,
                if self.hover == Some(hit) {
                    DANGER
                } else {
                    MUTED
                },
            );
            self.hits.push((rect, hit));
        }
        y + ROW * u
    }

    /// A value with − and + on either side.
    #[allow(clippy::too_many_arguments)]
    fn stepper_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        inner: f32,
        label: &str,
        value: &str,
        field: Field,
    ) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            t(label),
            sys(13.0 * u, false, INK),
        );
        let seg_x = x + self.label_w();
        let seg_w = inner - self.label_w() - 6.0 * u;
        let seg = Rect::from_xywh(seg_x, y + 5.0 * u, seg_w, 22.0 * u);
        fill_round(pm, seg, 8.0 * u, TRACK);
        let bw = 30.0 * u;
        for (i, sign) in [(0, "−"), (1, "+")] {
            let bx = if i == 0 { seg_x } else { seg_x + seg_w - bw };
            let Some(cell) = Rect::from_xywh(bx, y + 5.0 * u, bw, 22.0 * u) else {
                continue;
            };
            let hit = Hit::Choice(field, i);
            if self.hover == Some(hit) {
                fill_round(pm, cell.inset(2.0 * u), 6.0 * u, HOVER);
            }
            let st = sys(14.0 * u, true, INK);
            let (tw, th) = r.measure(sign, st);
            r.draw_text(
                pm,
                (cell.left() + (cell.width() - tw) / 2.0).round(),
                (cell.top() + (cell.height() - th) / 2.0).round(),
                sign,
                st,
            );
            self.hits.push((cell, hit));
        }
        if let Some(mid) = Rect::from_xywh(seg_x + bw, y + 5.0 * u, seg_w - 2.0 * bw, 22.0 * u) {
            fill_round(pm, mid.inset(2.0 * u), 6.0 * u, CARD);
            let st = sys(11.5 * u, true, INK);
            let (tw, th) = r.measure(value, st);
            r.draw_text(
                pm,
                (mid.left() + (mid.width() - tw) / 2.0).round(),
                (mid.top() + (mid.height() - th) / 2.0).round(),
                value,
                st,
            );
        }
        y + ROW * u
    }

    /// A row of colour dots; the chosen one has a ring.
    #[allow(clippy::too_many_arguments)]
    fn swatch_row(
        &mut self,
        pm: &mut Pixmap,
        r: &mut Renderer,
        y: f32,
        x: f32,
        _inner: f32,
        label: &str,
        colors: &[ColorU8],
        selected: Option<usize>,
        hit: fn(usize) -> Hit,
    ) -> f32 {
        let u = self.ui;
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            t(label),
            sys(13.0 * u, false, INK),
        );
        let d = 18.0 * u;
        let gap = if colors.len() > 6 { 7.0 * u } else { 10.0 * u };
        for (i, c) in colors.iter().enumerate() {
            let cx = x + self.label_w() + i as f32 * (d + gap);
            let Some(cell) = Rect::from_xywh(cx - 3.0 * u, y + 4.0 * u, d + 6.0 * u, d + 6.0 * u)
            else {
                continue;
            };
            let h = hit(i);
            if selected == Some(i) {
                stroke_round(pm, Some(cell), (d + 6.0 * u) / 2.0, ACCENT, 2.0 * u);
            } else if self.hover == Some(h) {
                fill_round(pm, Some(cell), (d + 6.0 * u) / 2.0, HOVER);
            }
            let dot = Rect::from_xywh(cx, y + 7.0 * u, d, d);
            fill_round(pm, dot, d / 2.0, *c);
            stroke_round(pm, dot, d / 2.0, BORDER, 1.0);
            // White means "none": crossed out.
            if *c == rgb(0xff, 0xff, 0xff)
                && let Some(dot) = dot
            {
                let mut pb = PathBuilder::new();
                let k = d * 0.2;
                pb.move_to(dot.left() + k, dot.bottom() - k);
                pb.line_to(dot.right() - k, dot.top() + k);
                if let Some(path) = pb.finish() {
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(MUTED.red(), MUTED.green(), MUTED.blue(), 255);
                    paint.anti_alias = true;
                    let stroke = Stroke {
                        width: 1.5 * u,
                        line_cap: LineCap::Round,
                        ..Stroke::default()
                    };
                    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                }
            }
            self.hits.push((cell, h));
        }
        y + ROW * u
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
            let label = t(label);
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
        self.version_link(pm, r, x + inner, y, v);
        y + FOOTER * u + PAD * u
    }

    /// The version, right-aligned at `right`; a newer one shows up here, and clicking checks
    /// for updates (or restarts into one).
    fn version_link(&mut self, pm: &mut Pixmap, r: &mut Renderer, right: f32, y: f32, v: &View) {
        let u = self.ui;
        let (x, inner) = (right, 0.0);
        let hover = self.hover == Some(Hit::Button(Button::Version));
        let tr = i18n::is_turkish();
        let (version, color) = match &v.update {
            Some((nv, true)) if tr => (format!("v{nv} hazır · Yeniden başlat"), ACCENT_INK),
            Some((nv, false)) if tr => (format!("v{nv} çıktı"), ACCENT_INK),
            Some((nv, true)) => (format!("v{nv} ready · Restart"), ACCENT_INK),
            Some((nv, false)) => (format!("v{nv} is out"), ACCENT_INK),
            None if hover => (t("Check for updates").to_string(), INK),
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
        if let Some((px, py)) = self.toy_press
            && ((x - px).powi(2) + (y - py).powi(2)).sqrt() > 8.0 * self.ui
        {
            self.toy_press = None;
            return (false, Some(Action::TakeToy));
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
        let hit = self.hit(x, y);
        self.toy_press = (hit == Some(Hit::ToyTile)).then_some((x, y));
        if hit == Some(Hit::Slider) {
            self.dragging = true;
            return self.slider_value(x).map(|volume| Action::Volume {
                volume,
                done: false,
            });
        }
        None
    }

    pub fn pointer_up(&mut self, x: f32, y: f32, v: &View) -> Option<Action> {
        let toy_click = self.toy_press.take().is_some();
        if std::mem::take(&mut self.dragging) {
            return self
                .slider_value(x)
                .map(|volume| Action::Volume { volume, done: true });
        }
        let hit = self.hit(x, y);
        if let Some(Hit::Text(f)) = hit {
            if self.editing.as_ref().is_some_and(|e| e.field == f) {
                return None;
            }
            // Clicking another field saves the one being edited first.
            let saved = self.finish_editing(v);
            let s = match &saved {
                Some(Action::Settings(s)) => s.as_ref(),
                _ => &v.settings,
            };
            self.editing = Some(Editing {
                field: f,
                text: Self::field_value(f, s),
                since: std::time::Instant::now(),
            });
            return saved;
        }
        // Cancel on the reply page drops what was typed.
        if hit == Some(Hit::Button(Button::ReplyCancel)) {
            self.end_reply();
            return None;
        }
        // Clicking anywhere else saves the field being edited; a reply is only sent by Send
        // (or Enter).
        let replying = self
            .editing
            .as_ref()
            .is_some_and(|e| e.field == TextField::Reply);
        if (!replying || hit == Some(Hit::Button(Button::ReplySend)))
            && let Some(a) = self.finish_editing(v)
        {
            return Some(a);
        }
        let mut s = v.settings.clone();
        match hit? {
            Hit::Tab(page) => {
                if self.page != page {
                    self.set_page(page);
                    if page != Page::History {
                        self.missed = 0;
                    }
                }
                return None;
            }
            Hit::Session(i) => {
                return v
                    .sessions
                    .get(i)
                    .and_then(|s| s.open.clone())
                    .map(Action::OpenUrl);
            }
            Hit::RemoveSession(i) => {
                return v
                    .sessions
                    .get(i)
                    .map(|s| Action::RemoveSession(s.id.clone()));
            }
            Hit::Text(_) => return None,
            Hit::ToyTile => return toy_click.then_some(Action::Fetch),
            Hit::Fur(i) => s.fur = Fur::ALL[i],
            Hit::Collar(i) => s.collar = Collar::ALL[i],
            Hit::Remove(list, i) => match list {
                List::Origins => {
                    if i < s.mcp_cors.allow_origins.len() {
                        s.mcp_cors.allow_origins.remove(i);
                    }
                }
                List::Barks => {
                    if i < s.bark_files.len() {
                        s.bark_files.remove(i);
                    }
                }
                List::Sources => {
                    if let Some(k) = s.source_colors.keys().nth(i).cloned() {
                        s.source_colors.remove(&k);
                    }
                }
            },
            Hit::TodoCheck(id) => return Some(Action::Todo(crate::todo::Edit::Toggle(id))),
            Hit::TodoRemove(id) => return Some(Action::Todo(crate::todo::Edit::Remove(id))),
            Hit::TodoClear(id) => return Some(Action::Todo(crate::todo::Edit::ClearDone(id))),
            Hit::TodoRemoveList(id) => {
                return Some(Action::Todo(crate::todo::Edit::RemoveList(id)));
            }
            Hit::SourceColor(i) => {
                if let Some(c) = s.source_colors.values_mut().nth(i) {
                    let now = parse_hex(c);
                    let at = SOURCE_PALETTE.iter().position(|p| Some(*p) == now);
                    let next = at.map_or(0, |a| (a + 1) % SOURCE_PALETTE.len());
                    *c = crate::look::to_hex(SOURCE_PALETTE[next]);
                }
            }
            Hit::Choice(field, i) => match field {
                Field::Size => {
                    let step = crate::settings::SIZE_STEP as i32;
                    let d = if i == 0 { -step } else { step };
                    let snapped = (s.size as i32 / step) * step;
                    s.size = (snapped + d).clamp(50, 400) as u32;
                }
                Field::Notify => {
                    let at = NOTIFY_STEPS
                        .iter()
                        .position(|n| *n == s.notify_secs)
                        .unwrap_or(1);
                    let next = if i == 0 {
                        at.saturating_sub(1)
                    } else {
                        (at + 1).min(NOTIFY_STEPS.len() - 1)
                    };
                    s.notify_secs = NOTIFY_STEPS[next];
                }
                Field::TextSize => s.text_size = TEXT_SIZES[i].0,
                Field::Speed => s.speed = SPEEDS[i].0,
                Field::Ground => {
                    s.ground = if i == 0 {
                        Ground::Taskbar
                    } else {
                        Ground::Screen
                    }
                }
                Field::Wear => s.wear = Wear::ALL[i].0,
                Field::Language => s.language = Lang::ALL[i].0,
                Field::PhoneWhen => s.phone.when = PHONE_WHEN[i].0,
                Field::Screen => s.screen = i as u32,
                Field::Theme => s.theme = Theme::ALL[i].0,
                Field::Browsers => {
                    self.custom_origins = i == 2;
                    let keep = s
                        .mcp_cors
                        .allow_origins
                        .iter()
                        .filter(|o| *o != "*")
                        .cloned()
                        .collect();
                    s.mcp_cors.allow_origins = match i {
                        0 => Vec::new(),
                        1 => vec!["*".into()],
                        _ => keep,
                    };
                    if i == 2 {
                        return (s != v.settings).then(|| Action::Settings(Box::new(s)));
                    }
                }
            },
            Hit::Toggle(t) => match t {
                Toggle::Sound => s.sound = !s.sound,
                Toggle::SoundOnPet => s.sound_on_pet = !s.sound_on_pet,
                Toggle::QuietNow => s.quiet = !s.quiet,
                Toggle::QuietFullscreen => s.quiet_fullscreen = !s.quiet_fullscreen,
                Toggle::Wander => s.wander = !s.wander,
                Toggle::Mcp => s.mcp_enabled = !s.mcp_enabled,
                Toggle::PrivateNetwork => {
                    s.mcp_cors.allow_private_network = !s.mcp_cors.allow_private_network
                }
                Toggle::CheckUpdates => s.check_updates = !s.check_updates,
                Toggle::AutoUpdate => s.auto_update = !s.auto_update,
                Toggle::Seasonal => s.seasonal = !s.seasonal,
                Toggle::EventSounds => s.event_sounds = !s.event_sounds,
                Toggle::Toy => s.toy = !s.toy,
                Toggle::SpecialDays => s.special_days = !s.special_days,
                Toggle::Siren => s.siren = !s.siren,
                Toggle::Ntfy => {
                    s.phone.ntfy.enabled = !s.phone.ntfy.enabled;
                    if s.phone.ntfy.enabled && s.phone.ntfy.topic.is_empty() {
                        s.phone.ntfy.topic = crate::remote::random_topic();
                    }
                }
                Toggle::Telegram => s.phone.telegram.enabled = !s.phone.telegram.enabled,
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
                    Button::AllSettings => Action::OpenSettings,
                    Button::PhoneTest => Action::PhoneTest,
                    Button::NewTopic => Action::NewTopic,
                    Button::CopyTopic => Action::CopyText(ntfy_link(&s)),
                    Button::ClearSessions => Action::ClearSessions,
                    Button::ReplyCancel => {
                        self.end_reply();
                        return None;
                    }
                    Button::ReplySend => return None,
                });
            }
        }
        (s != v.settings).then(|| Action::Settings(Box::new(s)))
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
        Rect::from_xywh(self.cx, y, self.cw, rows as f32 * ROW * self.ui)
    }

    /// Width of the label column of a row (physical).
    fn label_w(&self) -> f32 {
        (if self.window { LABEL_W_WINDOW } else { LABEL_W }) * self.ui
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
            t(label),
            sys(13.0 * u, false, INK),
        );
        // Segmented control on the right; options share the space by text width.
        let seg_x = x + self.label_w();
        let seg_w = inner - self.label_w() - 6.0 * u;
        let seg = Rect::from_xywh(seg_x, y + 5.0 * u, seg_w, 22.0 * u);
        fill_round(pm, seg, 8.0 * u, TRACK);
        let widths: Vec<f32> = options
            .iter()
            .map(|o| r.measure(t(o), sys(11.5 * u, false, INK)).0 + 10.0 * u)
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
            let o = t(o);
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
        tg: Toggle,
    ) -> f32 {
        let u = self.ui;
        let Some(row) = Rect::from_xywh(x, y, inner, ROW * u) else {
            return y + ROW * u;
        };
        if self.hover == Some(Hit::Toggle(tg)) {
            fill_round(pm, row.inset(2.0 * u), 10.0 * u, HOVER);
        }
        r.draw_text(
            pm,
            x + 12.0 * u,
            y + 8.0 * u,
            t(label),
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
        self.hits.push((row, Hit::Toggle(tg)));
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
        let track_x = x + self.label_w();
        let track_w = inner - self.label_w() - 20.0 * u - lw;
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
        let label = t(label);
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
    /// A bow tie: the Look tab.
    Bow,
    /// A bone: the Play tab.
    Bone,
    Speaker,
    /// A check mark: the Todo tab.
    Check,
    Clock,
    Plug,
    /// A cog: General, and the settings window.
    Gear,
    Phone,
    /// Two heads: the Agents page.
    Agents,
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
        Glyph::Bow => {
            // Two triangles meeting at a small knot.
            let mut pb = PathBuilder::new();
            for side in [-1.0f32, 1.0] {
                let (x0, y0) = p(side * 1.5, 0.0);
                let (x1, y1) = p(side * 7.0, -4.5);
                let (x2, y2) = p(side * 7.0, 4.5);
                pb.move_to(x0, y0);
                pb.line_to(x1, y1);
                pb.quad_to(x1 + side * 1.5 * u, (y1 + y2) / 2.0, x2, y2);
                pb.close();
            }
            fill(pm, pb);
            let mut pb = PathBuilder::new();
            oval(&mut pb, 0.0, 0.0, 2.2, 2.6);
            fill(pm, pb);
        }
        Glyph::Bone => {
            // A shaft with two round knobs at each end, tilted.
            let mut pb = PathBuilder::new();
            let (x0, y0) = p(-4.0, 3.0);
            let (x1, y1) = p(4.0, -3.0);
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            stroke(pm, pb, 3.0);
            let mut pb = PathBuilder::new();
            for (x, y) in [(-6.6, 2.4), (-4.6, 5.4), (6.6, -2.4), (4.6, -5.4)] {
                oval(&mut pb, x, y, 2.1, 2.1);
            }
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
        Glyph::Check => {
            let mut pb = PathBuilder::new();
            let (x0, y0) = p(-6.0, 0.5);
            let (x1, y1) = p(-2.0, 4.5);
            let (x2, y2) = p(6.0, -4.5);
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            pb.line_to(x2, y2);
            stroke(pm, pb, 2.2);
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
        Glyph::Gear => {
            let mut pb = PathBuilder::new();
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (x0, y0) = p(4.6 * a.cos(), 4.6 * a.sin());
                let (x1, y1) = p(7.0 * a.cos(), 7.0 * a.sin());
                pb.move_to(x0, y0);
                pb.line_to(x1, y1);
            }
            stroke(pm, pb, 2.4);
            let mut pb = PathBuilder::new();
            let (x, y) = p(0.0, 0.0);
            pb.push_circle(x, y, 4.6 * u);
            stroke(pm, pb, 1.8);
        }
        Glyph::Phone => {
            let (bx, by) = p(-4.0, -7.0);
            if let Some(r) = Rect::from_xywh(bx, by, 8.0 * u, 14.0 * u)
                && let Some(path) = round_rect(r, 2.0 * u)
            {
                let s = Stroke {
                    width: 1.6 * u,
                    ..Stroke::default()
                };
                pm.stroke_path(&path, &paint(c), &s, Transform::identity(), None);
            }
            let mut pb = PathBuilder::new();
            let (x0, y0) = p(-1.2, 4.5);
            let (x1, y1) = p(1.2, 4.5);
            pb.move_to(x0, y0);
            pb.line_to(x1, y1);
            stroke(pm, pb, 1.4);
        }
        Glyph::Agents => {
            let mut pb = PathBuilder::new();
            oval(&mut pb, -3.0, -2.5, 2.6, 2.6);
            oval(&mut pb, 4.0, -1.0, 2.1, 2.1);
            fill(pm, pb);
            let mut pb = PathBuilder::new();
            let (x0, y0) = p(-7.5, 6.5);
            let (xm, ym) = p(-3.0, -0.5);
            let (x1, y1) = p(1.5, 6.5);
            pb.move_to(x0, y0);
            pb.quad_to(xm, ym, x1, y1);
            let (x0, y0) = p(1.0, 5.5);
            let (xm, ym) = p(4.0, 0.5);
            let (x1, y1) = p(7.5, 5.5);
            pb.move_to(x0, y0);
            pb.quad_to(xm, ym, x1, y1);
            stroke(pm, pb, 1.8);
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

/// Keeps the end of `text` (with "…" in front) to fit `max` physical pixels.
fn fit_end(r: &mut Renderer, text: &str, st: TextStyle, max: f32) -> String {
    if r.measure(text, st).0 <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let s = String::from("…") + &chars[start..].iter().collect::<String>();
        if r.measure(&s, st).0 <= max {
            return s;
        }
        start += 1;
    }
    String::new()
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

/// Background of the settings window's sidebar.
const SIDEBAR_BG: ColorU8 = rgb(0xf3, 0xed, 0xe4);

/// The choices of "When" on the Phone page.
const PHONE_WHEN: [(PhoneWhen, &str); 3] = [
    (PhoneWhen::Away, "Away"),
    (PhoneWhen::Always, "Always"),
    (PhoneWhen::Never, "Never"),
];

/// The ntfy topic's web address (to open or subscribe to on the phone).
fn ntfy_link(s: &Settings) -> String {
    format!("{}/{}", s.phone.ntfy.server, s.phone.ntfy.topic)
}

/// The dog's state in words, and the colour of its dot.
fn status_label(v: &View) -> (&'static str, ColorU8) {
    match (v.visible, v.sleeping) {
        _ if v.silence => ("Quiet", MUTED),
        (false, true) => ("Hidden · asleep", MUTED),
        (true, true) => ("Sleeping", MUTED),
        (false, false) => ("Hidden", MUTED),
        (true, false) if v.quiet => ("Quiet", ACCENT),
        (true, false) => ("Awake", AWAKE),
    }
}

fn color8(c: ColorU8) -> Color {
    Color::from_rgba8(c.red(), c.green(), c.blue(), c.alpha())
}

/// A small red count centred at (`cx`, `cy`).
fn draw_badge(pm: &mut Pixmap, r: &mut Renderer, cx: f32, cy: f32, n: usize, u: f32) {
    let label = if n > 9 {
        "9+".to_string()
    } else {
        n.to_string()
    };
    let st = sys(9.5 * u, true, rgb(0xff, 0xff, 0xff));
    let (lw, lh) = r.measure(&label, st);
    let bw = (lw + 8.0 * u).max(16.0 * u);
    if let Some(b) = Rect::from_xywh(cx - bw / 2.0, cy - 8.0 * u, bw, 16.0 * u) {
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

    fn find(p: &Panel, hit: Hit) -> (f32, f32) {
        p.hits
            .iter()
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| center(*r))
            .unwrap_or_else(|| panic!("{hit:?} is not on screen"))
    }

    /// Scrolls the tab until `hit` is on screen.
    fn reach(p: &mut Panel, pm: &mut Pixmap, r: &mut Renderer, v: &View, hit: Hit) -> (f32, f32) {
        for _ in 0..40 {
            p.draw(pm, r, v);
            if p.hits.iter().any(|(_, h)| *h == hit) {
                return find(p, hit);
            }
            p.scroll_by(-1.0, pm.height());
        }
        panic!("{hit:?} not reached");
    }

    /// Applies an action's settings to the view, like the app does.
    fn apply(v: &mut View, a: Option<Action>) -> Settings {
        match a {
            Some(Action::Settings(s)) => {
                v.settings = *s;
                v.settings.clone()
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn clicks_produce_actions() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.draw(&mut pm, &mut r, &v);

        // Size steps by 50%.
        let (x, y) = find(&p, Hit::Choice(Field::Size, 1));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if s.size == 150));
        let (x, y) = find(&p, Hit::Choice(Field::Size, 0));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if s.size == 50));
        let (x, y) = find(&p, Hit::Choice(Field::Notify, 1));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if s.notify_secs == 15));
        let (x, y) = find(&p, Hit::Button(Button::Quit));
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Quit));

        // Sound lives on its own tab: switching tabs is not an action.
        let (x, y) = find(&p, Hit::Tab(Page::Sound));
        assert_eq!(p.pointer_up(x, y, &v), None);
        p.draw(&mut pm, &mut r, &v);
        assert!(
            !p.hits
                .iter()
                .any(|(_, h)| *h == Hit::Choice(Field::Size, 1))
        );
        let (x, y) = find(&p, Hit::Toggle(Toggle::Sound));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if !s.sound));

        // Walking is on the Dog tab, further down.
        p.set_tab(0);
        // Choosing what is already selected changes nothing.
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Choice(Field::Speed, 1));
        assert_eq!(p.pointer_up(x, y, &v), None);
    }

    #[test]
    fn play_tab_takes_the_toy_out() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.set_tab(TAB_PLAY);
        p.draw(&mut pm, &mut r, &v);
        // Dragging the tile away takes the toy out; a plain click throws it.
        let (x, y) = find(&p, Hit::ToyTile);
        assert_eq!(p.pointer_down(x, y), None);
        assert_eq!(p.pointer_move(x + 2.0, y, &v.settings).1, None);
        assert_eq!(
            p.pointer_move(x + 30.0, y - 30.0, &v.settings).1,
            Some(Action::TakeToy)
        );
        assert_eq!(p.pointer_down(x, y), None);
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Fetch));
        let (x, y) = find(&p, Hit::Toggle(Toggle::Toy));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if !s.toy));
    }

    #[test]
    fn connect_tab_edits_the_server() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let mut v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.set_page(Page::Connect);
        p.draw(&mut pm, &mut r, &v);

        // Address: a bad one is dropped, a good one saved.
        let (x, y) = find(&p, Hit::Text(TextField::McpAddr));
        p.pointer_up(x, y, &v);
        for _ in 0..30 {
            p.key(Key::Backspace, &v);
        }
        p.key(Key::Text("nope".into()), &v);
        assert_eq!(p.key(Key::Enter, &v).1, None);
        p.pointer_up(x, y, &v);
        for _ in 0..30 {
            p.key(Key::Backspace, &v);
        }
        p.key(Key::Text("127.0.0.1:8888".into()), &v);
        let s = {
            let a = p.key(Key::Enter, &v).1;
            apply(&mut v, a)
        };
        assert_eq!(s.mcp_addr, "127.0.0.1:8888");

        // Any, then Custom with a page of its own, then the page removed.
        let (x, y) = find(&p, Hit::Choice(Field::Browsers, 1));
        assert_eq!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .mcp_cors
            .allow_origins,
            ["*"]
        );
        p.draw(&mut pm, &mut r, &v);
        let (x, y) = find(&p, Hit::Choice(Field::Browsers, 2));
        assert!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .mcp_cors
            .allow_origins
            .is_empty()
        );
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Text(TextField::AddOrigin));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("https://app.example.com/".into()), &v);
        let s = {
            let a = p.key(Key::Enter, &v).1;
            apply(&mut v, a)
        };
        assert_eq!(s.mcp_cors.allow_origins, ["https://app.example.com"]);
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Remove(List::Origins, 0));
        assert!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .mcp_cors
            .allow_origins
            .is_empty()
        );
        let (x, y) = reach(
            &mut p,
            &mut pm,
            &mut r,
            &v,
            Hit::Toggle(Toggle::PrivateNetwork),
        );
        assert!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .mcp_cors
            .allow_private_network
        );
        let (x, y) = reach(
            &mut p,
            &mut pm,
            &mut r,
            &v,
            Hit::Toggle(Toggle::CheckUpdates),
        );
        assert!(
            !{
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .check_updates
        );
    }

    #[test]
    fn sound_and_look_lists() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let mut v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();

        p.set_tab(TAB_SOUND);
        // Quiet hours: a time is normalised, nonsense is dropped.
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Text(TextField::QuietFrom));
        p.pointer_up(x, y, &v);
        for _ in 0..10 {
            p.key(Key::Backspace, &v);
        }
        p.key(Key::Text("7:5".into()), &v);
        assert_eq!(
            {
                let a = p.key(Key::Enter, &v).1;
                apply(&mut v, a)
            }
            .quiet_from,
            "07:05"
        );
        p.pointer_up(x, y, &v);
        p.key(Key::Text("x".into()), &v);
        assert_eq!(p.key(Key::Enter, &v).1, None);
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Text(TextField::AddBark));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("\"~/woof.wav\"".into()), &v);
        assert_eq!(
            {
                let a = p.key(Key::Enter, &v).1;
                apply(&mut v, a)
            }
            .bark_files,
            ["~/woof.wav"]
        );
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Remove(List::Barks, 0));
        assert!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .bark_files
            .is_empty()
        );

        p.set_tab(TAB_LOOK);
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Text(TextField::AddSource));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("CI".into()), &v);
        let s = {
            let a = p.key(Key::Enter, &v).1;
            apply(&mut v, a)
        };
        let first = s.source_colors["ci"].clone();
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::SourceColor(0));
        let s = {
            let a = p.pointer_up(x, y, &v);
            apply(&mut v, a)
        };
        assert_ne!(s.source_colors["ci"], first, "next colour");
        let (x, y) = reach(&mut p, &mut pm, &mut r, &v, Hit::Remove(List::Sources, 0));
        assert!(
            {
                let a = p.pointer_up(x, y, &v);
                apply(&mut v, a)
            }
            .source_colors
            .is_empty()
        );
    }

    #[test]
    fn todo_tab() {
        use crate::todo::{Edit, Item, List};
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        let mut v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.set_tab(TAB_TODO);
        p.draw(&mut pm, &mut r, &v);
        // No lists: a field for a new one.
        let (x, y) = find(&p, Hit::Text(TextField::NewList));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("Release".into()), &v);
        assert_eq!(
            p.key(Key::Enter, &v).1,
            Some(Action::Todo(Edit::NewList("Release".into())))
        );
        v.todos = vec![List {
            id: 1,
            name: "Release".into(),
            items: vec![
                Item {
                    id: 2,
                    text: "tag".into(),
                    ..Item::default()
                },
                Item {
                    id: 3,
                    text: "build".into(),
                    done: true,
                    ..Item::default()
                },
            ],
        }];
        p.draw(&mut pm, &mut r, &v);
        let (x, y) = find(&p, Hit::TodoCheck(2));
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Todo(Edit::Toggle(2))));
        let (x, y) = find(&p, Hit::TodoRemove(3));
        assert_eq!(p.pointer_up(x, y, &v), Some(Action::Todo(Edit::Remove(3))));
        let (x, y) = find(&p, Hit::TodoClear(1));
        assert_eq!(
            p.pointer_up(x, y, &v),
            Some(Action::Todo(Edit::ClearDone(1)))
        );
        let (x, y) = find(&p, Hit::Text(TextField::AddTodo(1)));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("notes".into()), &v);
        assert_eq!(
            p.key(Key::Enter, &v).1,
            Some(Action::Todo(Edit::Add {
                list: "1".into(),
                text: "notes".into()
            }))
        );
    }

    #[test]
    fn look_tab_and_name_field() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        p.set_tab(TAB_LOOK);
        let v = view();
        let (w, h) = p.size(10_000);
        let mut pm = Pixmap::new(w, h).unwrap();
        p.draw(&mut pm, &mut r, &v);
        let (x, y) = find(&p, Hit::Fur(3));
        assert!(matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if s.fur == Fur::Black));
        let (x, y) = find(&p, Hit::Choice(Field::Wear, 1));
        assert!(
            matches!(p.pointer_up(x, y, &v), Some(Action::Settings(s)) if s.wear == Wear::Beanie)
        );

        // Click the name, type, Enter.
        let (x, y) = find(&p, Hit::Text(TextField::Name));
        assert_eq!(p.pointer_up(x, y, &v), None);
        assert!(p.is_editing());
        p.draw(&mut pm, &mut r, &v);
        for _ in 0..4 {
            p.key(Key::Backspace, &v);
        }
        p.key(Key::Text("Karabaş\n".into()), &v);
        let (_, a) = p.key(Key::Enter, &v);
        assert!(matches!(a, Some(Action::Settings(s)) if s.name == "Karabaş"));
        assert!(!p.is_editing());

        // Esc cancels; clicking elsewhere saves.
        p.pointer_up(x, y, &v);
        p.key(Key::Text("!".into()), &v);
        p.key(Key::Escape, &v);
        assert!(!p.is_editing());
        p.pointer_up(x, y, &v);
        p.key(Key::Text("y".into()), &v);
        let (tx, ty) = find(&p, Hit::Tab(Page::Dog));
        assert!(matches!(p.pointer_up(tx, ty, &v), Some(Action::Settings(s)) if s.name == "Dosty"));
        // Too long names stop at the limit.
        p.pointer_up(x, y, &v);
        p.key(Key::Text("x".repeat(40)), &v);
        let (_, a) = p.key(Key::Enter, &v);
        assert!(matches!(a, Some(Action::Settings(s)) if s.name.chars().count() == NAME_MAX));
        // A birthday must be a date.
        let (x, y) = find(&p, Hit::Text(TextField::Birthday));
        p.pointer_up(x, y, &v);
        p.key(Key::Text("2025-02-30".into()), &v);
        assert_eq!(p.key(Key::Enter, &v).1, None);
        p.pointer_up(x, y, &v);
        p.key(Key::Text("2025-03-14".into()), &v);
        assert!(
            matches!(p.key(Key::Enter, &v).1, Some(Action::Settings(s)) if s.birthday == "2025-03-14")
        );
    }

    #[test]
    fn every_tab_fits_and_scrolls() {
        let mut r = Renderer::new(false, Vec::new());
        for arrow in [None, Some(150.0)] {
            let mut p = Panel::new(1.0);
            p.set_arrow(arrow);
            let (w, h) = p.size(10_000);
            let mut pm = Pixmap::new(w, h).unwrap();
            for tab in TABS_POPOVER {
                p.set_page(tab);
                p.draw(&mut pm, &mut r, &view());
                // Nothing clickable outside the panel, and the footer is always there.
                assert!(
                    p.hits.iter().all(|(r, _)| r.bottom() <= h as f32 + 1.0),
                    "tab {tab:?}"
                );
                assert!(p.hits.iter().any(|(_, h)| *h == Hit::Button(Button::Quit)));
                // Scrolled to the end: no hit leaks over the tabs or the footer.
                p.scroll_by(-100.0, h);
                p.draw(&mut pm, &mut r, &view());
                let (_, tab_y) = find(&p, Hit::Tab(Page::Dog));
                let (_, quit_y) = find(&p, Hit::Button(Button::Quit));
                for (rect, hit) in &p.hits {
                    if !matches!(hit, Hit::Tab(_) | Hit::Button(_)) {
                        assert!(
                            rect.top() > tab_y && rect.bottom() < quit_y,
                            "tab {tab:?}: {hit:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn slider_drags_volume() {
        let mut r = Renderer::new(false, Vec::new());
        let mut p = Panel::new(1.0);
        p.set_tab(TAB_SOUND);
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
