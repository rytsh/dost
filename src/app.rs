//! Platform-independent app core: notification queue, settings, MCP server, watches and the
//! settings panel's actions. Each platform shell (`win.rs`, `mac.rs`) owns the windows and
//! implements [`Platform`] for the few things that differ (sound, clipboard, autostart, ...).

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::dog::{Anim, Bounds, Dog};
use crate::i18n::{self, t};
use crate::look::{self, Outfit};
use crate::mcp::{self, DogStatus, HISTORY_LEN, HistoryEntry, Mcp};
use crate::notify::{self, Event, Mood, Notification, Notifier, ProgressState, Reaction};
use crate::panel::{self, Action};
use crate::remote::{PhoneWhen, Remote, RemoteEvent, RemoteMessage};
use crate::render::{Bubble, Hits, Renderer, Scale};
use crate::sessions::{self, Sessions};
use crate::settings::{self, Settings, Store};
use crate::sound::{Sounds, Voice};
use crate::special;
use crate::todo::Todos;
use crate::toy::{self, Toy};
use crate::update;
use crate::watch::{self, Watches};

/// On average, the dog plays with the bone by itself once in this many seconds.
const SELF_PLAY_EVERY: f32 = 900.0;
/// A finished or failed progress bubble stays this long.
const PROGRESS_END: Duration = Duration::from_secs(6);
/// While other notifications wait, a running progress bubble yields after this long.
const PROGRESS_TURN: Duration = Duration::from_secs(8);
/// Away from the computer this long counts as gone: news meanwhile is summed up on return.
const AWAY: Duration = Duration::from_secs(5 * 60);
/// How often to look for a new version.
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// "Later" shows the bubble again after this long.
pub const SNOOZE: Duration = Duration::from_secs(10 * 60);
/// A new bubble ignores clicks this long after it appears.
const CLICK_GUARD: Duration = Duration::from_millis(400);

/// What a platform shell provides to the shared core.
pub trait Platform {
    /// Plays a 16-bit PCM WAV (already scaled to the volume); a new one stops the last.
    fn play_wav(&mut self, wav: Vec<u8>);
    fn copy_text(&mut self, text: &str) -> bool;
    fn autostart_enabled(&self) -> bool;
    fn set_autostart(&mut self, on: bool) -> Result<(), String>;
    fn open_folder(&mut self, path: &std::path::Path);
    /// Opens an http(s) URL in the default browser.
    fn open_url(&mut self, url: &str);
    /// Seconds since the last mouse or keyboard input, if the platform can tell.
    fn idle_secs(&self) -> Option<u64>;
    /// Something covers a whole screen (a video, a presentation, a game).
    fn fullscreen(&self) -> bool {
        false
    }
    /// The screen is locked (or the login window / screen saver is up).
    fn screen_locked(&self) -> bool {
        false
    }
    /// Posts a system notification (Notification Center / Windows toast).
    fn system_notify(&mut self, _title: &str, _body: &str) {}
    /// Shows or hides the dog's window.
    fn set_dog_visible(&mut self, visible: bool);
    fn close_panel(&mut self);
    fn quit(&mut self);
    /// "Start with Windows" / "Open at login".
    fn autostart_label(&self) -> &'static str;
    /// Opens the big settings window (or brings it to the front).
    fn open_settings_window(&mut self) {}
    /// Opens the panel on a text field to answer question `id` in words.
    fn open_reply(&mut self, _id: u64, _title: &str, _question: &str) {}
    /// How many screens there are (for the Screen setting).
    fn screen_count(&self) -> usize {
        1
    }
}

/// Global keyboard shortcuts for the bubble (⌥⌘ on macOS, Ctrl+Alt on Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Enter: the first choice, or open / close plain news.
    Accept,
    /// 1-4: that choice.
    Choice(usize),
    /// Backspace: the last choice ("no") or close.
    Dismiss,
    /// L: later.
    Later,
}

pub struct Shown {
    pub note: Notification,
    pub deadline: Option<Instant>,
    pub since: Instant,
}

pub struct Core {
    pub dog: Dog,
    pub renderer: Renderer,
    pub settings: Settings,
    pub store: Store,
    /// The user wants the dog shown (Hide/Show). While hidden it still walks in from the
    /// right edge to deliver important notifications; see [`Core::on_screen`].
    pub visible: bool,
    pub shown: Option<Shown>,
    pub queue: VecDeque<Notification>,
    pub hits: Hits,
    pub mcp_error: Option<String>,
    /// The panel must be redrawn (settings or state changed).
    pub panel_dirty: bool,
    /// Notifications that arrived while hidden and were not seen; cleared when the panel opens.
    pub missed: usize,
    rx: Receiver<Event>,
    notifier: Notifier,
    mcp: Arc<Mcp>,
    mcp_server: Option<mcp::Running>,
    status: Arc<Mutex<DogStatus>>,
    last: Instant,
    last_settings_check: Instant,
    next_id: u64,
    /// Reactions waiting for the next frame (they need the walking bounds).
    reactions: Vec<Reaction>,
    /// The dog window was shown last frame (to tell the shell when that changes).
    was_on_screen: bool,
    /// Hide/Show was asked for; applied on the next frame (it needs the walking bounds).
    toggle_visible: bool,
    /// The user just clicked a bubble away: the next queued one is shown without barking.
    seen: bool,
    /// Since when the user has been away (idle for [`AWAY`]), Unix seconds.
    away_since: Option<u64>,
    /// Notifications up to this id are summed up already (never twice).
    summed: u64,
    /// Where History is kept between runs.
    history_path: Option<std::path::PathBuf>,
    /// History changed since it was last written.
    history_dirty: bool,
    /// An app is full screen (checked each second).
    fullscreen: bool,
    /// The screen was locked at the last check.
    locked: bool,
    /// A running update check, and when the last one started.
    update: Option<Receiver<update::Found>>,
    last_update_check: Option<Instant>,
    /// The version found newer than this one, for the panel.
    pub new_version: Option<String>,
    /// A newer version is installed and waits for a restart.
    pub update_ready: bool,
    /// The user asked (Check now): say so even when there is nothing new.
    update_asked: bool,
    /// The "Restart" question's answer.
    restart_answer: Option<Receiver<String>>,
    /// Important notifications that arrived while locked: (title, body), for the system
    /// notification after unlocking.
    while_locked: Vec<(String, String)>,
    last_fullscreen_check: Instant,
    last_history_save: Instant,
    /// Barks and event voices, rebuilt when the sound files change.
    sounds: Sounds,
    sound_files: Vec<String>,
    /// The toy, while the dog is on screen.
    pub toy: Toy,
    /// The day the outfit was last chosen (holidays change at midnight).
    look_day: Option<chrono::NaiveDate>,
    /// The phone channels (ntfy, Telegram).
    remote: Remote,
    /// The last problem a phone channel had, for the settings.
    pub phone_status: Option<String>,
    /// The phone settings the channels were last set up with.
    phone_cfg: Option<crate::remote::PhoneSettings>,
    /// Notifications sent to the phone that wait for an answer there: id -> sent.
    remote_open: std::collections::HashSet<u64>,
    /// What agents are doing.
    pub sessions: Arc<Sessions>,
    last_session_check: Instant,
    /// The minute of silence (10 November, 09:05) lasts until then.
    silence_until: Option<Instant>,
    /// The day the special-day bubble was shown (once a day).
    special_shown: Option<chrono::NaiveDate>,
    last_special_check: Instant,
}

impl Core {
    /// Loads settings, starts watches and (if enabled) the MCP server.
    /// `x`, `ground`: where the dog starts (physical pixels).
    pub fn new(renderer: Renderer, x: f32, ground: f32) -> Self {
        let mut store = Store::new(settings::config_dir().map(|d| d.join("settings.json")));
        let mut settings = store.load();
        let first_start = settings.birthday.is_empty();
        if first_start {
            // The day it was adopted is its birthday.
            settings.birthday = look::today().format("%Y-%m-%d").to_string();
            store.save(&settings);
        }
        update::cleanup();
        let mut core = Self::build(renderer, x, ground, store, watch::default_path());
        core.load_history(settings::config_dir().map(|d| d.join("history.json")));
        core.restore(&settings);
        core.apply_settings(settings, false);
        // Hidden or asleep from last time: no hello bark.
        let quiet_start = !core.visible || core.dog.is_put_to_sleep();
        let name = core.settings.name.clone();
        let tr = i18n::is_turkish();
        let hello = match &core.mcp_error {
            None if quiet_start => None,
            None => Some(
                match look::Season::on(look::today(), core.settings.birthday()) {
                    Some(look::Season::Birthday) if tr => Notification::new(
                        "Hav! Bugün doğum günüm!",
                        format!("{name} bugün {} oldu. Bana baktığın için teşekkürler.", core.age_text()),
                    )
                    .mood(Some(Mood::Good)),
                    Some(look::Season::Birthday) => Notification::new(
                        "Woof! It's my birthday!",
                        format!("{name} is {} today. Thanks for having me.", core.age_text()),
                    )
                    .mood(Some(Mood::Good)),
                    _ if first_start && tr => Notification::new(
                        "Hav!",
                        format!(
                            "Merhaba, ben {name}. Beni sürükleyebilir, kemiğimi fırlatabilir ya da bana bir ad vermek için simgeme tıklayabilirsin."
                        ),
                    ),
                    _ if first_start => Notification::new(
                        "Woof!",
                        format!(
                            "Hi, I'm {name}. Drag me around, throw my bone, or click my tray icon to give me a name."
                        ),
                    ),
                    _ if tr => Notification::new(
                        "Hav!",
                        format!("{name} burada. Beni sürükleyebilir ya da ayarlar için simgeme tıklayabilirsin."),
                    ),
                    _ => Notification::new(
                        "Woof!",
                        format!("{name} is here. Drag me around, or click my tray icon for settings."),
                    ),
                }
                .local()
                .timeout(Duration::from_secs(6)),
            ),
            Some(e) => Some(
                Notification::new(t("MCP server could not start"), e.clone())
                    .local()
                    .sticky(),
            ),
        };
        if let Some(hello) = hello {
            core.notifier.send(hello);
        }
        core
    }

    /// "2 years" / "1 year" / "3 days" since the birthday.
    pub fn age_text(&self) -> String {
        let a = age_text(self.settings.birthday(), look::today());
        if !i18n::is_turkish() {
            return a;
        }
        match a.split_once(' ') {
            Some(("0", _)) => "bugün geldi".into(),
            Some((n, unit)) => {
                let unit = match unit.trim_end_matches('s') {
                    "year" => "yaşında",
                    "month" => "aylık",
                    _ => "günlük",
                };
                format!("{n} {unit}")
            }
            None => t(&a).to_string(),
        }
    }

    fn build(
        renderer: Renderer,
        x: f32,
        ground: f32,
        store: Store,
        watches_path: Option<std::path::PathBuf>,
    ) -> Self {
        let (notifier, rx) = notify::channel();
        let status = Arc::new(Mutex::new(DogStatus::default()));
        let watches = Watches::new(notifier.clone(), watches_path.clone());
        watches.start();
        // Todo lists live next to the watches (none in tests).
        let todos = Arc::new(Todos::new(
            watches_path
                .as_ref()
                .and_then(|p| Some(p.parent()?.join("todos.json"))),
        ));
        let sessions = Arc::new(Sessions::default());
        // Scheduled jobs live next to the watches too.
        let cron = crate::cron::Cron::new(
            notifier.clone(),
            watches_path
                .as_ref()
                .and_then(|p| Some(p.parent()?.join("cron.json"))),
        );
        cron.start();
        let mcp = Arc::new(Mcp::new(
            notifier.clone(),
            watches,
            Arc::clone(&todos),
            Arc::clone(&status),
            Arc::clone(&sessions),
            cron,
        ));
        let remote = Remote::new(notifier.remote_sender());
        Core {
            dog: Dog::new(x.round(), ground),
            renderer,
            settings: Settings::default(),
            store,
            visible: true,
            shown: None,
            queue: VecDeque::new(),
            hits: Hits::default(),
            mcp_error: None,
            panel_dirty: true,
            rx,
            notifier,
            mcp,
            mcp_server: None,
            status,
            last: Instant::now(),
            last_settings_check: Instant::now(),
            next_id: 0,
            reactions: Vec::new(),
            missed: 0,
            was_on_screen: true,
            toggle_visible: false,
            seen: false,
            away_since: None,
            summed: 0,
            history_path: None,
            history_dirty: false,
            fullscreen: false,
            locked: false,
            update: None,
            last_update_check: None,
            new_version: None,
            update_ready: false,
            update_asked: false,
            restart_answer: None,
            while_locked: Vec::new(),
            last_fullscreen_check: Instant::now(),
            last_history_save: Instant::now(),
            sounds: Sounds::default(),
            sound_files: Vec::new(),
            toy: Toy::new(x + 90.0, ground),
            look_day: None,
            remote,
            phone_status: None,
            phone_cfg: None,
            remote_open: Default::default(),
            sessions,
            last_session_check: Instant::now(),
            silence_until: None,
            special_shown: None,
            last_special_check: Instant::now() - Duration::from_secs(10),
        }
    }

    /// Loads History from the last run. Whatever was still waiting then is gone now.
    fn load_history(&mut self, path: Option<std::path::PathBuf>) {
        let old: Vec<HistoryEntry> = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        self.history_path = path;
        let Ok(mut s) = self.status.lock() else {
            return;
        };
        for mut h in old.into_iter().rev().take(HISTORY_LEN).rev() {
            if h.outcome == "queued" || h.outcome == "shown" || h.outcome == "updated" {
                h.outcome = "closed".into();
            }
            // Ids start over each run.
            h.id = 0;
            s.history.push_back(h);
        }
    }

    /// Writes History at most every few seconds, and right away with `now`.
    fn save_history(&mut self, now: bool) {
        if !self.history_dirty
            || (!now && self.last_history_save.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        let Some(path) = self.history_path.clone() else {
            return;
        };
        self.history_dirty = false;
        self.last_history_save = Instant::now();
        let data = match self.status.lock() {
            Ok(s) => serde_json::to_vec_pretty(&s.history),
            Err(_) => return,
        };
        if let Ok(data) = data {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Saves what must survive a quit (History). Called by the shells before exiting.
    pub fn shutdown(&mut self) {
        self.save_history(true);
    }

    /// Hidden / asleep as saved last time; hidden starts off screen without walking off.
    fn restore(&mut self, s: &Settings) {
        if s.hidden {
            // The shells check on_screen() before showing the dog's window.
            self.visible = false;
            self.was_on_screen = false;
            self.dog.start_away();
        }
        if s.asleep {
            self.dog.sleep();
        }
    }

    /// Saves hidden / asleep when they change, so the next start looks the same.
    fn remember_state(&mut self) {
        if self.toggle_visible {
            return;
        }
        let (hidden, asleep) = (!self.visible, self.dog.is_put_to_sleep());
        if self.settings.hidden != hidden || self.settings.asleep != asleep {
            self.settings.hidden = hidden;
            self.settings.asleep = asleep;
            self.store.save(&self.settings);
        }
    }

    /// One frame: notifications, settings file changes, the dog's movement.
    /// `bounds`: where the dog may walk on its current screen.
    pub fn tick(&mut self, p: &mut dyn Platform, bounds: &Bounds) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;

        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Notify(n) => self.receive(*n),
                Event::React(r) => self.reactions.push(r),
                Event::Dismiss(key) => self.dismiss(&key),
                Event::PanelChanged => self.panel_dirty = true,
                Event::Remote(e) => self.on_remote(e),
                Event::AllowAlways(rule) => {
                    let mut s = self.settings.clone();
                    if !s.local_mcp.always_allow.contains(&rule) {
                        s.local_mcp.always_allow.push(rule);
                        self.apply_settings(s, true);
                    }
                }
                Event::Record(entry) => self.record_entry(entry),
            }
        }
        if now - self.last_special_check >= Duration::from_secs(1) {
            self.last_special_check = now;
            self.special_days(p);
        }
        if now - self.last_session_check >= Duration::from_secs(30) {
            self.last_session_check = now;
            if self.sessions.expire(watch::now_unix()) {
                self.panel_dirty = true;
            }
        }
        for r in std::mem::take(&mut self.reactions) {
            if self.in_silence() {
                continue;
            }
            self.react(p, r, bounds);
        }
        if std::mem::take(&mut self.toggle_visible) {
            self.set_visible(!self.visible, bounds);
        }
        if now - self.last_fullscreen_check > Duration::from_secs(1) {
            self.last_fullscreen_check = now;
            let fs = self.settings.quiet_fullscreen && p.fullscreen();
            if fs != self.fullscreen {
                self.fullscreen = fs;
                self.panel_dirty = true;
            }
            self.check_lock(p);
            self.check_update(false);
        }
        if now - self.last_settings_check > Duration::from_secs(1) {
            self.last_settings_check = now;
            if let Some(s) = self.store.reload_if_changed() {
                self.apply_settings(s, false);
            }
        }
        self.expire(now);
        if self
            .shown
            .as_ref()
            .is_some_and(|s| self.should_hide(s, now))
        {
            let s = self.shown.take().unwrap();
            if s.note
                .progress
                .as_ref()
                .is_some_and(|p| p.state == ProgressState::Running)
            {
                // Taking turns with other notifications; it comes back after them.
                let mut note = s.note;
                note.announced = true;
                self.queue.push_back(note);
            }
        }
        if self.shown.is_none() && self.ready_for_next() && !self.in_silence() {
            self.show_next(p, now);
        }
        self.dog.hold = (self.shown.is_some() || self.in_silence()) && !self.dog.is_performing();
        self.welcome_back(p.idle_secs());
        self.restart_if_asked(p);
        self.publish_status(p);
        self.dog.update(dt, bounds);
        self.play(bounds, dt);
        if self.dog.take_left() {
            self.panel_dirty = true;
        }
        if self.look_day != Some(look::today()) {
            // Midnight: a holiday may start or end.
            self.apply_look();
        }
        let on = self.on_screen();
        if on != self.was_on_screen {
            self.was_on_screen = on;
            p.set_dog_visible(on);
        }
        self.remember_state();
        self.save_history(false);
    }

    /// Notifications waiting to be shown now (not the snoozed ones).
    pub fn waiting(&self) -> usize {
        let now = Instant::now();
        self.queue
            .iter()
            .filter(|n| n.not_before.is_none_or(|t| now >= t))
            .count()
    }

    /// Whether the dog window should be shown: when visible, or while walking off after Hide.
    pub fn on_screen(&self) -> bool {
        self.visible || !self.dog.is_away()
    }

    /// While hidden, bubbles are shown on their own at the top right (under the tray icon)
    /// instead of above the dog.
    pub fn bubble_alone(&self) -> bool {
        !self.visible && self.shown.is_some()
    }

    /// Waits for the dog to finish walking off before showing a detached bubble.
    fn ready_for_next(&self) -> bool {
        self.visible || self.dog.is_away()
    }

    /// The toy and fetch: moves the toy, sends the dog after it when it lands, and hands it
    /// over between the toy and the dog's mouth.
    fn play(&mut self, b: &Bounds, dt: f32) {
        if !self.settings.toy {
            // Put away: the game ends, the toy goes with it.
            if self.dog.is_fetching() {
                self.dog.stop_fetch();
                let _ = self.dog.take_dropped();
            }
            let _ = self.dog.take_picked();
            if self.toy.state == toy::State::Carried {
                self.toy.put_down(self.dog.mouth_x(), b.ground);
            }
            return;
        }
        let edges = self.toy_edges(b);
        self.toy.update(dt, b, edges);
        if self.dog.take_picked() {
            self.toy.carry();
        }
        if let Some(x) = self.dog.take_dropped() {
            let y = if self.dog.is_held() {
                self.dog.y - 60.0 * b.scale
            } else {
                b.ground
            };
            self.toy.put_down(x.clamp(edges.0, edges.1), y);
        }
        if self.toy.take_landed() && self.toy_shown() {
            self.dog.fetch(self.toy.x, b);
        } else if self.settings.wander
            && self.shown.is_none()
            && self.toy.state == toy::State::Resting
            && self.toy_shown()
            && !self.dog.is_performing()
            && !self.dog.is_held()
            && !self.dog.is_sleeping()
            && !self.in_silence()
            && fastrand::f32() < dt / SELF_PLAY_EVERY
        {
            // Now and then it goes and plays with the bone by itself.
            self.dog.fetch(self.toy.x, b);
        }
        // The game ended some other way while the toy was in the mouth.
        if self.toy.state == toy::State::Carried && !self.dog.is_carrying() {
            let x = self.dog.mouth_x().clamp(edges.0, edges.1);
            self.toy.put_down(x, b.ground);
        }
    }

    /// Where the toy may go: the walkable area plus a little.
    fn toy_edges(&self, b: &Bounds) -> (f32, f32) {
        let m = (EDGE - 20.0) * b.scale;
        (b.left - m, b.right + m)
    }

    /// The toy is drawn: the dog is out, and the toy is not in its mouth.
    pub fn toy_shown(&self) -> bool {
        self.settings.toy && self.visible && !self.dog.is_away() && self.toy.is_shown()
    }

    /// The toy is grabbed with the mouse; the dog stops chasing it.
    pub fn grab_toy(&mut self) {
        self.dog.stop_fetch();
        self.toy.grab();
    }

    /// The toy is taken out (dragged from the panel) and held at (`x`, `y`), physical: it
    /// comes back if it was put away, and leaves the dog's mouth.
    pub fn take_toy(&mut self, x: f32, y: f32) {
        if !self.settings.toy {
            let s = Settings {
                toy: true,
                ..self.settings.clone()
            };
            self.apply_settings(s, true);
        }
        self.dog.stop_fetch();
        let _ = (self.dog.take_picked(), self.dog.take_dropped());
        self.toy.put_down(x, y);
        self.toy.grab();
    }

    /// Right click on the toy: put away (the Bone button or the Walk tab bring it back).
    pub fn put_toy_away(&mut self) {
        if self.settings.toy {
            let s = Settings {
                toy: false,
                ..self.settings.clone()
            };
            self.apply_settings(s, true);
        }
    }

    /// Throws the toy from the dog's mouth (or where it lies) in a random arc.
    fn throw_toy(&mut self, b: &Bounds) {
        if !self.settings.toy || !self.visible || self.dog.is_away() || self.dog.is_put_to_sleep() {
            return;
        }
        self.dog.stop_fetch();
        let mid = (b.left + b.right) / 2.0;
        let dir = if self.dog.x < mid { 1.0 } else { -1.0 };
        let r = fastrand::f32();
        let (sx, sy) = (self.dog.x, b.ground - 70.0 * b.scale);
        self.toy.put_down(sx, sy);
        self.toy.throw(
            dir * (500.0 + 700.0 * r) * b.scale,
            -(700.0 + 300.0 * r) * b.scale,
        );
    }

    /// Plays a voice at the set volume.
    fn voice(&mut self, p: &mut dyn Platform, v: Voice) {
        if let Some(wav) = self
            .sounds
            .wav(v, self.settings.volume, self.settings.event_sounds)
        {
            p.play_wav(wav);
        }
    }

    /// The voice for a notification.
    fn voice_of(n: &Notification) -> Voice {
        if n.is_question() {
            return Voice::Ask;
        }
        match n.mood {
            Mood::Good => Voice::Happy,
            Mood::Bad => Voice::Sad,
            Mood::Neutral => Voice::Bark,
        }
    }

    /// The dog reacts to a notification it shows: barks, or is happy or sad.
    fn react_to(&mut self, n: &Notification) {
        match n.mood {
            _ if n.is_question() => self.dog.notify(),
            Mood::Good => self.dog.pet(),
            Mood::Bad => self.dog.sad(),
            Mood::Neutral => self.dog.notify(),
        }
    }

    /// Recolours the dog and dresses it for the day.
    fn apply_look(&mut self) {
        let today = look::today();
        self.look_day = Some(today);
        let s = &self.settings;
        let season = s
            .seasonal
            .then(|| look::Season::on(today, s.birthday()))
            .flatten();
        let outfit = Outfit::new(s.wear, season);
        let pin = s.special_days.then(|| special::Pin::on(today)).flatten();
        self.renderer
            .set_look(s.fur, s.collar, outfit, s.theme, s.source_colors());
        self.renderer.set_pin(pin);
    }

    /// Hide: the dog walks off to the right. Show: it walks back in.
    pub fn set_visible(&mut self, visible: bool, b: &Bounds) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.dog.enter(b);
        } else {
            self.dog.leave(b);
        }
        self.panel_dirty = true;
    }

    /// The panel was opened: if something was missed while hidden, it opens on History.
    /// Returns true when it should (the missed count clears once that tab is shown).
    pub fn panel_opened(&mut self) -> bool {
        if self.missed > 0 {
            self.missed = 0;
            self.panel_dirty = true;
            return true;
        }
        false
    }

    /// Takes in a new notification: numbers it, records it, and either queues it or applies it
    /// as an update to a progress bubble with the same id.
    fn receive(&mut self, mut n: Notification) {
        if let Some(pr) = &n.progress {
            let target = self
                .shown
                .as_mut()
                .map(|s| &mut s.note)
                .into_iter()
                .chain(self.queue.iter_mut())
                .find(|q| q.progress.as_ref().is_some_and(|qp| qp.id == pr.id));
            if let Some(q) = target {
                let done = pr.state != ProgressState::Running;
                q.title = n.title;
                q.body = n.body;
                q.progress = n.progress;
                if done {
                    q.timeout = notify::Timeout::After(PROGRESS_END);
                }
                let id = q.id;
                if done && let Some(s) = self.shown.as_mut().filter(|s| s.note.id == id) {
                    s.deadline = Some(Instant::now() + PROGRESS_END);
                    match s.note.progress.as_ref().map(|p| p.state) {
                        Some(ProgressState::Done) => self.dog.pet(),
                        _ => self.dog.sad(),
                    }
                }
                self.update_history(id, |h| {
                    h.outcome = if done {
                        "finished".into()
                    } else {
                        "updated".into()
                    };
                });
                return;
            }
        }
        self.next_id += 1;
        n.id = self.next_id;
        self.note_locked(&n);
        self.record(&n, "queued");
        if let Some(sid) = n.session.clone() {
            self.session_from(&n, &sid);
        }
        self.send_to_phone(&n);
        self.queue.push_back(n);
    }

    /// The user is away from the computer: idle for a while, or the screen is locked.
    fn user_away(&self) -> bool {
        self.away_since.is_some() || self.locked
    }

    /// Sends a notification to the phone when the settings say so.
    fn send_to_phone(&mut self, n: &Notification) {
        let ph = &self.settings.phone;
        if n.local || n.silent || !ph.any_enabled() {
            return;
        }
        if ph.when == PhoneWhen::Away && !self.user_away() {
            return;
        }
        // A running progress bar is not news; its end is.
        if n.progress
            .as_ref()
            .is_some_and(|p| p.state == ProgressState::Running)
        {
            return;
        }
        if n.is_question() {
            self.remote_open.insert(n.id);
        }
        self.remote.send(RemoteMessage {
            id: n.id,
            source: n.source.clone(),
            title: n.title.clone(),
            body: n.body.clone(),
            choices: n.choices.clone(),
            allow_text: n.allow_text,
            url: n.url.clone(),
            mood: match n.mood {
                Mood::Good => "good",
                Mood::Bad => "bad",
                Mood::Neutral => "neutral",
            },
        });
    }

    /// A question sent to the phone ended here (answered, expired, withdrawn).
    fn phone_closed(&mut self, id: u64, outcome: &str) {
        if self.remote_open.remove(&id) {
            self.remote.resolve(id, outcome);
        }
    }

    fn on_remote(&mut self, e: RemoteEvent) {
        match e {
            RemoteEvent::Answer { id, answer } => {
                self.remote_open.remove(&id);
                let shown = self.shown.as_ref().is_some_and(|s| s.note.id == id);
                let note = if shown {
                    self.shown.take().map(|s| s.note)
                } else {
                    self.queue
                        .iter()
                        .position(|n| n.id == id)
                        .and_then(|i| self.queue.remove(i))
                };
                let Some(note) = note else { return };
                // A button's label, or (when allowed) the user's own words.
                let ok = note.choices.contains(&answer) || note.allow_text;
                if !ok {
                    if shown {
                        self.shown = Some(Shown {
                            note,
                            deadline: None,
                            since: Instant::now(),
                        });
                    } else {
                        self.queue.push_front(note);
                    }
                    return;
                }
                self.send_answer(&note, &answer, "answered on phone");
                if shown {
                    self.seen = self.waiting() > 0;
                }
            }
            RemoteEvent::Paired { chat_id } => {
                let mut s = self.settings.clone();
                s.phone.telegram.chat_id = chat_id;
                self.apply_settings(s, true);
                self.push(
                    Notification::new("Telegram", t("Dost can reach your phone."))
                        .local()
                        .mood(Some(Mood::Good)),
                );
            }
            RemoteEvent::Error { channel, error } => {
                self.phone_status = Some(format!("{channel}: {error}"));
                self.panel_dirty = true;
            }
        }
    }

    /// Hands an answer to whoever asked, and records it.
    fn send_answer(&mut self, note: &Notification, answer: &str, how: &str) {
        if let Some(tx) = &note.reply {
            let _ = tx.send(answer.to_string());
        }
        if let Some(sid) = &note.session {
            self.sessions.report(
                sessions::Report {
                    id: sid.clone(),
                    state: Some(sessions::State::Working),
                    ..Default::default()
                },
                watch::now_unix(),
            );
        }
        let id = note.id;
        let text: String = answer.chars().take(60).collect();
        self.update_history(id, |h| h.outcome = format!("{how}: {text}"));
        self.dog.pet();
    }

    /// A notification from an agent session updates that session.
    fn session_from(&mut self, n: &Notification, sid: &str) {
        let state = if n.is_question() {
            Some(sessions::State::Waiting)
        } else if let Some(p) = &n.progress {
            Some(match p.state {
                ProgressState::Running => sessions::State::Working,
                ProgressState::Done => sessions::State::Done,
                ProgressState::Failed => sessions::State::Failed,
            })
        } else {
            match n.mood {
                Mood::Good => Some(sessions::State::Done),
                Mood::Bad => Some(sessions::State::Failed),
                Mood::Neutral => None,
            }
        };
        let detail = if n.body.is_empty() { &n.title } else { &n.body };
        self.sessions.report(
            sessions::Report {
                id: sid.to_string(),
                source: n.source.clone(),
                state,
                detail: Some(detail.chars().take(120).collect()),
                ..Default::default()
            },
            watch::now_unix(),
        );
        self.panel_dirty = true;
    }

    /// The minute of silence is on.
    pub fn in_silence(&self) -> bool {
        self.silence_until.is_some_and(|t| Instant::now() < t)
    }

    /// 10 November: the minute of silence at 09:05 and the remembrance bubble after it;
    /// national holidays: a bubble in the morning.
    fn special_days(&mut self, p: &mut dyn Platform) {
        if !self.settings.special_days {
            self.silence_until = None;
            return;
        }
        let now = chrono::Local::now().naive_local();
        let today = now.date();
        if let Some(left) = special::silence_left(now) {
            if self.silence_until.is_none() {
                self.silence_until = Some(Instant::now() + Duration::from_secs(left as u64));
                // The bubble on screen steps aside; it comes back after.
                if let Some(s) = self.shown.take() {
                    let mut note = s.note;
                    note.announced = true;
                    self.queue.push_front(note);
                }
                self.dog.stand_still();
                if self.settings.siren
                    && self.settings.sound
                    && let Some(wav) =
                        crate::sound::scaled_wav(&special::siren_wav(), self.settings.volume)
                {
                    p.play_wav(wav);
                }
                self.panel_dirty = true;
            }
            return;
        }
        if self.silence_until.is_some_and(|t| Instant::now() >= t) {
            self.silence_until = None;
            self.panel_dirty = true;
        }
        if self.special_shown == Some(today) {
            return;
        }
        let tr = i18n::is_turkish();
        let text = if special::remembrance_due(now) {
            Some(special::remembrance_text(tr))
        } else if now.time() >= chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap() {
            special::holiday_text(today, tr)
        } else {
            None
        };
        let Some((title, body)) = text else { return };
        self.special_shown = Some(today);
        let mut n = Notification::new(title, body)
            .local()
            .timeout(Duration::from_secs(30));
        if special::Day::on(today) == Some(special::Day::Remembrance) {
            n.silent = true;
        } else {
            n.mood = Mood::Good;
        }
        self.push(n);
    }

    /// Withdraws notifications with this key (e.g. a question answered in the terminal).
    /// Dropping a question also drops its reply sender, so the waiting `ask` returns.
    fn dismiss(&mut self, key: &str) {
        let mut gone = Vec::new();
        if self
            .shown
            .as_ref()
            .is_some_and(|s| s.note.key.as_deref() == Some(key))
        {
            gone.push(self.shown.take().unwrap().note.id);
        }
        self.queue.retain(|n| {
            let keep = n.key.as_deref() != Some(key);
            if !keep {
                gone.push(n.id);
            }
            keep
        });
        for id in gone {
            self.phone_closed(id, "answered elsewhere");
            self.update_history(id, |h| h.outcome = "answered elsewhere".into());
        }
    }

    /// Drops questions whose time ran out, shown or queued.
    fn expire(&mut self, now: Instant) {
        let mut gone = Vec::new();
        if self.shown.as_ref().is_some_and(|s| s.note.is_expired(now)) {
            gone.push(self.shown.take().unwrap().note.id);
        }
        self.queue.retain(|n| {
            let keep = !n.is_expired(now);
            if !keep {
                gone.push(n.id);
            }
            keep
        });
        for id in gone {
            self.phone_closed(id, "expired");
            self.update_history(id, |h| h.outcome = "expired (no answer)".into());
        }
    }

    fn should_hide(&self, s: &Shown, now: Instant) -> bool {
        if s.deadline.is_some_and(|d| now >= d) {
            return true;
        }
        // A running progress bar does not block what is waiting behind it.
        s.note
            .progress
            .as_ref()
            .is_some_and(|p| p.state == ProgressState::Running)
            && self.waiting() > 0
            && now - s.since >= PROGRESS_TURN
    }

    fn show_next(&mut self, p: &mut dyn Platform, now: Instant) {
        let quiet = self.hushed();
        let seen = std::mem::take(&mut self.seen);
        let why = if self.settings.is_quiet(local_minute()) {
            "skipped (quiet)"
        } else if self.dog.is_put_to_sleep() {
            "skipped (asleep)"
        } else {
            "skipped (full screen)"
        };
        while let Some(i) = self
            .queue
            .iter()
            .position(|n| n.not_before.is_none_or(|t| now >= t))
        {
            let Some(mut note) = self.queue.remove(i) else {
                return;
            };
            if note.not_before.take().is_some() {
                self.update_history(note.id, |h| h.outcome = "reminded".into());
            }
            if quiet && !note.is_important() {
                self.update_history(note.id, |h| h.outcome = why.into());
                continue;
            }
            // Hidden: plain news barks and waits in History; important ones get a bubble.
            if !self.visible && !note.is_important() {
                if !note.announced {
                    if self.settings.sound && !quiet {
                        self.voice(p, Self::voice_of(&note));
                    }
                    if let Some(text) = &note.copy {
                        p.copy_text(text);
                    }
                }
                self.missed += 1;
                self.update_history(note.id, |h| h.outcome = "missed (hidden)".into());
                continue;
            }
            if !note.announced && !note.silent {
                if !seen {
                    self.react_to(&note);
                }
                if self.settings.sound && !quiet && !seen {
                    self.voice(p, Self::voice_of(&note));
                }
                if let Some(text) = &note.copy {
                    p.copy_text(text);
                }
            }
            let deadline = note
                .resolve_timeout(self.settings.notify_timeout())
                .map(|t| now + t);
            let id = note.id;
            self.shown = Some(Shown {
                note,
                deadline,
                since: now,
            });
            self.update_history(id, |h| {
                if h.outcome == "queued" {
                    h.outcome = "shown".into();
                }
            });
            return;
        }
    }

    fn react(&mut self, p: &mut dyn Platform, r: Reaction, bounds: &Bounds) {
        match r {
            Reaction::Happy => self.dog.pet(),
            Reaction::Bark => {
                self.dog.notify();
                if self.settings.sound && !self.hushed() {
                    self.voice(p, Voice::Bark);
                }
            }
            Reaction::Sleep => self.dog.sleep(),
            Reaction::Wake => self.dog.wake_up(),
            Reaction::Zoomies => self.dog.zoomies(bounds),
            Reaction::Sad => self.dog.sad(),
            Reaction::Fetch => self.throw_toy(bounds),
        }
        self.panel_dirty = true;
    }

    /// No barking and only important bubbles: quiet hours / Quiet on, the dog was put
    /// to sleep, or the minute of silence.
    pub fn hushed(&self) -> bool {
        self.settings.is_quiet(local_minute())
            || self.dog.is_put_to_sleep()
            || self.fullscreen
            || self.in_silence()
    }

    fn record(&mut self, n: &Notification, outcome: &str) {
        self.panel_dirty = true;
        self.history_dirty = true;
        if let Ok(mut s) = self.status.lock() {
            s.history.push_back(HistoryEntry {
                id: n.id,
                at: watch::now_unix(),
                source: n.source.clone(),
                title: n.title.clone(),
                message: n.body.clone(),
                outcome: outcome.into(),
            });
            while s.history.len() > HISTORY_LEN {
                s.history.pop_front();
            }
        }
    }

    /// Keeps something that is not a notification in History (a workspace tool's work).
    fn record_entry(&mut self, mut e: HistoryEntry) {
        self.panel_dirty = true;
        self.history_dirty = true;
        e.at = watch::now_unix();
        if let Ok(mut s) = self.status.lock() {
            s.history.push_back(e);
            while s.history.len() > HISTORY_LEN {
                s.history.pop_front();
            }
        }
    }

    fn update_history(&mut self, id: u64, f: impl FnOnce(&mut HistoryEntry)) {
        self.panel_dirty = true;
        self.history_dirty = true;
        if let Ok(mut s) = self.status.lock()
            && let Some(h) = s.history.iter_mut().rev().find(|h| h.id == id)
        {
            f(h);
        }
    }

    /// Queues a notification from the app itself (not via MCP).
    fn push(&mut self, n: Notification) {
        self.receive(n);
    }

    /// The dog's scale on a screen with the given UI (DPI) scale.
    pub fn scale(&self, ui: f32) -> Scale {
        Scale::new(ui, self.settings.size).with_text(self.settings.text_size)
    }

    pub fn bubble(&self) -> Option<Bubble<'_>> {
        bubble_of(self.shown.as_ref(), self.waiting())
    }

    /// Click on the dog's window that was not a drag: answers a question, opens a link,
    /// closes the bubble or pets the dog.
    pub fn click(&mut self, p: &mut dyn Platform, x: f32, y: f32) {
        let inside =
            |r: tiny_skia::Rect| x >= r.left() && x <= r.right() && y >= r.top() && y <= r.bottom();
        let choice = self.hits.choices.iter().position(|r| inside(*r));
        let on_bubble = self.hits.bubble.is_some_and(inside);
        if self.hits.clear.is_some_and(inside) {
            self.clear_all();
            return;
        }
        if self.hits.snooze.is_some_and(inside) {
            self.snooze();
            return;
        }
        if self.hits.reply.is_some_and(inside) {
            if let Some(s) = &self.shown {
                let (id, title, body) = (s.note.id, s.note.title.clone(), s.note.body.clone());
                p.open_reply(id, &title, &body);
            }
            return;
        }
        if let Some(i) = choice {
            self.answer(i);
        } else if on_bubble {
            // A question is only closed by answering it.
            if self.shown.as_ref().is_some_and(|s| s.note.is_question()) {
                return;
            }
            // A double click must not close the next bubble before it can be read.
            if self
                .shown
                .as_ref()
                .is_some_and(|s| s.since.elapsed() < CLICK_GUARD)
            {
                return;
            }
            if let Some(s) = self.shown.take() {
                if let Some(url) = &s.note.url {
                    p.open_url(url);
                    self.update_history(s.note.id, |h| h.outcome = "opened link".into());
                } else {
                    self.update_history(s.note.id, |h| h.outcome = "dismissed".into());
                }
                self.seen = self.waiting() > 0;
            }
        } else {
            if self.in_silence() {
                return;
            }
            self.dog.pet();
            if self.settings.sound && self.settings.sound_on_pet {
                self.voice(p, Voice::Bark);
            }
        }
    }

    /// Notices the user leaving (idle for [`AWAY`]) and coming back; on return, if news came
    /// meanwhile, shows one bubble that sums it up. Idle time never holds notifications back.
    fn welcome_back(&mut self, idle: Option<u64>) {
        let Some(idle) = idle else { return };
        if idle >= AWAY.as_secs() {
            // Idle started `idle` seconds ago: what came since then counts.
            self.away_since
                .get_or_insert(watch::now_unix().saturating_sub(idle));
            return;
        }
        let Some(since) = self.away_since.take() else {
            return;
        };
        let after = std::mem::replace(&mut self.summed, self.next_id);
        let Some(summary) = self.summary_since(since, after) else {
            return;
        };
        let n = Notification::new("While you were away", summary).sticky();
        self.push(n);
        // It goes first, ahead of what it sums up.
        if let Some(i) = self
            .queue
            .iter()
            .position(|n| n.title == "While you were away")
            && let Some(n) = self.queue.remove(i)
        {
            self.queue.push_front(n);
        }
    }

    /// "3 notifications, 1 failed · CI, OpenCode" for what arrived since `since` (Unix
    /// seconds) with an id above `after`.
    fn summary_since(&self, since: u64, after: u64) -> Option<String> {
        let s = self.status.lock().ok()?;
        let news: Vec<&HistoryEntry> = s
            .history
            .iter()
            .filter(|h| h.id > after && h.at >= since && h.title != "While you were away")
            .collect();
        if news.len() < 2 {
            // One item is shown (or in History) by itself already.
            return None;
        }
        let low = |h: &HistoryEntry| format!("{} {}", h.title, h.outcome).to_lowercase();
        let failed = news
            .iter()
            .filter(|h| {
                let t = low(h);
                t.contains("fail") || t.contains("error")
            })
            .count();
        let asked = news
            .iter()
            .filter(|h| h.outcome.starts_with("expired") || h.outcome == "answered elsewhere")
            .count();
        let mut parts = vec![format!("{} notifications", news.len())];
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        if asked > 0 {
            parts.push(format!("{asked} unanswered"));
        }
        let mut sources: Vec<&str> = news.iter().filter_map(|h| h.source.as_deref()).collect();
        sources.sort_unstable();
        sources.dedup();
        let mut text = parts.join(", ");
        if !sources.is_empty() {
            text.push_str(&format!(" · {}", sources.join(", ")));
        }
        let last: Vec<String> = news
            .iter()
            .rev()
            .take(3)
            .map(|h| {
                if h.title.is_empty() {
                    h.message.chars().take(40).collect()
                } else {
                    h.title.chars().take(40).collect()
                }
            })
            .collect();
        text.push_str(&format!("\nLatest: {}", last.join("; ")));
        Some(text)
    }

    /// Looks for a new version at start and once a day (or now, with `now`); installs it when
    /// allowed and announces it.
    /// "Restart" on the "update installed" bubble: starts the new version and quits.
    fn restart_if_asked(&mut self, p: &mut dyn Platform) {
        let Some(rx) = &self.restart_answer else {
            return;
        };
        match rx.try_recv() {
            Ok(a) if a == "Restart" => {
                self.restart_answer = None;
                self.shutdown();
                update::restart();
                p.quit();
            }
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.restart_answer = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn check_update(&mut self, now: bool) {
        if let Some(rx) = &self.update {
            match rx.try_recv() {
                Ok(found) => {
                    self.update = None;
                    self.on_update(found);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.update = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            return;
        }
        let due = self
            .last_update_check
            .is_none_or(|t| t.elapsed() >= UPDATE_EVERY);
        if self.update_ready || !(now || due && self.settings.check_updates) {
            return;
        }
        self.last_update_check = Some(Instant::now());
        let install = self.settings.auto_update && update::can_install();
        self.update = Some(update::check(install));
        self.update_asked = now;
    }

    fn on_update(&mut self, found: update::Found) {
        let asked = std::mem::take(&mut self.update_asked);
        match found {
            update::Found::UpToDate => {
                if asked {
                    self.push(
                        Notification::new(
                            "Dost is up to date",
                            format!("Version {}", env!("CARGO_PKG_VERSION")),
                        )
                        .timeout(Duration::from_secs(5)),
                    );
                }
            }
            update::Found::Failed(e) => {
                if asked {
                    self.push(Notification::new("Could not check for updates", e));
                }
            }
            update::Found::Newer {
                version,
                url,
                ready,
            } => {
                let first =
                    self.new_version.as_deref() != Some(&version) || ready != self.update_ready;
                self.new_version = Some(version.clone());
                self.update_ready = ready;
                self.panel_dirty = true;
                if !(first || asked) {
                    return;
                }
                if ready {
                    let (mut n, rx) = Notification::new(
                        format!("Dost {version} is installed"),
                        "Restart to use it.",
                    )
                    .ask(
                        vec!["Restart".into(), "Later".into()],
                        Duration::from_secs(3600),
                    );
                    n.timeout = notify::Timeout::Sticky;
                    self.restart_answer = Some(rx);
                    self.push(n);
                } else {
                    let mut n = Notification::new(
                        format!("Dost {version} is out"),
                        "Click to open the download page.",
                    );
                    n.url = Some(url).filter(|u| u.starts_with("https://"));
                    self.push(n.timeout(Duration::from_secs(30)));
                }
            }
        }
    }

    /// Collects what arrives while the screen is locked; on unlock, posts it as a system
    /// notification too, so it is seen even without looking at the dog.
    fn check_lock(&mut self, p: &mut dyn Platform) {
        let locked = p.screen_locked();
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if locked {
            self.while_locked.clear();
            return;
        }
        let news = std::mem::take(&mut self.while_locked);
        match news.as_slice() {
            [] => {}
            [(title, body)] => p.system_notify(title, body),
            [.., (title, _)] => {
                let rest = news.len() - 1;
                p.system_notify(
                    &format!("Dost: {} waiting", news.len()),
                    &format!("{title} and {rest} more. Click the dog's icon to see them."),
                );
            }
        }
    }

    /// Remembers an important notification that arrived while the screen was locked.
    fn note_locked(&mut self, n: &Notification) {
        if !self.locked || !(n.is_important() || n.title.to_lowercase().contains("fail")) {
            return;
        }
        let title = match &n.source {
            Some(s) if !n.title.is_empty() => format!("{s}: {}", n.title),
            Some(s) => s.clone(),
            None if n.title.is_empty() => "Dost".into(),
            None => n.title.clone(),
        };
        self.while_locked
            .push((title, n.body.chars().take(200).collect()));
    }

    /// A global keyboard shortcut. Returns true if it did something.
    pub fn on_key(&mut self, p: &mut dyn Platform, key: Key) -> bool {
        let Some(s) = self.shown.as_ref() else {
            return false;
        };
        match key {
            // The first (highlighted) choice; on plain news, opens its link or closes it.
            Key::Accept if s.note.is_question() => self.answer(0),
            Key::Accept => {
                let s = self.shown.take().unwrap();
                match &s.note.url {
                    Some(url) => {
                        p.open_url(url);
                        self.update_history(s.note.id, |h| h.outcome = "opened link".into());
                    }
                    None => self.update_history(s.note.id, |h| h.outcome = "dismissed".into()),
                }
                self.seen = self.waiting() > 0;
            }
            Key::Choice(i) if i < s.note.choices.len() => self.answer(i),
            Key::Choice(_) => return false,
            // A question's last choice is the "no" (Reject, Deny, No).
            Key::Dismiss if s.note.is_question() => {
                let last = s.note.choices.len().saturating_sub(1);
                self.answer(last);
            }
            Key::Dismiss => {
                let s = self.shown.take().unwrap();
                self.update_history(s.note.id, |h| h.outcome = "dismissed".into());
                self.seen = self.waiting() > 0;
            }
            Key::Later => {
                if !s.note.can_snooze() {
                    return false;
                }
                self.snooze();
            }
        }
        self.panel_dirty = true;
        true
    }

    /// Answers the shown question with choice `i`.
    fn answer(&mut self, i: usize) {
        let Some(s) = self.shown.take() else { return };
        let answer = s.note.choices.get(i).cloned().unwrap_or_default();
        self.phone_closed(s.note.id, &format!("answered: {answer}"));
        self.send_answer(&s.note, &answer, "answered");
        self.seen = self.waiting() > 0;
    }

    /// The user typed an answer to question `id` (the "Reply…" field).
    pub fn answer_text(&mut self, id: u64, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let shown = self.shown.as_ref().is_some_and(|s| s.note.id == id);
        let note = if shown {
            self.shown.take().map(|s| s.note)
        } else {
            self.queue
                .iter()
                .position(|n| n.id == id)
                .and_then(|i| self.queue.remove(i))
        };
        let Some(note) = note else { return };
        self.phone_closed(id, &format!("answered: {text}"));
        self.send_answer(&note, text, "answered");
        if shown {
            self.seen = self.waiting() > 0;
        }
        self.panel_dirty = true;
    }

    /// "Later": the bubble goes away and comes back after [`SNOOZE`], silently.
    pub fn snooze(&mut self) {
        let Some(s) = self.shown.take_if(|s| s.note.can_snooze()) else {
            return;
        };
        let mut note = s.note;
        note.not_before = Some(Instant::now() + SNOOZE);
        // It comes back as a reminder: barks again and stays until clicked.
        note.timeout = notify::Timeout::Sticky;
        note.announced = false;
        self.update_history(note.id, |h| h.outcome = "later".into());
        self.queue.push_back(note);
        self.seen = self.waiting() > 0;
    }

    /// "Clear all": closes the shown bubble and everything waiting, except questions (an
    /// agent waits for their answer) and running progress bars. Those stay, without barking.
    fn clear_all(&mut self) {
        // Snoozed ones stay too: the user asked for them later.
        let keep = |n: &Notification| {
            n.not_before.is_some()
                || n.is_question()
                || n.progress
                    .as_ref()
                    .is_some_and(|p| p.state == ProgressState::Running)
        };
        let mut gone = Vec::new();
        if let Some(s) = self.shown.take() {
            if keep(&s.note) {
                let mut note = s.note;
                note.announced = true;
                self.queue.push_front(note);
            } else {
                gone.push(s.note.id);
            }
        }
        self.queue.retain(|n| {
            let k = keep(n);
            if !k {
                gone.push(n.id);
            }
            k
        });
        for n in self.queue.iter_mut() {
            n.announced = true;
        }
        for id in gone {
            self.update_history(id, |h| h.outcome = "dismissed".into());
        }
    }

    pub fn apply_settings(&mut self, s: Settings, save: bool) {
        self.dog.speed = s.speed;
        self.dog.wander = s.wander;
        let addr_changed = self
            .mcp_server
            .as_ref()
            .is_some_and(|r| r.addr != s.mcp_addr);
        if !s.mcp_enabled || addr_changed {
            self.mcp_server = None;
        }
        if s.mcp_enabled && self.mcp_server.is_none() {
            match mcp::serve(&s.mcp_addr, Arc::clone(&self.mcp)) {
                Ok(r) => {
                    self.mcp_server = Some(r);
                    self.mcp_error = None;
                }
                Err(e) => self.mcp_error = Some(e),
            }
        }
        if !s.mcp_enabled {
            self.mcp_error = None;
        }
        // Edited in the file by hand (or from another copy of the settings).
        if s.hidden != self.settings.hidden && s.hidden == self.visible {
            self.toggle_visible = true;
        }
        if s.asleep != self.settings.asleep && s.asleep != self.dog.is_put_to_sleep() {
            if s.asleep {
                self.dog.sleep();
            } else {
                self.dog.wake_up();
            }
        }
        if save {
            self.store.save(&s);
        }
        if s.bark_files != self.sound_files {
            self.sounds = Sounds::new(&s.bark_files);
            self.sound_files = s.bark_files.clone();
            let bad = Sounds::bad_files(&s.bark_files);
            if !bad.is_empty() {
                self.push(Notification::new(
                    "Could not use a bark sound",
                    format!("{} (needs a 16-bit PCM WAV file).", bad.join(", ")),
                ));
            }
        }
        self.mcp.set_name(&s.name);
        self.mcp.set_cors(&s.mcp_cors);
        self.mcp.local.configure(&s.local_mcp);
        if self.phone_cfg.as_ref() != Some(&s.phone) {
            self.remote.configure(&s.phone);
            self.phone_cfg = Some(s.phone.clone());
            self.phone_status = None;
        }
        i18n::set(s.language);
        self.settings = s;
        self.apply_look();
        self.panel_dirty = true;
    }

    pub fn on_action(&mut self, p: &mut dyn Platform, action: Action) {
        match action {
            Action::Settings(s) => {
                self.apply_settings(*s, true);
                if let Some(e) = self.mcp_error.clone() {
                    self.push(Notification::new("MCP server could not start", e).sticky());
                }
            }
            Action::Volume { volume, done } => {
                if self.settings.volume != volume.min(100) {
                    // Saved once the drag ends.
                    self.settings.volume = volume.min(100);
                }
                if done {
                    let s = self.settings.clone();
                    self.apply_settings(s, true);
                    if self.settings.sound {
                        self.voice(p, Voice::Bark);
                    }
                }
            }
            Action::TestNotification => self.push(Notification::new(
                "Test",
                "This is a test notification. Click the bubble to close it.",
            )),
            Action::Fetch => {
                // The bone was put away: the button brings it back.
                if !self.settings.toy {
                    let s = Settings {
                        toy: true,
                        ..self.settings.clone()
                    };
                    self.apply_settings(s, true);
                }
                self.reactions.push(Reaction::Fetch);
            }
            // The shells handle it (they know where the mouse is); without one, bring it out.
            Action::TakeToy => {
                let (x, y) = (self.dog.x, self.dog.y - 80.0);
                self.take_toy(x, y);
                self.toy.release(1.0);
            }
            Action::ToggleSleep => self.dog.toggle_sleep(),
            Action::ToggleVisible => {
                self.toggle_visible = true;
            }
            Action::ToggleAutostart => {
                let on = !p.autostart_enabled();
                if let Err(e) = p.set_autostart(on) {
                    self.push(Notification::new("Could not change autostart", e));
                }
            }
            Action::CopyMcpUrl => {
                let url = self.mcp_url();
                let msg = if p.copy_text(&url) {
                    format!("{url} copied to the clipboard.")
                } else {
                    url
                };
                self.push(Notification::new("MCP address", msg).timeout(Duration::from_secs(5)));
            }
            Action::OpenConfigDir => {
                if let Some(path) = self.store.path().map(|p| p.to_path_buf()) {
                    if !path.exists() {
                        self.store.save(&self.settings);
                    }
                    if let Some(dir) = path.parent() {
                        p.open_folder(dir);
                    }
                }
            }
            Action::Quit => {
                p.close_panel();
                p.quit();
            }
            Action::Close => p.close_panel(),
            Action::OpenSettings => {
                p.close_panel();
                p.open_settings_window();
            }
            Action::Reply { id, text } => self.answer_text(id, &text),
            Action::PhoneTest => {
                self.phone_status = None;
                self.remote.test();
            }
            Action::NewTopic => {
                let mut s = self.settings.clone();
                s.phone.ntfy.topic = crate::remote::random_topic();
                self.apply_settings(s, true);
            }
            Action::CopyText(text) => {
                if p.copy_text(&text) {
                    self.push(
                        Notification::new(t("Copy"), text)
                            .local()
                            .timeout(Duration::from_secs(4)),
                    );
                }
            }
            Action::OpenUrl(url) => p.open_url(&url),
            Action::ClearSessions => self.sessions.clear_finished(),
            Action::Cron(edit) => {
                use crate::cron::NotifyWhen;
                use panel::CronEdit;
                let cron = &self.mcp.cron;
                match edit {
                    CronEdit::Add {
                        schedule,
                        action,
                        always,
                    } => {
                        let added = cron.add(crate::cron::NewJob {
                            name: String::new(),
                            schedule,
                            action,
                            notify: if always {
                                NotifyWhen::Always
                            } else {
                                NotifyWhen::Failure
                            },
                            source: None,
                        });
                        if let Err(e) = added {
                            self.push(Notification::new(t("Schedule"), e).local());
                        }
                    }
                    CronEdit::Remove(id) => {
                        cron.remove(id);
                    }
                    CronEdit::Enable(id, on) => {
                        cron.set_enabled(id, on);
                    }
                    CronEdit::Run(id) => {
                        cron.run_now(id);
                    }
                }
                self.panel_dirty = true;
            }
            Action::RemoveSession(id) => {
                self.sessions.remove(&id);
            }
            Action::Todo(edit) => self.mcp.todo_edit(&edit),
            Action::CheckUpdate => {
                if self.update_ready {
                    self.shutdown();
                    update::restart();
                    p.close_panel();
                    p.quit();
                } else if let (Some(v), false) = (&self.new_version, update::can_install()) {
                    p.open_url(&format!("https://github.com/rytsh/dost/releases/tag/v{v}"));
                } else {
                    self.last_update_check = None;
                    self.check_update(true);
                }
            }
        }
        self.panel_dirty = true;
    }

    pub fn toggle_mute(&mut self, p: &mut dyn Platform) {
        let mut s = self.settings.clone();
        s.sound = !s.sound;
        self.on_action(p, Action::Settings(Box::new(s)));
    }

    pub fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.settings.mcp_addr)
    }

    pub fn panel_view(&self, p: &dyn Platform) -> panel::View {
        panel::View {
            settings: self.settings.clone(),
            age: self.age_text(),
            outfit: self.renderer.outfit(),
            sleeping: self.dog.is_put_to_sleep(),
            visible: self.visible,
            autostart: p.autostart_enabled(),
            autostart_label: p.autostart_label(),
            mcp_ok: self.mcp_server.is_some() || !self.settings.mcp_enabled,
            mcp_status: match (&self.mcp_server, &self.mcp_error) {
                (Some(r), _) => format!("http://{}/mcp", r.addr),
                (None, Some(e)) => e.clone(),
                (None, None) => "Off".into(),
            },
            version: env!("CARGO_PKG_VERSION"),
            update: self.new_version.clone().map(|v| (v, self.update_ready)),
            history: self.history_items(),
            quiet: self.settings.is_quiet(local_minute()) || self.fullscreen,
            missed: self.missed,
            todos: self.mcp.todo_lists(),
            jobs: self
                .mcp
                .cron
                .list()
                .into_iter()
                .map(|j| {
                    let next = self.mcp.cron.next_run(&j);
                    (j, next)
                })
                .collect(),
            sessions: self.sessions.list(),
            phone_status: self.phone_status.clone(),
            screens: p.screen_count(),
            special: self
                .settings
                .special_days
                .then(|| special::Day::on(look::today()))
                .flatten(),
            silence: self.in_silence(),
        }
    }

    fn history_items(&self) -> Vec<panel::HistoryItem> {
        use chrono::TimeZone;
        let Ok(s) = self.status.lock() else {
            return Vec::new();
        };
        s.history
            .iter()
            .rev()
            .take(5)
            .map(|h| panel::HistoryItem {
                time: chrono::Local
                    .timestamp_opt(h.at as i64, 0)
                    .single()
                    .map(|t| t.format("%H:%M").to_string())
                    .unwrap_or_default(),
                source: h.source.clone(),
                title: if h.title.is_empty() {
                    h.message.clone()
                } else {
                    h.title.clone()
                },
                outcome: h.outcome.clone(),
            })
            .collect()
    }

    fn publish_status(&self, p: &dyn Platform) {
        let activity = match self.dog.frame().anim {
            _ if self.in_silence() => "standing in silence (10 November, 09:05)",
            _ if self.dog.is_put_to_sleep() => "sleeping",
            _ if self.dog.is_away() => "away (hidden)",
            _ if self.dog.is_fetching() => "playing fetch",
            Anim::Idle => "standing",
            Anim::Walk if self.dog.is_performing() => "doing zoomies",
            Anim::Walk => "walking",
            Anim::Sit => "sitting",
            Anim::Bark => "barking",
            Anim::Happy => "happy",
            Anim::Sleep => "napping",
            Anim::Held => "being carried",
            Anim::Sad => "sad",
        };
        let idle = p.idle_secs();
        let quiet = self.hushed();
        if let Ok(mut s) = self.status.lock() {
            s.activity = activity.into();
            s.age = self.age_text();
            s.visible = self.visible;
            s.showing = self
                .shown
                .as_ref()
                .map(|s| format!("{}: {}", s.note.title, s.note.body));
            s.queued = self.waiting();
            s.idle_secs = idle;
            s.quiet = quiet;
        }
    }
}

/// The bubble for what is shown. A free function so shells can borrow `shown` while drawing
/// with `renderer` (another field of [`Core`]).
pub fn bubble_of(shown: Option<&Shown>, pending: usize) -> Option<Bubble<'_>> {
    shown.map(|s| Bubble {
        title: &s.note.title,
        body: &s.note.body,
        pending,
        source: s.note.source.as_deref(),
        choices: &s.note.choices,
        progress: s.note.progress.as_ref().map(|p| (p.value, p.state)),
        link: s.note.url.is_some(),
        snooze: s.note.can_snooze(),
        reply: s.note.allow_text,
        time: s.since.elapsed().as_secs_f32(),
    })
}

/// Minutes since local midnight.
fn local_minute() -> u32 {
    use chrono::Timelike;
    let t = chrono::Local::now();
    t.hour() * 60 + t.minute()
}

/// How long since `birthday`, in words: "2 years", "1 year", "5 months", "3 days".
pub fn age_text(birthday: Option<chrono::NaiveDate>, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let Some(b) = birthday.filter(|b| *b <= today) else {
        return "new here".into();
    };
    let mut months = (today.year() - b.year()) * 12 + today.month() as i32 - b.month() as i32;
    if today.day() < b.day() {
        months -= 1;
    }
    let plural = |n: i64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    if months >= 12 {
        plural((months / 12) as i64, "year")
    } else if months >= 1 {
        plural(months as i64, "month")
    } else {
        plural((today - b).num_days(), "day")
    }
}

/// Where the dog stands: the base line (taskbar top / Dock top, or the screen's bottom edge)
/// minus the `lift` setting. All values in the platform's y-down physical pixels.
/// The dog stays at least one dog height below the top and never below the screen.
pub fn ground_line(
    s: &Settings,
    work_bottom: f32,
    screen_top: f32,
    screen_bottom: f32,
    ui: f32,
    px: f32,
) -> f32 {
    let base = match s.ground {
        settings::Ground::Taskbar => work_bottom,
        settings::Ground::Screen => screen_bottom,
    };
    let highest = (screen_top + 150.0 * px).min(base);
    (base - (s.lift as f32 * ui).round()).clamp(highest, screen_bottom)
}

/// How far from the screen's sides the dog walks (sprite pixels).
pub const EDGE: f32 = 80.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Progress;

    #[derive(Default)]
    struct Fake {
        barks: usize,
        copied: Vec<String>,
        opened: Vec<String>,
        window: Option<bool>,
        /// Overrides the idle time (default 7 s).
        idle: Option<u64>,
        locked: bool,
        system: Vec<(String, String)>,
    }

    impl Platform for Fake {
        fn play_wav(&mut self, _: Vec<u8>) {
            self.barks += 1;
        }
        fn copy_text(&mut self, text: &str) -> bool {
            self.copied.push(text.into());
            true
        }
        fn autostart_enabled(&self) -> bool {
            false
        }
        fn set_autostart(&mut self, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn open_folder(&mut self, _: &std::path::Path) {}
        fn open_url(&mut self, url: &str) {
            self.opened.push(url.into());
        }
        fn idle_secs(&self) -> Option<u64> {
            Some(self.idle.unwrap_or(7))
        }
        fn screen_locked(&self) -> bool {
            self.locked
        }
        fn system_notify(&mut self, title: &str, body: &str) {
            self.system.push((title.into(), body.into()));
        }
        fn set_dog_visible(&mut self, v: bool) {
            self.window = Some(v);
        }
        fn close_panel(&mut self) {}
        fn quit(&mut self) {}
        fn autostart_label(&self) -> &'static str {
            "Open at login"
        }
    }

    const B: Bounds = Bounds {
        ground: 1000.0,
        left: 100.0,
        right: 1800.0,
        scale: 1.0,
    };

    fn core() -> Core {
        let mut c = Core::build(
            Renderer::new(false, Vec::new()),
            900.0,
            B.ground,
            Store::new(None),
            None,
        );
        c.settings.quiet_from = "00:00".into();
        c.settings.quiet_to = "00:00".into();
        c.settings.check_updates = false;
        c
    }

    fn step(c: &mut Core, p: &mut Fake) {
        c.tick(p, &B);
    }

    /// Ages the bubble on screen past [`CLICK_GUARD`].
    fn settle(c: &mut Core) {
        if let Some(s) = c.shown.as_mut() {
            s.since -= CLICK_GUARD;
        }
    }

    /// Clicks the middle of a rect from the last frame's hits.
    fn click_at(c: &mut Core, p: &mut Fake, r: tiny_skia::Rect) {
        c.click(
            p,
            (r.left() + r.right()) / 2.0,
            (r.top() + r.bottom()) / 2.0,
        );
    }

    fn history(c: &Core) -> Vec<(String, String)> {
        let s = c.status.lock().unwrap();
        s.history
            .iter()
            .map(|h| (h.title.clone(), h.outcome.clone()))
            .collect()
    }

    fn progress(id: &str, value: Option<f32>, state: ProgressState) -> Notification {
        let mut n = Notification::new(id, "").sticky();
        n.progress = Some(Progress {
            id: id.into(),
            value,
            state,
        });
        n
    }

    #[test]
    fn question_is_answered_by_its_button_only() {
        let (mut c, mut p) = (core(), Fake::default());
        let (n, rx) = Notification::new("", "Deploy?")
            .ask(vec!["Yes".into(), "No".into()], Duration::from_secs(60));
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert_eq!(p.barks, 1);
        let b = c.bubble().unwrap();
        assert_eq!(b.choices, ["Yes", "No"]);
        // Fake the drawn frame's hit areas.
        c.hits = Hits {
            bubble: tiny_skia::Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
            choices: vec![
                tiny_skia::Rect::from_xywh(10.0, 60.0, 50.0, 26.0).unwrap(),
                tiny_skia::Rect::from_xywh(70.0, 60.0, 50.0, 26.0).unwrap(),
            ],
            ..Default::default()
        };
        // Clicking the text does not close a question.
        c.click(&mut p, 20.0, 20.0);
        assert!(c.shown.is_some());
        let no = c.hits.choices[1];
        click_at(&mut c, &mut p, no);
        assert!(c.shown.is_none());
        assert_eq!(rx.try_recv().unwrap(), "No");
        assert_eq!(history(&c), [("".into(), "answered: No".into())]);
    }

    #[test]
    fn next_after_click_does_not_bark() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier.send(Notification::new("A", "first"));
        c.notifier.send(Notification::new("B", "second"));
        step(&mut c, &mut p);
        assert_eq!(p.barks, 1);
        c.hits = Hits {
            bubble: tiny_skia::Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
            choices: Vec::new(),
            ..Default::default()
        };
        settle(&mut c);
        c.click(&mut p, 20.0, 20.0);
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().title, "B");
        c.click(&mut p, 20.0, 20.0);
        assert_eq!(
            c.bubble().unwrap().title,
            "B",
            "a double click does not skip it"
        );
        assert_eq!(p.barks, 1, "the user is already looking");
    }

    #[test]
    fn clear_all_keeps_questions() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier.send(Notification::new("A", "first"));
        let (q, _rx) =
            Notification::new("Q", "Deploy?").ask(vec!["Yes".into()], Duration::from_secs(60));
        c.notifier.send(q);
        c.notifier.send(Notification::new("B", "second"));
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().pending, 2);
        c.hits = Hits {
            clear: tiny_skia::Rect::from_xywh(0.0, 80.0, 60.0, 20.0),
            ..Default::default()
        };
        c.click(&mut p, 10.0, 90.0);
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().title, "Q");
        assert!(c.queue.is_empty());
        assert_eq!(p.barks, 1, "the question is not announced again");
        let h = history(&c);
        assert_eq!(h[0], ("A".into(), "dismissed".into()));
        assert_eq!(h[2], ("B".into(), "dismissed".into()));
    }

    #[test]
    fn question_withdrawn_by_key() {
        let (mut c, mut p) = (core(), Fake::default());
        let (mut n, rx) =
            Notification::new("", "Allow edit?").ask(vec!["Allow".into()], Duration::from_secs(60));
        n.key = Some("per_1".into());
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert!(c.shown.is_some());
        c.notifier.dismiss("per_1".into());
        step(&mut c, &mut p);
        assert!(c.shown.is_none());
        assert!(rx.try_recv().is_err(), "sender dropped");
        assert_eq!(history(&c)[0].1, "answered elsewhere");
    }

    #[test]
    fn unanswered_question_expires() {
        let (mut c, mut p) = (core(), Fake::default());
        let (n, rx) =
            Notification::new("", "Still there?").ask(vec!["Yes".into()], Duration::from_millis(1));
        c.notifier.send(n);
        std::thread::sleep(Duration::from_millis(5));
        step(&mut c, &mut p);
        assert!(c.shown.is_none());
        assert!(rx.try_recv().is_err());
        assert_eq!(history(&c)[0].1, "expired (no answer)");
    }

    #[test]
    fn progress_updates_in_place_and_barks_once() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier
            .send(progress("build", Some(0.1), ProgressState::Running));
        step(&mut c, &mut p);
        for v in [0.4, 0.8] {
            c.notifier
                .send(progress("build", Some(v), ProgressState::Running));
            step(&mut c, &mut p);
        }
        assert_eq!(p.barks, 1);
        assert!(c.queue.is_empty());
        let shown = c.shown.as_ref().unwrap();
        assert_eq!(shown.note.progress.as_ref().unwrap().value, Some(0.8));
        assert_eq!(shown.deadline, None);

        c.notifier
            .send(progress("build", None, ProgressState::Done));
        step(&mut c, &mut p);
        let shown = c.shown.as_ref().unwrap();
        assert_eq!(
            shown.note.progress.as_ref().unwrap().state,
            ProgressState::Done
        );
        assert!(shown.deadline.is_some());
        assert_eq!(history(&c), [("build".into(), "finished".into())]);
    }

    #[test]
    fn running_progress_yields_to_others() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier
            .send(progress("tests", Some(0.2), ProgressState::Running));
        step(&mut c, &mut p);
        c.notifier.send(Notification::new("Hello", ""));
        step(&mut c, &mut p);
        assert_eq!(c.shown.as_ref().unwrap().note.title, "tests");
        c.shown.as_mut().unwrap().since -= PROGRESS_TURN;
        step(&mut c, &mut p);
        assert_eq!(c.shown.as_ref().unwrap().note.title, "Hello");
        // It comes back after, without barking again.
        let barks = p.barks;
        c.shown = None;
        step(&mut c, &mut p);
        assert_eq!(c.shown.as_ref().unwrap().note.title, "tests");
        assert_eq!(p.barks, barks);
    }

    #[test]
    fn quiet_mode_skips_plain_notifications() {
        let (mut c, mut p) = (core(), Fake::default());
        c.settings.quiet = true;
        c.notifier.send(Notification::new("FYI", ""));
        c.notifier.send(Notification::new("Important", "").sticky());
        step(&mut c, &mut p);
        assert_eq!(c.shown.as_ref().unwrap().note.title, "Important");
        assert_eq!(p.barks, 0);
        assert_eq!(history(&c)[0], ("FYI".into(), "skipped (quiet)".into()));
    }

    #[test]
    fn link_copy_and_idle() {
        let (mut c, mut p) = (core(), Fake::default());
        let mut n = Notification::new("PR ready", "");
        n.url = Some("https://example.com".into());
        n.copy = Some("git pull".into());
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert_eq!(p.copied, ["git pull"]);
        assert!(c.bubble().unwrap().link);
        c.hits.bubble = tiny_skia::Rect::from_xywh(0.0, 0.0, 100.0, 50.0);
        settle(&mut c);
        c.click(&mut p, 10.0, 10.0);
        assert_eq!(p.opened, ["https://example.com"]);
        assert_eq!(c.status.lock().unwrap().idle_secs, Some(7));
    }

    /// Runs frames until `done` or `secs` pass.
    fn run(c: &mut Core, p: &mut Fake, secs: f32, done: impl Fn(&Core) -> bool) {
        for _ in 0..(secs * 60.0) as usize {
            c.last -= Duration::from_millis(16);
            step(c, p);
            if done(c) {
                return;
            }
        }
    }

    fn hidden(c: &mut Core, p: &mut Fake) {
        c.on_action(p, Action::ToggleVisible);
        run(c, p, 30.0, |c| c.dog.is_away());
        assert!(c.dog.is_away());
        assert_eq!(p.window, Some(false));
    }

    #[test]
    fn hidden_dog_barks_and_counts_missed() {
        let (mut c, mut p) = (core(), Fake::default());
        hidden(&mut c, &mut p);
        let barks = p.barks;
        let mut n = Notification::new("Done", "Build ok");
        n.copy = Some("make run".into());
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert!(c.shown.is_none(), "no invisible bubble");
        assert!(c.dog.is_away(), "the dog stays away for plain news");
        assert_eq!(p.barks, barks + 1);
        assert_eq!(p.copied, ["make run"]);
        assert_eq!(c.missed, 1);
        assert_eq!(history(&c)[0].1, "missed (hidden)");
        assert!(c.panel_opened());
        assert_eq!(c.missed, 0);
        assert!(!c.panel_opened());
    }

    #[test]
    fn hidden_question_shows_a_bubble_alone() {
        let (mut c, mut p) = (core(), Fake::default());
        hidden(&mut c, &mut p);
        let (n, rx) =
            Notification::new("", "Deploy?").ask(vec!["Yes".into()], Duration::from_secs(60));
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert!(c.shown.is_some(), "shown right away");
        assert!(c.bubble_alone(), "on its own, without the dog");
        assert!(c.dog.is_away(), "the dog does not come back");
        assert_eq!(p.window, Some(false));
        assert!(!c.visible);
        c.hits = Hits {
            bubble: tiny_skia::Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
            choices: vec![tiny_skia::Rect::from_xywh(10.0, 60.0, 50.0, 26.0).unwrap()],
            ..Default::default()
        };
        c.click(&mut p, 30.0, 70.0);
        assert_eq!(rx.try_recv().unwrap(), "Yes");
        assert!(!c.bubble_alone());
    }

    #[test]
    fn bubble_waits_while_the_dog_walks_off() {
        let (mut c, mut p) = (core(), Fake::default());
        c.on_action(&mut p, Action::ToggleVisible);
        step(&mut c, &mut p);
        assert!(!c.dog.is_away(), "still walking off");
        c.notifier.send(Notification::new("Important", "").sticky());
        step(&mut c, &mut p);
        assert!(c.shown.is_none(), "no bubble over a dog that is leaving");
        run(&mut c, &mut p, 30.0, |c| c.shown.is_some());
        assert!(c.dog.is_away());
        assert!(c.bubble_alone());
    }

    #[test]
    fn show_brings_the_dog_back() {
        let (mut c, mut p) = (core(), Fake::default());
        hidden(&mut c, &mut p);
        c.on_action(&mut p, Action::ToggleVisible);
        step(&mut c, &mut p);
        assert_eq!(p.window, Some(true));
        assert!(c.visible);
        run(&mut c, &mut p, 30.0, |c| !c.dog.is_performing());
        assert!(c.dog.x <= B.right);
        // Plain notifications show normally again.
        c.notifier.send(Notification::new("Hi", ""));
        step(&mut c, &mut p);
        assert!(c.shown.is_some());
        assert_eq!(c.missed, 0);
    }

    #[test]
    fn reactions() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier.react(Reaction::Zoomies);
        step(&mut c, &mut p);
        assert!(c.dog.is_performing());
        c.notifier.react(Reaction::Sleep);
        step(&mut c, &mut p);
        assert!(c.dog.is_sleeping());
        c.notifier.react(Reaction::Wake);
        step(&mut c, &mut p);
        assert!(!c.dog.is_sleeping());
    }

    #[test]
    fn hidden_and_asleep_are_remembered() {
        let dir = std::env::temp_dir().join(format!("dost-remember-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_file(&path);
        let make = || {
            let mut store = Store::new(Some(path.clone()));
            let s = store.load();
            let mut c = Core::build(
                Renderer::new(false, Vec::new()),
                900.0,
                B.ground,
                store,
                None,
            );
            c.restore(&s);
            c.apply_settings(s, false);
            c
        };
        let (mut c, mut p) = (make(), Fake::default());
        hidden(&mut c, &mut p);
        c.on_action(&mut p, Action::ToggleSleep);
        step(&mut c, &mut p);

        let (mut c, mut p) = (make(), Fake::default());
        assert!(!c.visible);
        assert!(c.dog.is_away(), "starts off screen, no walking off");
        assert!(c.dog.is_put_to_sleep());
        step(&mut c, &mut p);
        assert!(!c.on_screen());
        assert_eq!(p.window, None, "the dog window is never shown");

        c.on_action(&mut p, Action::ToggleVisible);
        c.on_action(&mut p, Action::ToggleSleep);
        step(&mut c, &mut p);
        let c = make();
        assert!(c.visible && !c.dog.is_away() && !c.dog.is_put_to_sleep());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("dost-history-{}", std::process::id()));
        let path = dir.join("history.json");
        let _ = std::fs::remove_file(&path);
        let (mut c, mut p) = (core(), Fake::default());
        c.load_history(Some(path.clone()));
        c.notifier.send(Notification::new("Built", "ok"));
        c.notifier.send(Notification::new("Waiting", ""));
        step(&mut c, &mut p);
        c.shutdown();

        let mut c = core();
        c.load_history(Some(path.clone()));
        let h = history(&c);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0], ("Built".into(), "closed".into()));
        assert_eq!(h[1], ("Waiting".into(), "closed".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn later_brings_it_back() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier.send(Notification::new("Deploy", "done"));
        c.notifier.send(Notification::new("Next", ""));
        step(&mut c, &mut p);
        assert!(c.bubble().unwrap().snooze);
        assert_eq!(c.bubble().unwrap().pending, 1);
        c.hits = Hits {
            snooze: tiny_skia::Rect::from_xywh(0.0, 0.0, 50.0, 20.0),
            ..Default::default()
        };
        c.click(&mut p, 10.0, 10.0);
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().title, "Next");
        assert_eq!(
            c.bubble().unwrap().pending,
            0,
            "snoozed ones are not counted"
        );
        c.shown = None;
        step(&mut c, &mut p);
        assert!(c.shown.is_none(), "not yet");
        let barks = p.barks;
        for n in c.queue.iter_mut() {
            n.not_before = Some(Instant::now());
        }
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().title, "Deploy");
        assert_eq!(p.barks, barks + 1, "barks again as a reminder");
        assert!(
            c.shown.as_ref().unwrap().deadline.is_none(),
            "stays until clicked"
        );

        // Questions cannot be put off: an agent is waiting.
        let (n, _rx) =
            Notification::new("", "Allow?").ask(vec!["Yes".into()], Duration::from_secs(60));
        assert!(!n.can_snooze());
    }

    #[test]
    fn keyboard_answers() {
        let (mut c, mut p) = (core(), Fake::default());
        let ask = |c: &mut Core| {
            let (n, rx) = Notification::new("", "Allow?").ask(
                vec!["Allow".into(), "Always".into(), "Reject".into()],
                Duration::from_secs(60),
            );
            c.notifier.send(n);
            rx
        };
        assert!(!c.on_key(&mut p, Key::Accept), "nothing shown");
        let rx = ask(&mut c);
        step(&mut c, &mut p);
        assert!(!c.on_key(&mut p, Key::Later), "questions cannot be put off");
        assert!(c.on_key(&mut p, Key::Accept));
        assert_eq!(rx.try_recv().unwrap(), "Allow");
        let rx = ask(&mut c);
        step(&mut c, &mut p);
        assert!(!c.on_key(&mut p, Key::Choice(3)));
        c.on_key(&mut p, Key::Choice(1));
        assert_eq!(rx.try_recv().unwrap(), "Always");
        let rx = ask(&mut c);
        step(&mut c, &mut p);
        c.on_key(&mut p, Key::Dismiss);
        assert_eq!(rx.try_recv().unwrap(), "Reject");

        let mut n = Notification::new("PR", "");
        n.url = Some("https://example.com".into());
        c.notifier.send(n);
        step(&mut c, &mut p);
        c.on_key(&mut p, Key::Accept);
        assert_eq!(p.opened, ["https://example.com"]);
        c.notifier.send(Notification::new("FYI", ""));
        step(&mut c, &mut p);
        assert!(c.on_key(&mut p, Key::Later));
        assert!(c.shown.is_none());
        assert_eq!(c.queue.len(), 1);
    }

    #[test]
    fn welcome_back_sums_up() {
        let (mut c, mut p) = (core(), Fake::default());
        p.idle = Some(600);
        step(&mut c, &mut p);
        c.notifier
            .send(Notification::new("Build failed", "").source(Some("CI".into())));
        c.notifier
            .send(Notification::new("Tests passed", "").source(Some("OpenCode".into())));
        step(&mut c, &mut p);
        c.shown = None;
        step(&mut c, &mut p);
        c.shown = None;
        p.idle = Some(1);
        step(&mut c, &mut p);
        step(&mut c, &mut p);
        let s = c.shown.as_ref().expect("summary shown");
        assert_eq!(s.note.title, "While you were away");
        assert!(
            s.note
                .body
                .starts_with("2 notifications, 1 failed · CI, OpenCode"),
            "{}",
            s.note.body
        );
        assert!(s.note.body.contains("Latest: Tests passed; Build failed"));

        // Only once, and not for a single item.
        c.shown = None;
        p.idle = Some(600);
        step(&mut c, &mut p);
        c.notifier.send(Notification::new("One", ""));
        step(&mut c, &mut p);
        c.shown = None;
        p.idle = Some(1);
        step(&mut c, &mut p);
        step(&mut c, &mut p);
        assert!(c.shown.is_none());
    }

    #[test]
    fn locked_screen_news_goes_to_the_system() {
        let (mut c, mut p) = (core(), Fake::default());
        let lock_check = |c: &mut Core, p: &mut Fake| {
            c.last_fullscreen_check -= Duration::from_secs(2);
            step(c, p);
        };
        p.locked = true;
        lock_check(&mut c, &mut p);
        c.notifier.send(Notification::new("FYI", "plain"));
        c.notifier.send(
            Notification::new("Deploy?", "prod")
                .source(Some("CI".into()))
                .sticky(),
        );
        step(&mut c, &mut p);
        assert!(p.system.is_empty(), "not while locked");
        p.locked = false;
        lock_check(&mut c, &mut p);
        assert_eq!(p.system, [("CI: Deploy?".into(), "prod".into())]);
        lock_check(&mut c, &mut p);
        assert_eq!(p.system.len(), 1, "once");
    }

    #[test]
    fn update_found_asks_to_restart() {
        let (mut c, mut p) = (core(), Fake::default());
        c.on_update(update::Found::Newer {
            version: "9.0.0".into(),
            url: "https://github.com/rytsh/dost/releases/tag/v9.0.0".into(),
            ready: true,
        });
        step(&mut c, &mut p);
        assert_eq!(c.bubble().unwrap().title, "Dost 9.0.0 is installed");
        assert_eq!(c.bubble().unwrap().choices, ["Restart", "Later"]);
        c.on_key(&mut p, Key::Choice(1));
        step(&mut c, &mut p);
        assert!(c.restart_answer.is_none());
        // The same news is not repeated by the daily check.
        c.on_update(update::Found::Newer {
            version: "9.0.0".into(),
            url: String::new(),
            ready: true,
        });
        step(&mut c, &mut p);
        assert!(c.shown.is_none());

        // Not installed (auto update off): a link to the release.
        let (mut c, mut p) = (core(), Fake::default());
        c.on_update(update::Found::Newer {
            version: "9.0.0".into(),
            url: "https://github.com/rytsh/dost/releases/tag/v9.0.0".into(),
            ready: false,
        });
        step(&mut c, &mut p);
        assert!(c.bubble().unwrap().link);
    }

    #[test]
    fn asleep_is_quiet() {
        let (mut c, mut p) = (core(), Fake::default());
        c.on_action(&mut p, Action::ToggleSleep);
        step(&mut c, &mut p);
        assert!(c.hushed());
        c.notifier.send(Notification::new("FYI", ""));
        c.notifier.send(Notification::new("Important", "").sticky());
        step(&mut c, &mut p);
        assert_eq!(p.barks, 0, "no barking while asleep");
        assert_eq!(c.shown.as_ref().unwrap().note.title, "Important");
        assert!(c.dog.is_sleeping(), "an important bubble does not wake it");
        assert_eq!(history(&c)[0], ("FYI".into(), "skipped (asleep)".into()));
        assert!(c.status.lock().unwrap().quiet);
    }

    #[test]
    fn failures_make_the_dog_sad() {
        let (mut c, mut p) = (core(), Fake::default());
        c.notifier
            .send(Notification::new("Build failed", "3 errors").mood(None));
        step(&mut c, &mut p);
        assert_eq!(c.dog.frame().anim, Anim::Sad);
        assert_eq!(p.barks, 1);
        c.shown = None;
        c.notifier
            .send(Notification::new("Tests passed", "").mood(None));
        step(&mut c, &mut p);
        assert_eq!(c.dog.frame().anim, Anim::Happy);
    }

    #[test]
    fn bone_is_fetched() {
        let (mut c, mut p) = (core(), Fake::default());
        c.on_action(&mut p, Action::Fetch);
        run(&mut c, &mut p, 3.0, |c| c.dog.is_fetching());
        assert!(c.dog.is_fetching(), "the dog runs after it");
        let home = c.dog.x;
        run(&mut c, &mut p, 30.0, |c| c.toy.state == toy::State::Carried);
        assert_eq!(c.toy.state, toy::State::Carried);
        assert!(!c.toy_shown(), "in the mouth, drawn with the dog");
        run(&mut c, &mut p, 30.0, |c| !c.dog.is_fetching());
        assert_eq!(c.dog.x, home, "brought back");
        run(&mut c, &mut p, 3.0, |c| c.toy.state == toy::State::Resting);
        assert!(c.toy_shown());
        assert!(
            (c.toy.x - c.dog.mouth_x()).abs() < 1.0,
            "put down at the mouth"
        );
    }

    #[test]
    fn bone_can_be_put_away_and_dropping_is_not_a_throw() {
        let (mut c, mut p) = (core(), Fake::default());
        // Without wandering it never goes to play with the bone by itself.
        c.settings.wander = false;
        run(&mut c, &mut p, 1.0, |_| false);
        // Picked up and let go in place: the dog does not run after it.
        c.grab_toy();
        let (x, y) = (c.toy.x, c.toy.y - 50.0);
        c.toy.drag_to(x, y);
        c.toy.release(B.scale);
        run(&mut c, &mut p, 3.0, |_| false);
        assert!(!c.dog.is_fetching());
        // Put away: not drawn, and fetch does nothing.
        let s = Settings {
            toy: false,
            ..c.settings.clone()
        };
        c.on_action(&mut p, Action::Settings(Box::new(s)));
        assert!(!c.toy_shown());
        c.reactions.push(Reaction::Fetch);
        run(&mut c, &mut p, 3.0, |_| false);
        assert!(!c.dog.is_fetching());
        // The Bone button brings it back and throws it.
        c.on_action(&mut p, Action::Fetch);
        assert!(c.settings.toy);
        run(&mut c, &mut p, 3.0, |c| c.dog.is_fetching());
        assert!(c.dog.is_fetching());
    }

    #[test]
    fn toy_put_away_by_right_click_and_taken_out_of_the_panel() {
        let (mut c, mut p) = (core(), Fake::default());
        c.put_toy_away();
        assert!(!c.settings.toy && !c.toy_shown());
        // Dragged out of the panel: back, held where the mouse is, thrown when let go.
        c.take_toy(700.0, 500.0);
        assert!(c.settings.toy);
        assert_eq!(c.toy.state, toy::State::Held);
        assert_eq!((c.toy.x, c.toy.y), (700.0, 500.0));
        c.toy.drag_to(760.0, 480.0);
        c.toy.release(B.scale);
        run(&mut c, &mut p, 5.0, |c| c.toy.state == toy::State::Resting);
        assert_eq!(c.toy.state, toy::State::Resting);
    }

    #[test]
    fn name_and_look_reach_mcp_and_renderer() {
        let (mut c, mut p) = (core(), Fake::default());
        let s = Settings {
            name: "Karabaş".into(),
            wear: look::Wear::Beanie,
            ..c.settings.clone()
        };
        c.on_action(&mut p, Action::Settings(Box::new(s)));
        let r = c
            .mcp
            .dispatch(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
            .unwrap();
        let text = r["result"]["instructions"].as_str().unwrap();
        assert!(text.starts_with("Karabaş (the user's dog)"), "{text}");
        assert_eq!(c.renderer.outfit().hat, Some(look::Item::Beanie));
        assert_eq!(c.panel_view(&p).settings.name, "Karabaş");

        // A new fur colour tells the shells to draw the tray icon again.
        let before = c.renderer.look_gen();
        let fur = if c.settings.fur == look::Fur::Black {
            look::Fur::Cream
        } else {
            look::Fur::Black
        };
        let s = Settings {
            fur,
            ..c.settings.clone()
        };
        c.on_action(&mut p, Action::Settings(Box::new(s)));
        assert_ne!(c.renderer.look_gen(), before);
        let before = c.renderer.look_gen();
        let s = Settings {
            name: "Dost".into(),
            ..c.settings.clone()
        };
        c.on_action(&mut p, Action::Settings(Box::new(s)));
        assert_eq!(
            c.renderer.look_gen(),
            before,
            "other settings keep the icon"
        );
    }

    #[test]
    fn answered_in_words_and_from_the_phone() {
        let (mut c, mut p) = (core(), Fake::default());
        let (mut n, rx) = Notification::new("Plan", "Which approach?")
            .ask(vec!["A".into(), "B".into()], Duration::from_secs(60));
        n.allow_text = true;
        n.session = Some("s1".into());
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert!(c.bubble().unwrap().reply);
        assert_eq!(c.sessions.list()[0].state, sessions::State::Waiting);
        let id = c.shown.as_ref().unwrap().note.id;
        c.answer_text(id, "  B, but skip the tests ");
        assert_eq!(rx.try_recv().unwrap(), "B, but skip the tests");
        assert_eq!(c.sessions.list()[0].state, sessions::State::Working);

        // From the phone: a choice is taken, anything else only when words are allowed.
        let (n, rx) = Notification::new("", "Deploy?")
            .ask(vec!["Yes".into(), "No".into()], Duration::from_secs(60));
        c.notifier.send(n);
        step(&mut c, &mut p);
        let id = c.shown.as_ref().unwrap().note.id;
        c.on_remote(RemoteEvent::Answer {
            id,
            answer: "maybe".into(),
        });
        assert!(c.shown.is_some(), "not a choice");
        c.on_remote(RemoteEvent::Answer {
            id,
            answer: "No".into(),
        });
        assert_eq!(rx.try_recv().unwrap(), "No");
        assert!(c.shown.is_none());
        assert_eq!(history(&c).last().unwrap().1, "answered on phone: No");
    }

    #[test]
    fn telegram_pairing_is_saved() {
        let (mut c, _p) = (core(), Fake::default());
        c.on_remote(RemoteEvent::Paired {
            chat_id: "42".into(),
        });
        assert_eq!(c.settings.phone.telegram.chat_id, "42");
    }

    #[test]
    fn minute_of_silence_holds_everything() {
        let (mut c, mut p) = (core(), Fake::default());
        c.silence_until = Some(Instant::now() + Duration::from_secs(60));
        c.notifier.send(Notification::new("Build done", ""));
        c.notifier.react(Reaction::Bark);
        step(&mut c, &mut p);
        assert!(c.shown.is_none(), "waits");
        assert_eq!(p.barks, 0);
        assert!(c.hushed());
        c.silence_until = Some(Instant::now());
        step(&mut c, &mut p);
        assert_eq!(c.shown.as_ref().unwrap().note.title, "Build done");
    }

    #[test]
    fn ages() {
        let d = |s| look::parse_date(s).unwrap();
        assert_eq!(age_text(Some(d("2024-10-21")), d("2026-10-21")), "2 years");
        assert_eq!(
            age_text(Some(d("2025-10-22")), d("2026-10-21")),
            "11 months"
        );
        assert_eq!(age_text(Some(d("2026-10-20")), d("2026-10-21")), "1 day");
        assert_eq!(age_text(None, d("2026-10-21")), "new here");
    }

    #[test]
    fn asleep_while_hidden() {
        let (mut c, mut p) = (core(), Fake::default());
        hidden(&mut c, &mut p);
        c.on_action(&mut p, Action::ToggleSleep);
        step(&mut c, &mut p);
        assert!(c.dog.is_sleeping());
        assert!(c.dog.is_away(), "sleeping does not bring it back");
        c.notifier.send(Notification::new("FYI", ""));
        step(&mut c, &mut p);
        assert_eq!(p.barks, 0);
        assert_eq!(c.missed, 0, "skipped while asleep, not counted as missed");
        let (n, _rx) =
            Notification::new("", "Allow?").ask(vec!["Yes".into()], Duration::from_secs(60));
        c.notifier.send(n);
        step(&mut c, &mut p);
        assert!(c.bubble_alone(), "questions still pop up next to the icon");
        assert_eq!(p.barks, 0, "silently");
    }
}
