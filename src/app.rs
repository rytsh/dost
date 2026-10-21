//! Platform-independent app core: notification queue, settings, MCP server, watches and the
//! settings panel's actions. Each platform shell (`win.rs`, `mac.rs`) owns the windows and
//! implements [`Platform`] for the few things that differ (sound, clipboard, autostart, ...).

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::dog::{Anim, Bounds, Dog};
use crate::mcp::{self, DogStatus, Mcp};
use crate::notify::{self, Notification, Notifier};
use crate::panel::{self, Action};
use crate::render::{Bubble, Hits, Renderer, Scale};
use crate::settings::{self, Settings, Store};
use crate::watch::{self, Watches};

/// What a platform shell provides to the shared core.
pub trait Platform {
    fn play_bark(&mut self, volume: u8);
    fn copy_text(&mut self, text: &str) -> bool;
    fn autostart_enabled(&self) -> bool;
    fn set_autostart(&mut self, on: bool) -> Result<(), String>;
    fn open_folder(&mut self, path: &std::path::Path);
    /// Shows or hides the dog's window.
    fn set_dog_visible(&mut self, visible: bool);
    fn close_panel(&mut self);
    fn quit(&mut self);
    /// "Start with Windows" / "Open at login".
    fn autostart_label(&self) -> &'static str;
}

pub struct Shown {
    pub note: Notification,
    pub deadline: Option<Instant>,
}

pub struct Core {
    pub dog: Dog,
    pub renderer: Renderer,
    pub settings: Settings,
    pub store: Store,
    pub visible: bool,
    pub shown: Option<Shown>,
    pub queue: VecDeque<Notification>,
    pub hits: Hits,
    pub mcp_error: Option<String>,
    /// The panel must be redrawn (settings or state changed).
    pub panel_dirty: bool,
    rx: Receiver<Notification>,
    notifier: Notifier,
    mcp: Arc<Mcp>,
    mcp_server: Option<mcp::Running>,
    status: Arc<Mutex<DogStatus>>,
    last: Instant,
    last_settings_check: Instant,
}

impl Core {
    /// Loads settings, starts watches and (if enabled) the MCP server.
    /// `x`, `ground`: where the dog starts (physical pixels).
    pub fn new(renderer: Renderer, x: f32, ground: f32) -> Self {
        let mut store = Store::new(settings::config_dir().map(|d| d.join("settings.json")));
        let settings = store.load();

        let (notifier, rx) = notify::channel();
        let status = Arc::new(Mutex::new(DogStatus::default()));
        let watches = Watches::new(notifier.clone(), watch::default_path());
        watches.start();
        let mcp = Arc::new(Mcp::new(notifier.clone(), watches, Arc::clone(&status)));

        let mut core = Core {
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
        };
        core.apply_settings(settings, false);
        let hello = match &core.mcp_error {
            None => Notification::new(
                "Woof!",
                "Hi, I'm Dost. Drag me around, or click my tray icon for settings.",
            )
            .timeout(Duration::from_secs(6)),
            Some(e) => Notification::new("MCP server could not start", e.clone()).sticky(),
        };
        core.notifier.send(hello);
        core
    }

    /// One frame: notifications, settings file changes, the dog's movement.
    /// `bounds`: where the dog may walk on its current screen.
    pub fn tick(&mut self, p: &mut dyn Platform, bounds: &Bounds) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;

        while let Ok(n) = self.rx.try_recv() {
            self.queue.push_back(n);
        }
        if now - self.last_settings_check > Duration::from_secs(1) {
            self.last_settings_check = now;
            if let Some(s) = self.store.reload_if_changed() {
                self.apply_settings(s, false);
            }
        }
        if self
            .shown
            .as_ref()
            .is_some_and(|s| s.deadline.is_some_and(|d| now >= d))
        {
            self.shown = None;
        }
        if self.shown.is_none()
            && let Some(note) = self.queue.pop_front()
        {
            self.dog.notify();
            if self.settings.sound {
                p.play_bark(self.settings.volume);
            }
            let deadline = note
                .resolve_timeout(self.settings.notify_timeout())
                .map(|t| now + t);
            self.shown = Some(Shown { note, deadline });
        }
        self.dog.hold = self.shown.is_some();
        self.publish_status();
        self.dog.update(dt, bounds);
    }

    /// The dog's scale on a screen with the given UI (DPI) scale.
    pub fn scale(&self, ui: f32) -> Scale {
        Scale::new(ui, self.settings.size).with_text(self.settings.text_size)
    }

    pub fn bubble(&self) -> Option<Bubble<'_>> {
        self.shown.as_ref().map(|s| Bubble {
            title: &s.note.title,
            body: &s.note.body,
            pending: self.queue.len(),
        })
    }

    /// Click on the dog's window that was not a drag: closes the bubble or pets the dog.
    pub fn click(&mut self, p: &mut dyn Platform, x: f32, y: f32) {
        let on_bubble = self
            .hits
            .bubble
            .is_some_and(|r| x >= r.left() && x <= r.right() && y >= r.top() && y <= r.bottom());
        if on_bubble {
            self.shown = None;
        } else {
            self.dog.pet();
            if self.settings.sound && self.settings.sound_on_pet {
                p.play_bark(self.settings.volume);
            }
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
        if save {
            self.store.save(&s);
        }
        self.settings = s;
        self.panel_dirty = true;
    }

    pub fn on_action(&mut self, p: &mut dyn Platform, action: Action) {
        match action {
            Action::Settings(s) => {
                self.apply_settings(s, true);
                if let Some(e) = &self.mcp_error {
                    self.queue.push_back(
                        Notification::new("MCP server could not start", e.clone()).sticky(),
                    );
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
                        p.play_bark(self.settings.volume);
                    }
                }
            }
            Action::TestNotification => self.queue.push_back(Notification::new(
                "Test",
                "This is a test notification. Click the bubble to close it.",
            )),
            Action::ToggleSleep => self.dog.toggle_sleep(),
            Action::ToggleVisible => {
                self.visible = !self.visible;
                p.set_dog_visible(self.visible);
            }
            Action::ToggleAutostart => {
                let on = !p.autostart_enabled();
                if let Err(e) = p.set_autostart(on) {
                    self.queue
                        .push_back(Notification::new("Could not change autostart", e));
                }
            }
            Action::CopyMcpUrl => {
                let url = self.mcp_url();
                let msg = if p.copy_text(&url) {
                    format!("{url} copied to the clipboard.")
                } else {
                    url
                };
                self.queue.push_back(
                    Notification::new("MCP address", msg).timeout(Duration::from_secs(5)),
                );
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
        }
        self.panel_dirty = true;
    }

    pub fn toggle_mute(&mut self, p: &mut dyn Platform) {
        let mut s = self.settings.clone();
        s.sound = !s.sound;
        self.on_action(p, Action::Settings(s));
    }

    pub fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.settings.mcp_addr)
    }

    pub fn panel_view(&self, p: &dyn Platform) -> panel::View {
        panel::View {
            settings: self.settings.clone(),
            sleeping: self.dog.is_sleeping(),
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
        }
    }

    fn publish_status(&self) {
        let activity = match self.dog.frame().anim {
            Anim::Idle => "standing",
            Anim::Walk => "walking",
            Anim::Sit => "sitting",
            Anim::Bark => "barking",
            Anim::Happy => "happy",
            Anim::Sleep => "sleeping",
            Anim::Held => "being carried",
        };
        if let Ok(mut s) = self.status.lock() {
            s.activity = activity.into();
            s.visible = self.visible;
            s.showing = self
                .shown
                .as_ref()
                .map(|s| format!("{}: {}", s.note.title, s.note.body));
            s.queued = self.queue.len();
        }
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
