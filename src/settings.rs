//! User settings: changed from "Settings" in the tray menu and saved to `settings.json`.
//! The file can also be edited by hand; the app notices the change and reloads it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::look::{Collar, Fur, Theme, Wear};

/// Dog size, percent. The sprite is 128 px tall; on screen it is rounded to a whole multiple so
/// pixels stay crisp and even (100 = 1:1). The panel steps by `SIZE_STEP`.
pub const SIZE_STEP: u32 = 50;
pub const SPEEDS: [(f32, &str); 3] = [(0.6, "Slow"), (1.0, "Normal"), (1.6, "Fast")];
/// Bubble text size multipliers.
pub const TEXT_SIZES: [(f32, &str); 4] = [(1.0, "S"), (1.2, "M"), (1.45, "L"), (1.75, "XL")];
/// How long the bubble stays, seconds, as the panel steps through them; 0 = until clicked.
pub const NOTIFY_STEPS: [u64; 10] = [5, 10, 15, 20, 30, 45, 60, 120, 300, 0];

/// The baseline the dog stands on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ground {
    /// Top of the taskbar (bottom of the screen if there is no taskbar or it auto-hides).
    #[default]
    Taskbar,
    /// Very bottom of the screen; the dog stands in front of the taskbar.
    Screen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Dog size, percent (50-400); values 1-6 from old files are multiplied by 50.
    pub size: u32,
    /// Walking speed multiplier.
    pub speed: f32,
    /// false: the dog stays in place; it only stands, sits or sleeps.
    pub wander: bool,
    pub ground: Ground,
    /// Raises the ground this much above the baseline (logical pixels, -20..2000).
    /// If negative, the dog starts slightly below the line.
    pub lift: i32,
    /// Bubble text size multiplier (0.8-3.0).
    pub text_size: f32,
    /// Bark sound when a notification arrives.
    pub sound: bool,
    /// Volume, percent (0-100).
    pub volume: u8,
    /// Also bark when the dog is clicked.
    pub sound_on_pet: bool,
    /// How long notifications without an explicit timeout stay in the bubble; 0 = until clicked.
    pub notify_secs: u64,
    pub mcp_enabled: bool,
    pub mcp_addr: String,
    /// Which web pages may use the MCP server from a browser (pages on localhost always can).
    pub mcp_cors: crate::mcp::Cors,
    /// Do not disturb: no barking and only important bubbles (questions, sticky, progress).
    pub quiet: bool,
    /// Daily quiet hours as "HH:MM" local time; `quiet_from` == `quiet_to` disables them.
    /// The range may wrap past midnight (22:00-08:00).
    pub quiet_from: String,
    pub quiet_to: String,
    /// Quiet while an app is full screen (a video, a presentation, screen sharing).
    pub quiet_fullscreen: bool,
    /// Look for new versions (at start and daily).
    pub check_updates: bool,
    /// Download and install new versions by itself (otherwise it only tells).
    pub auto_update: bool,
    /// Hidden (Hide): kept across restarts.
    pub hidden: bool,
    /// Put to sleep (Sleep): kept across restarts.
    pub asleep: bool,
    /// The dog's name: shown in bubbles and the panel, and told to agents over MCP.
    pub name: String,
    /// The day the dog was adopted ("YYYY-MM-DD", set on first start): its birthday.
    pub birthday: String,
    pub fur: Fur,
    pub collar: Collar,
    pub wear: Wear,
    /// Holiday outfits on their days (birthday, Halloween, Christmas).
    pub seasonal: bool,
    pub theme: Theme,
    /// Bubble colour per source, lowercase name -> "#rrggbb"; others get a colour of their own.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub source_colors: std::collections::BTreeMap<String, String>,
    /// Bark sounds: 16-bit PCM WAV files, one picked at random; empty = the built-in bark.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bark_files: Vec<String>,
    /// A different sound for good news, failures and questions.
    pub event_sounds: bool,
    /// The bone lies next to the dog (to throw, and now and then it plays with it).
    pub toy: bool,
    /// Language of the panel, the settings window and the dog's own bubbles.
    pub language: Lang,
    /// Special days: commemorations (10 November, 09:05) and national holidays.
    pub special_days: bool,
    /// The siren at a commemoration's minute of silence.
    pub siren: bool,
    /// Which screen the dog lives on: 0 wherever it is put, 1 the main screen, n the
    /// screen n-1 (in the order the system lists them).
    pub screen: u32,
    /// Notifications on the phone (ntfy, Telegram).
    pub phone: crate::remote::PhoneSettings,
}

/// The language of the app's own text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    /// The system's language (Turkish when it is Turkish, otherwise English).
    #[default]
    Auto,
    En,
    Tr,
}

impl Lang {
    pub const ALL: [(Lang, &str); 3] = [
        (Lang::Auto, "System"),
        (Lang::En, "English"),
        (Lang::Tr, "Türkçe"),
    ];
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            size: 100,
            speed: 1.0,
            wander: true,
            ground: Ground::Taskbar,
            lift: -3,
            notify_secs: 10,
            text_size: 1.2,
            sound: true,
            volume: 50,
            sound_on_pet: false,
            mcp_enabled: true,
            mcp_addr: "127.0.0.1:7777".into(),
            mcp_cors: Default::default(),
            quiet: false,
            quiet_from: "22:00".into(),
            quiet_to: "22:00".into(),
            quiet_fullscreen: true,
            check_updates: true,
            auto_update: true,
            hidden: false,
            asleep: false,
            name: DEFAULT_NAME.into(),
            birthday: String::new(),
            fur: Fur::Golden,
            collar: Collar::Red,
            wear: Wear::None,
            seasonal: true,
            theme: Theme::Classic,
            source_colors: Default::default(),
            bark_files: Vec::new(),
            event_sounds: true,
            toy: true,
            language: Lang::Auto,
            special_days: true,
            siren: false,
            screen: 0,
            phone: Default::default(),
        }
    }
}

pub const DEFAULT_NAME: &str = "Dost";
/// Longest name, in characters.
pub const NAME_MAX: usize = 20;

impl Settings {
    /// Fixes invalid values (for hand-edited files).
    pub fn sanitized(mut self) -> Self {
        let d = Settings::default();
        // In old settings files (64 px sprite) size was a multiplier from 1 to 6.
        if self.size <= 6 {
            self.size *= 50;
        }
        self.size = self.size.clamp(50, 400);
        if !(self.speed.is_finite() && (0.1..=5.0).contains(&self.speed)) {
            self.speed = d.speed;
        }
        self.notify_secs = self.notify_secs.min(3600);
        self.lift = self.lift.clamp(-20, 2000);
        self.volume = self.volume.min(100);
        if !(self.text_size.is_finite() && (0.8..=3.0).contains(&self.text_size)) {
            self.text_size = d.text_size;
        }
        if self.mcp_addr.trim().is_empty() {
            self.mcp_addr = d.mcp_addr;
        }
        if parse_hhmm(&self.quiet_from).is_none() || parse_hhmm(&self.quiet_to).is_none() {
            self.quiet_from = d.quiet_from;
            self.quiet_to = d.quiet_to;
        }
        self.name = clean_name(&self.name);
        if self.wear == Wear::Gone {
            self.wear = Wear::None;
        }
        if !self.birthday.is_empty() && crate::look::parse_date(&self.birthday).is_none() {
            self.birthday.clear();
        }
        self.screen = self.screen.min(16);
        let p = &mut self.phone;
        p.ntfy.server = p.ntfy.server.trim().trim_end_matches('/').to_string();
        if p.ntfy.server.is_empty() {
            p.ntfy.server = d.phone.ntfy.server;
        }
        p.ntfy.topic = p.ntfy.topic.trim().to_string();
        p.telegram.token = p.telegram.token.trim().to_string();
        p.telegram.chat_id = p.telegram.chat_id.trim().to_string();
        self
    }

    /// The birthday as a date (`None` until the first start sets it).
    pub fn birthday(&self) -> Option<chrono::NaiveDate> {
        crate::look::parse_date(&self.birthday)
    }

    /// `source_colors` parsed; bad entries are left out.
    pub fn source_colors(&self) -> Vec<(String, tiny_skia::ColorU8)> {
        self.source_colors
            .iter()
            .filter_map(|(k, v)| Some((k.trim().to_lowercase(), crate::look::parse_hex(v)?)))
            .collect()
    }

    /// Whether do-not-disturb is on at `minute` (minutes since local midnight).
    pub fn is_quiet(&self, minute: u32) -> bool {
        if self.quiet {
            return true;
        }
        let (Some(from), Some(to)) = (parse_hhmm(&self.quiet_from), parse_hhmm(&self.quiet_to))
        else {
            return false;
        };
        match from.cmp(&to) {
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Less => (from..to).contains(&minute),
            std::cmp::Ordering::Greater => minute >= from || minute < to,
        }
    }

    /// `None`: stays until clicked.
    pub fn notify_timeout(&self) -> Option<Duration> {
        (self.notify_secs > 0).then(|| Duration::from_secs(self.notify_secs))
    }
}

/// The settings file and the last read/written modification time.
pub struct Store {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
}

impl Store {
    pub fn new(path: Option<PathBuf>) -> Self {
        Store { path, mtime: None }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Defaults if the file is missing or corrupt.
    pub fn load(&mut self) -> Settings {
        let Some(path) = &self.path else {
            return Settings::default();
        };
        self.mtime = mtime(path);
        std::fs::read(path)
            .ok()
            .and_then(|d| serde_json::from_slice::<Settings>(&d).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&mut self, s: &Settings) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec_pretty(s) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                self.mtime = mtime(path);
            }
        }
    }

    /// Returns the new settings if the file was changed externally.
    pub fn reload_if_changed(&mut self) -> Option<Settings> {
        let path = self.path.as_ref()?;
        let now = mtime(path);
        if now.is_none() || now == self.mtime {
            return None;
        }
        Some(self.load())
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// One line, no control characters, at most [`NAME_MAX`] characters; empty -> the default.
pub fn clean_name(s: &str) -> String {
    let n: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect::<String>()
        .trim()
        .to_string();
    if n.is_empty() { DEFAULT_NAME.into() } else { n }
}

/// "HH:MM" -> minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `%APPDATA%\dost` (Windows) or `~/.config/dost`.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("dost"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_and_bad_files() {
        let s: Settings = serde_json::from_str(r#"{ "size": 300 }"#).unwrap();
        assert_eq!(
            s,
            Settings {
                size: 300,
                ..Settings::default()
            }
        );
        let s: Settings =
            serde_json::from_str(r#"{ "size": 999, "speed": -1, "mcp_addr": " ", "lift": -50 }"#)
                .unwrap();
        assert_eq!(
            s.sanitized(),
            Settings {
                size: 400,
                lift: -20,
                ..Settings::default()
            }
        );
        assert_eq!(
            Settings {
                notify_secs: 0,
                ..Settings::default()
            }
            .notify_timeout(),
            None
        );
    }

    #[test]
    fn quiet_hours() {
        let mut s = Settings::default();
        assert!(!s.is_quiet(23 * 60));
        s.quiet_from = "22:00".into();
        s.quiet_to = "08:00".into();
        assert!(s.is_quiet(23 * 60));
        assert!(s.is_quiet(7 * 60 + 59));
        assert!(!s.is_quiet(8 * 60));
        assert!(!s.is_quiet(12 * 60));
        s.quiet_from = "12:00".into();
        s.quiet_to = "13:00".into();
        assert!(s.is_quiet(12 * 60 + 30));
        assert!(!s.is_quiet(13 * 60));
        s.quiet = true;
        assert!(s.is_quiet(9 * 60));
        let bad = Settings {
            quiet_from: "25:00".into(),
            ..Settings::default()
        };
        assert_eq!(bad.sanitized().quiet_from, "22:00");
    }

    #[test]
    fn identity_and_look() {
        let s: Settings = serde_json::from_str(
            r##"{ "name": "  Karabaş\n ", "fur": "black", "collar": "blue", "wear": "beanie",
                 "birthday": "2026-02-30", "source_colors": { "CI": "#ff0000", "x": "nope" } }"##,
        )
        .unwrap();
        let s = s.sanitized();
        assert_eq!(s.name, "Karabaş");
        assert_eq!(
            (s.fur, s.collar, s.wear),
            (Fur::Black, Collar::Blue, Wear::Beanie)
        );
        assert_eq!(s.birthday, "", "not a real date");
        assert_eq!(s.source_colors().len(), 1);
        assert_eq!(s.source_colors()[0].0, "ci");
        // The bandana and the glasses are gone: an old file reads as wearing nothing.
        for gone in ["bandana", "glasses"] {
            let old: Settings =
                serde_json::from_str(&format!(r#"{{ "wear": "{gone}", "collar": "none" }}"#))
                    .unwrap();
            let old = old.sanitized();
            assert_eq!((old.wear, old.collar), (Wear::None, Collar::None));
        }
        assert_eq!(clean_name(""), DEFAULT_NAME);
        assert_eq!(clean_name(&"x".repeat(50)).chars().count(), NAME_MAX);
        // Defaults are not written out for the empty maps and lists.
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!json.contains("bark_files") && !json.contains("source_colors"));
    }

    #[test]
    fn ground_settings() {
        let s: Settings = serde_json::from_str(r#"{ "ground": "screen", "lift": 35 }"#).unwrap();
        assert_eq!((s.ground, s.lift), (Ground::Screen, 35));
        let s: Settings = serde_json::from_str(r#"{ "lift": 99999 }"#).unwrap();
        assert_eq!(s.sanitized().lift, 2000);
    }

    #[test]
    fn save_load_reload() {
        let dir = std::env::temp_dir().join(format!("dost-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut store = Store::new(Some(path.clone()));
        assert_eq!(store.load(), Settings::default());
        let s = Settings {
            size: 200,
            wander: false,
            ..Settings::default()
        };
        store.save(&s);
        assert_eq!(store.reload_if_changed(), None);
        assert_eq!(Store::new(Some(path.clone())).load(), s);

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, r#"{ "size": 2 }"#).unwrap();
        let t = SystemTime::now() + Duration::from_secs(5);
        let _ = std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(t));
        // A "2" from an old file (64 px sprite × 2) maps to the same on-screen size, 100%.
        assert_eq!(store.reload_if_changed().map(|s| s.size), Some(100));
        let _ = std::fs::remove_dir_all(dir);
    }
}
