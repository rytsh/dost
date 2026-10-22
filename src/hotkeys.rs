//! Global keyboard shortcuts for the bubble: ⌥⌘ (macOS) or Ctrl+Alt (Windows, Linux) with
//! Enter, 1-4, Backspace or L. They are only registered while a bubble is shown, so they do
//! not take these keys away from other apps the rest of the time.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use dost::app::Key;

#[cfg(target_os = "macos")]
const MODS: Modifiers = Modifiers::ALT.union(Modifiers::META);
#[cfg(any(windows, target_os = "linux"))]
const MODS: Modifiers = Modifiers::ALT.union(Modifiers::CONTROL);

const KEYS: [(Code, Key); 7] = [
    (Code::Enter, Key::Accept),
    (Code::Digit1, Key::Choice(0)),
    (Code::Digit2, Key::Choice(1)),
    (Code::Digit3, Key::Choice(2)),
    (Code::Digit4, Key::Choice(3)),
    (Code::Backspace, Key::Dismiss),
    (Code::KeyL, Key::Later),
];

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    keys: Vec<(HotKey, Key)>,
    on: bool,
}

impl Hotkeys {
    pub fn new() -> Self {
        Hotkeys {
            manager: GlobalHotKeyManager::new().ok(),
            keys: KEYS
                .iter()
                .map(|(c, k)| (HotKey::new(Some(MODS), *c), *k))
                .collect(),
            on: false,
        }
    }

    /// Registers the shortcuts while `active` (a bubble is shown), unregisters otherwise.
    pub fn set_active(&mut self, active: bool) {
        if active == self.on {
            return;
        }
        let Some(m) = &self.manager else { return };
        self.on = active;
        for (hk, _) in &self.keys {
            // A key another app already uses is simply skipped.
            let _ = if active {
                m.register(*hk)
            } else {
                m.unregister(*hk)
            };
        }
        if !active {
            // Presses that arrived just before; they are for a bubble that is gone.
            while GlobalHotKeyEvent::receiver().try_recv().is_ok() {}
        }
    }

    /// Shortcuts pressed since the last call.
    pub fn pressed(&self) -> Vec<Key> {
        let mut out = Vec::new();
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if let Some((_, k)) = self.keys.iter().find(|(hk, _)| hk.id() == ev.id) {
                out.push(*k);
            }
        }
        out
    }
}
