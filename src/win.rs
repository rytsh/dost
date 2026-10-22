//! Windows shell: a per-pixel-alpha, always-on-top layered window for the dog (not shown on
//! the taskbar), a tray icon (left click: settings panel, right click: short menu) and the
//! settings panel window. Behaviour lives in [`dost::app::Core`].

use std::cell::RefCell;
use std::time::Duration;

use tiny_skia::Pixmap;
use tray_icon::menu::{ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use dost::app::{self, Core, EDGE, Platform};
use dost::dog::Bounds;
use dost::panel::{self, Panel};
use dost::render::{Renderer, Scale};

const TIMER_ID: usize = 1;
const FRAME_MS: u32 = 16;
/// A press that moves less than this is a click, not a drag.
const DRAG_THRESHOLD: i32 = 4;

struct Drag {
    start: POINT,
    /// Cursor offset from the dog's feet.
    grab_dx: f32,
    grab_dy: f32,
    moved: bool,
}

struct App {
    core: Core,
    shell: Shell,
}

/// Windows-specific state; implements [`Platform`] for the core.
struct Shell {
    hwnd: HWND,
    pixmap: Pixmap,
    dib: Dib,
    /// Top-left of the dog's window on screen.
    win_pos: POINT,
    drag: Option<Drag>,
    last_topmost: std::time::Instant,
    tray: TrayIcon,
    /// What the tray icon shows: (sleeping dog, unread mark).
    /// Asleep, something missed, and the colours (`Renderer::look_gen`) the icon shows.
    tray_icon: (bool, bool, u32),
    menu: Menu,
    items: MenuItems,
    panel: PanelWindow,
    /// The big settings window (a normal, resizable top-level window).
    settings: PanelWindow,
    hotkeys: crate::hotkeys::Hotkeys,
    /// While hidden: the bubble on its own above the tray icon.
    alone: AloneBubble,
    /// The bone's window.
    toy: ToyWindow,
}

/// A small layered window for the toy, so it can fly anywhere along the taskbar.
struct ToyWindow {
    hwnd: HWND,
    pixmap: Pixmap,
    dib: Dib,
    pos: POINT,
    shown: bool,
    /// Cursor offset from the toy's bottom-centre while dragging.
    drag: Option<(f32, f32)>,
}

/// A bubble window without the dog, above the tray icon.
struct AloneBubble {
    hwnd: HWND,
    pixmap: Pixmap,
    dib: Dib,
    pos: POINT,
    shown: bool,
    /// The tray icon's top-centre, refreshed on tray events.
    anchor: Option<POINT>,
}

struct MenuItems {
    show: MenuItem,
    mute: MenuItem,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let hwnd = unsafe { create_window() };
    let renderer = Renderer::new(true, Vec::new());

    let (menu, items) = build_menu();
    let icon = Icon::from_rgba(renderer.icon_rgba(32, false, false), 32, 32).expect("tray icon");
    // Left click opens the panel, right click shows the short menu.
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Dost")
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .expect("tray icon");

    let work = work_area(POINT { x: 0, y: 0 });
    let start_x = work.left as f32 + (work.right - work.left) as f32 * 0.75;
    let core = Core::new(renderer, start_x, work.bottom as f32);
    let shell = Shell {
        hwnd,
        pixmap: Pixmap::new(1, 1).unwrap(),
        dib: Dib::default(),
        win_pos: POINT { x: 0, y: 0 },
        drag: None,
        last_topmost: std::time::Instant::now(),
        tray,
        tray_icon: (false, false, u32::MAX),
        menu,
        items,
        panel: PanelWindow::default(),
        settings: PanelWindow::default(),
        hotkeys: crate::hotkeys::Hotkeys::new(),
        alone: AloneBubble {
            hwnd: unsafe { create_window() },
            pixmap: Pixmap::new(1, 1).unwrap(),
            dib: Dib::default(),
            pos: POINT { x: 0, y: 0 },
            shown: false,
            anchor: None,
        },
        toy: ToyWindow {
            hwnd: unsafe { create_window() },
            pixmap: Pixmap::new(1, 1).unwrap(),
            dib: Dib::default(),
            pos: POINT { x: 0, y: 0 },
            shown: false,
            drag: None,
        },
    };
    // Hidden last time: stays hidden.
    let shown = core.on_screen();
    APP.with(|a| *a.borrow_mut() = Some(App { core, shell }));
    with_app(App::sync_menu);

    unsafe {
        if shown {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        SetTimer(hwnd, TIMER_ID, FRAME_MS, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    with_app(|a| a.core.shutdown());
    APP.with(|a| a.borrow_mut().take());
}

/// Right-click menu: only the essentials; everything else lives in the settings panel.
fn build_menu() -> (Menu, MenuItems) {
    let items = MenuItems {
        show: MenuItem::with_id("show", "Hide", true, None),
        mute: MenuItem::with_id("mute", "Mute", true, None),
    };
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id("panel", "Settings…", true, None),
        &PredefinedMenuItem::separator(),
        &items.show,
        &items.mute,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit Dost", true, None),
    ])
    .expect("menu");
    (menu, items)
}

impl Platform for Shell {
    fn play_wav(&mut self, wav: Vec<u8>) {
        sound::play(wav);
    }

    fn copy_text(&mut self, text: &str) -> bool {
        clipboard::set_text(text)
    }

    fn autostart_enabled(&self) -> bool {
        autostart::enabled()
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        autostart::set(on)
    }

    fn open_folder(&mut self, path: &std::path::Path) {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }

    fn open_url(&mut self, url: &str) {
        // `explorer <url>` opens the default browser; the URL was checked to be http(s).
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }

    fn idle_secs(&self) -> Option<u64> {
        use windows_sys::Win32::System::SystemInformation::GetTickCount;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        let now = unsafe { GetTickCount() };
        Some(u64::from(now.wrapping_sub(info.dwTime)) / 1000)
    }

    fn fullscreen(&self) -> bool {
        use windows_sys::Win32::UI::Shell::{
            QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
            SHQueryUserNotificationState,
        };
        let mut state = 0;
        if unsafe { SHQueryUserNotificationState(&mut state) } != 0 {
            return false;
        }
        matches!(
            state,
            QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
        )
    }

    fn screen_locked(&self) -> bool {
        use windows_sys::Win32::System::StationsAndDesktops::{
            CloseDesktop, DESKTOP_SWITCHDESKTOP, OpenInputDesktop, SwitchDesktop,
        };
        // While locked, the input desktop is the secure one: it cannot be switched to.
        unsafe {
            let d = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
            if d.is_null() {
                return true;
            }
            let ok = SwitchDesktop(d) != 0;
            CloseDesktop(d);
            !ok
        }
    }

    fn system_notify(&mut self, title: &str, body: &str) {
        use windows_sys::Win32::UI::Shell::{
            NIF_INFO, NIF_STATE, NIIF_INFO, NIM_ADD, NIM_DELETE, NIS_HIDDEN, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        };
        // A hidden helper icon of our own carries the Windows notification (toast).
        const ID: u32 = 0xD057;
        let copy = |dst: &mut [u16], s: &str| {
            let max = dst.len() - 1;
            for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(max)) {
                *d = c;
            }
        };
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = ID;
            Shell_NotifyIconW(NIM_DELETE, &nid);
            nid.uFlags = NIF_INFO | NIF_STATE;
            nid.dwState = NIS_HIDDEN;
            nid.dwStateMask = NIS_HIDDEN;
            nid.dwInfoFlags = NIIF_INFO;
            copy(&mut nid.szInfoTitle, title);
            copy(&mut nid.szInfo, if body.is_empty() { " " } else { body });
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    fn set_dog_visible(&mut self, visible: bool) {
        unsafe { ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    }

    fn close_panel(&mut self) {
        self.panel.close();
    }

    fn quit(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
    }

    fn autostart_label(&self) -> &'static str {
        "Start with Windows"
    }

    fn open_settings_window(&mut self) {
        if let Some(hwnd) = self.settings.hwnd {
            unsafe {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
            }
            return;
        }
        self.settings.open_window();
    }

    fn open_reply(&mut self, id: u64, title: &str, question: &str) {
        if !self.panel.is_open() {
            let anchor = self
                .tray_anchor()
                .or(self.alone.anchor)
                .unwrap_or_else(cursor_pos);
            self.panel.open(anchor);
        }
        let Some(hwnd) = self.panel.hwnd else {
            return;
        };
        if let Some(p) = self.panel.panel.as_mut() {
            p.start_reply(id, title, question);
        }
        self.panel.redraw = true;
        unsafe { SetForegroundWindow(hwnd) };
    }

    fn screen_count(&self) -> usize {
        monitors().len().max(1)
    }
}

/// Which panel window: the popover above the tray icon or the big settings window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Popover,
    Settings,
}

impl Shell {
    fn surface(&mut self, which: Which) -> &mut PanelWindow {
        match which {
            Which::Popover => &mut self.panel,
            Which::Settings => &mut self.settings,
        }
    }

    /// The tray icon's top-centre.
    fn tray_anchor(&self) -> Option<POINT> {
        self.tray.rect().map(|r| POINT {
            x: (r.position.x + r.size.width as f64 / 2.0) as i32,
            y: r.position.y as i32,
        })
    }
}

impl App {
    /// Keyboard shortcuts for the bubble: on only while one is shown.
    fn keys(&mut self) {
        for k in self.shell.hotkeys.pressed() {
            self.core.on_key(&mut self.shell, k);
        }
        let shown = self.core.shown.is_some();
        self.shell.hotkeys.set_active(shown);
    }

    fn tick(&mut self) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            self.on_menu(ev.id.as_ref());
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = ev
            {
                let anchor = POINT {
                    x: (rect.position.x + rect.size.width as f64 / 2.0) as i32,
                    y: rect.position.y as i32,
                };
                self.toggle_panel(Some(anchor));
            }
        }

        let bounds = self.bounds();
        self.core.tick(&mut self.shell, &bounds);
        self.keys();
        if self.core.on_screen() {
            self.paint();
        }
        self.paint_toy();
        self.paint_alone();
        if self.core.panel_dirty {
            self.core.panel_dirty = false;
            self.sync_menu();
        }
        self.paint_panel(Which::Popover);
        self.paint_panel(Which::Settings);
        let state = (
            self.core.dog.is_put_to_sleep(),
            self.core.missed > 0,
            self.core.renderer.look_gen(),
        );
        if state != self.shell.tray_icon {
            self.shell.tray_icon = state;
            let rgba = self.core.renderer.icon_rgba(32, state.0, state.1);
            if let Ok(icon) = Icon::from_rgba(rgba, 32, 32) {
                let _ = self.shell.tray.set_icon(Some(icon));
            }
        }
        let now = std::time::Instant::now();
        if now - self.shell.last_topmost > Duration::from_secs(2) {
            // Clicking the taskbar raises it above us; put the dog back on top.
            self.shell.last_topmost = now;
            for hwnd in [self.shell.hwnd, self.shell.toy.hwnd] {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
    }

    fn bounds(&self) -> Bounds {
        let (work, mon, ui) = self.dog_screen();
        let px = Scale::new(ui, self.core.settings.size).px as f32;
        Bounds {
            ground: app::ground_line(
                &self.core.settings,
                work.bottom as f32,
                mon.top as f32,
                mon.bottom as f32,
                ui,
                px,
            ),
            left: work.left as f32 + EDGE * px,
            right: work.right as f32 - EDGE * px,
            scale: px,
        }
    }

    /// Work area, monitor rect and UI scale of the screen the dog lives on (settings →
    /// Screen: 1 is the primary one), or else of the one it is on.
    fn dog_screen(&self) -> (RECT, RECT, f32) {
        let n = self.core.settings.screen as usize;
        if n > 0
            && let Some(&mon) = monitors().get(n - 1)
        {
            let mi = hmonitor_info(mon);
            return (mi.rcWork, mi.rcMonitor, hmonitor_scale(mon));
        }
        let pt = self.dog_point();
        let mi = monitor_info(pt);
        (mi.rcWork, mi.rcMonitor, monitor_scale(pt))
    }

    fn dog_point(&self) -> POINT {
        POINT {
            x: self.core.dog.x as i32,
            y: self.core.dog.y as i32 - 10,
        }
    }

    fn paint(&mut self) {
        let scale = self.core.scale(monitor_scale(self.dog_point()));
        let frame = self.core.dog.frame();
        let core = &mut self.core;
        let bubble =
            dost::app::bubble_of(core.shown.as_ref().filter(|_| core.visible), core.waiting());
        let (w, h) = core.renderer.size(frame.anim, bubble.as_ref(), scale);
        let sh = &mut self.shell;
        if sh.pixmap.width() != w || sh.pixmap.height() != h {
            sh.pixmap = Pixmap::new(w, h).unwrap();
        }
        // The window centres the dog, clamped so the bubble stays on screen.
        let mon = monitor_rect(POINT {
            x: core.dog.x as i32,
            y: core.dog.y as i32 - 10,
        });
        let x = (core.dog.x as i32 - w as i32 / 2)
            .clamp(mon.left, (mon.right - w as i32).max(mon.left));
        let y = (core.dog.y - Renderer::ground_offset(h, scale)).round() as i32;
        sh.win_pos = POINT { x, y };
        let anchor = core.dog.x - x as f32;
        core.hits = core
            .renderer
            .draw(&mut sh.pixmap, anchor, &frame, bubble.as_ref(), scale);
        unsafe { sh.dib.present(sh.hwnd, &sh.pixmap, sh.win_pos) };
    }

    /// The toy's window: shown while the toy lies, flies or is dragged (not in the mouth).
    fn paint_toy(&mut self) {
        let show = self.core.toy_shown();
        let t = &mut self.shell.toy;
        if !show {
            if t.shown {
                t.shown = false;
                unsafe { ShowWindow(t.hwnd, SW_HIDE) };
            }
            return;
        }
        let Some(item) = self.core.renderer.outfit().toy else {
            return;
        };
        let pt = POINT {
            x: self.core.toy.x as i32,
            y: self.core.toy.y as i32 - 10,
        };
        let px = self.core.scale(monitor_scale(pt)).px;
        let size = self.core.renderer.toy_size(item, px);
        if t.pixmap.width() != size {
            t.pixmap = Pixmap::new(size, size).unwrap();
        }
        self.core
            .renderer
            .draw_toy(&mut t.pixmap, item, self.core.toy.quarter(), px);
        t.pos = POINT {
            x: (self.core.toy.x - size as f32 / 2.0).round() as i32,
            y: (self.core.toy.y - size as f32).round() as i32,
        };
        unsafe {
            t.dib.present(t.hwnd, &t.pixmap, t.pos);
            if !t.shown {
                t.shown = true;
                ShowWindow(t.hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(
                    t.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn toy_down(&mut self) {
        let p = cursor_pos();
        let t = &mut self.shell.toy;
        t.drag = Some((p.x as f32 - self.core.toy.x, p.y as f32 - self.core.toy.y));
        self.core.grab_toy();
        unsafe { SetCapture(t.hwnd) };
    }

    fn toy_move(&mut self) {
        if let Some((dx, dy)) = self.shell.toy.drag {
            let p = cursor_pos();
            self.core.toy.drag_to(p.x as f32 - dx, p.y as f32 - dy);
        }
    }

    fn toy_up(&mut self) {
        if self.shell.toy.drag.take().is_some() {
            unsafe { ReleaseCapture() };
            let scale = self.bounds().scale;
            self.core.toy.release(scale);
        }
    }

    /// While hidden: shows the current bubble on its own just above the tray icon.
    fn paint_alone(&mut self) {
        if !self.core.bubble_alone() {
            if self.shell.alone.shown {
                self.shell.alone.shown = false;
                unsafe { ShowWindow(self.shell.alone.hwnd, SW_HIDE) };
            }
            return;
        }
        let anchor = self
            .shell
            .tray_anchor()
            .or(self.shell.alone.anchor)
            .unwrap_or_else(|| {
                let w = work_area(POINT { x: 0, y: 0 });
                POINT {
                    x: w.right - 40,
                    y: w.bottom,
                }
            });
        self.shell.alone.anchor = Some(anchor);
        let k = monitor_scale(anchor);
        let scale = self.core.scale(k);
        let work = work_area(anchor);
        let core = &mut self.core;
        let Some(bubble) = dost::app::bubble_of(core.shown.as_ref(), core.waiting()) else {
            return;
        };
        let (w, h) = core.renderer.bubble_size(&bubble, scale);
        let a = &mut self.shell.alone;
        if a.pixmap.width() != w || a.pixmap.height() != h {
            a.pixmap = Pixmap::new(w, h).unwrap();
        }
        let margin = (PANEL_MARGIN as f32 * k) as i32;
        let x = (anchor.x - w as i32 / 2).clamp(
            work.left + margin,
            (work.right - w as i32 - margin).max(work.left),
        );
        // Above a bottom taskbar (tail down); below a top one (tail up).
        let below = anchor.y < (work.top + work.bottom) / 2;
        // The tail's tip right at the taskbar (the window's edge is shadow room).
        let inset = core.renderer.alone_inset(scale) as i32 - (k.round() as i32);
        let y = if below {
            work.top - inset
        } else {
            (work.bottom - h as i32 + inset).max(work.top)
        };
        core.hits = core.renderer.draw_bubble_alone(
            &mut a.pixmap,
            (anchor.x - x) as f32,
            !below,
            &bubble,
            scale,
        );
        a.pos = POINT { x, y };
        unsafe { a.dib.present(a.hwnd, &a.pixmap, a.pos) };
        if !a.shown {
            a.shown = true;
            unsafe { ShowWindow(a.hwnd, SW_SHOWNOACTIVATE) };
        }
    }

    /// A click on the bubble shown on its own (while hidden).
    fn alone_click(&mut self, client: POINT) {
        self.core
            .click(&mut self.shell, client.x as f32, client.y as f32);
    }

    fn sync_menu(&mut self) {
        let i = &self.shell.items;
        i.show
            .set_text(if self.core.visible { "Hide" } else { "Show" });
        i.mute.set_text(if self.core.settings.sound {
            "Mute"
        } else {
            "Unmute"
        });
        self.shell.panel.redraw = true;
        self.shell.settings.redraw = true;
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            "panel" => self.toggle_panel(None),
            "show" => self.on_action(panel::Action::ToggleVisible),
            "mute" => self.core.toggle_mute(&mut self.shell),
            "quit" => self.on_action(panel::Action::Quit),
            _ => {}
        }
        self.sync_menu();
    }

    fn on_action(&mut self, action: panel::Action) {
        if action == panel::Action::TakeToy {
            // The Bone button dragged out of the panel: the toy follows the mouse from here.
            unsafe { ReleaseCapture() };
            self.close_panel();
            let p = cursor_pos();
            let lift = 12.0 * self.bounds().scale;
            self.core.take_toy(p.x as f32, p.y as f32 + lift);
            self.shell.toy.drag = Some((0.0, -lift));
            unsafe { SetCapture(self.shell.toy.hwnd) };
            return;
        }
        self.core.on_action(&mut self.shell, action);
        self.sync_menu();
    }

    /// Opens the panel next to `anchor` (the tray icon), or near the cursor; closes it if open.
    fn toggle_panel(&mut self, anchor: Option<POINT>) {
        if self.shell.panel.is_open() {
            self.close_panel();
            return;
        }
        let missed = self.core.missed;
        self.shell.panel.open(anchor.unwrap_or_else(cursor_pos));
        self.shell.panel.redraw = true;
        if self.core.panel_opened()
            && let Some(p) = self.shell.panel.panel.as_mut()
        {
            p.show_missed(missed);
        }
    }

    fn paint_panel(&mut self, which: Which) {
        let w = self.shell.surface(which);
        // The caret blinks while a field is edited.
        if w.panel.as_ref().is_some_and(Panel::is_editing) {
            w.redraw = true;
        }
        if !w.is_open() || !w.redraw {
            return;
        }
        w.redraw = false;
        let view = self.core.panel_view(&self.shell);
        self.shell
            .surface(which)
            .paint(&mut self.core.renderer, &view);
    }

    /// A key for the panel's text field.
    fn panel_key(&mut self, which: Which, key: panel::Key) {
        let view = self.core.panel_view(&self.shell);
        let w = self.shell.surface(which);
        let Some(p) = w.panel.as_mut() else {
            return;
        };
        let (redraw, action) = p.key(key, &view);
        w.redraw |= redraw;
        if let Some(a) = action {
            self.panel_action(which, a);
        }
    }

    /// An action from one of the panel windows: Close closes the window it came from.
    fn panel_action(&mut self, which: Which, action: panel::Action) {
        if which == Which::Settings && action == panel::Action::Close {
            self.close_window(Which::Settings);
            return;
        }
        self.on_action(action);
    }

    /// Closes the popover; a name being edited is saved.
    fn close_panel(&mut self) {
        self.close_window(Which::Popover);
    }

    /// Closes a panel window; a field being edited is saved.
    fn close_window(&mut self, which: Which) {
        let view = self.core.panel_view(&self.shell);
        let w = self.shell.surface(which);
        let action = w.panel.as_mut().and_then(|p| p.finish_editing(&view));
        w.close();
        if let Some(a) = action {
            self.on_action(a);
        }
    }

    fn mouse_down(&mut self, client: POINT) {
        let sh = &mut self.shell;
        let screen = POINT {
            x: sh.win_pos.x + client.x,
            y: sh.win_pos.y + client.y,
        };
        sh.drag = Some(Drag {
            start: screen,
            grab_dx: screen.x as f32 - self.core.dog.x,
            grab_dy: screen.y as f32 - self.core.dog.y,
            moved: false,
        });
        unsafe { SetCapture(sh.hwnd) };
    }

    fn mouse_move(&mut self) {
        let Some(drag) = self.shell.drag.as_mut() else {
            return;
        };
        let p = cursor_pos();
        if !drag.moved
            && ((p.x - drag.start.x).abs() > DRAG_THRESHOLD
                || (p.y - drag.start.y).abs() > DRAG_THRESHOLD)
        {
            drag.moved = true;
            self.core.dog.grab();
        }
        if drag.moved {
            self.core
                .dog
                .drag_to(p.x as f32 - drag.grab_dx, p.y as f32 - drag.grab_dy);
        }
    }

    fn mouse_up(&mut self, client: POINT) {
        let Some(drag) = self.shell.drag.take() else {
            return;
        };
        unsafe { ReleaseCapture() };
        if drag.moved {
            // Falls onto the ground of the screen it was dropped on.
            let ground = self.bounds().ground;
            if self.core.dog.y > ground {
                self.core.dog.y = ground;
            }
            self.core.dog.release();
            return;
        }
        self.core
            .click(&mut self.shell, client.x as f32, client.y as f32);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let client = || POINT {
        x: (lp & 0xffff) as i16 as i32,
        y: ((lp >> 16) & 0xffff) as i16 as i32,
    };
    match msg {
        WM_TIMER if wp == TIMER_ID => {
            with_app(App::tick);
            0
        }
        WM_LBUTTONDOWN => {
            if with_app(|a| a.shell.alone.hwnd == hwnd) == Some(true) {
                return 0;
            }
            if with_app(|a| a.shell.toy.hwnd == hwnd) == Some(true) {
                with_app(App::toy_down);
                return 0;
            }
            with_app(|a| a.mouse_down(client()));
            0
        }
        WM_MOUSEMOVE => {
            with_app(|a| {
                a.toy_move();
                a.mouse_move();
            });
            0
        }
        WM_LBUTTONUP => {
            if with_app(|a| a.shell.alone.hwnd == hwnd) == Some(true) {
                with_app(|a| a.alone_click(client()));
                return 0;
            }
            if with_app(|a| a.shell.toy.hwnd == hwnd) == Some(true) {
                with_app(App::toy_up);
                return 0;
            }
            with_app(|a| a.mouse_up(client()));
            0
        }
        WM_CAPTURECHANGED => {
            with_app(|a| {
                if a.shell.drag.take().is_some_and(|d| d.moved) {
                    a.core.dog.release();
                }
                a.toy_up();
            });
            0
        }
        WM_RBUTTONUP => {
            // Right click on the toy: put it away.
            if with_app(|a| a.shell.toy.hwnd == hwnd) == Some(true) {
                with_app(|a| a.core.put_toy_away());
                return 0;
            }
            // The menu runs its own modal loop (WM_TIMER still arrives), so it must not be
            // shown while App is borrowed.
            if let Some(menu) = with_app(|a| a.shell.menu.clone()) {
                unsafe { menu.show_context_menu_for_hwnd(hwnd as isize, None) };
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => {
            // The dog's window is much bigger than the dog (room for the bubble and hats):
            // only what is drawn takes the mouse; elsewhere it goes to the windows behind.
            let p = POINT {
                x: (lp & 0xffff) as i16 as i32,
                y: ((lp >> 16) & 0xffff) as i16 as i32,
            };
            let over = with_app(|a| {
                let sh = &a.shell;
                if sh.hwnd != hwnd || sh.drag.is_some() {
                    return true;
                }
                let px = a.core.scale(monitor_scale(p)).px as i32;
                dost::render::drawn_near(&sh.pixmap, p.x - sh.win_pos.x, p.y - sh.win_pos.y, 2 * px)
            });
            if over == Some(false) {
                HTTRANSPARENT as LRESULT
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe fn create_window() -> HWND {
    let class: Vec<u16> = "DostDogWindow\0".encode_utf16().collect();
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            // The paw from dost.exe's resources (build.rs), id 1.
            hIcon: LoadIconW(hinstance, 1 as _),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    }
}

fn cursor_pos() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    p
}

fn monitor_info(pt: POINT) -> MONITORINFO {
    hmonitor_info(unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) })
}

/// Work area excluding the taskbar; `bottom` is the taskbar's top edge.
fn work_area(pt: POINT) -> RECT {
    monitor_info(pt).rcWork
}

fn monitor_rect(pt: POINT) -> RECT {
    monitor_info(pt).rcMonitor
}

fn monitor_scale(pt: POINT) -> f32 {
    hmonitor_scale(unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) })
}

fn hmonitor_scale(mon: HMONITOR) -> f32 {
    let (mut dx, mut dy) = (96u32, 96u32);
    if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } != 0 {
        return 1.0;
    }
    dx as f32 / 96.0
}

fn hmonitor_info(mon: HMONITOR) -> MONITORINFO {
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe { GetMonitorInfoW(mon, &mut mi) };
    mi
}

/// The monitors, primary first, then the others in `EnumDisplayMonitors` order.
fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn add(
        mon: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows_sys::core::BOOL {
        unsafe { (*(data as *mut Vec<HMONITOR>)).push(mon) };
        1
    }
    let mut all: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(add),
            &mut all as *mut Vec<HMONITOR> as LPARAM,
        );
    }
    if let Some(i) = all
        .iter()
        .position(|&m| hmonitor_info(m).dwFlags & MONITORINFOF_PRIMARY != 0)
    {
        let primary = all.remove(i);
        all.insert(0, primary);
    }
    all
}

/// Off-screen DIB for `UpdateLayeredWindow`.
struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: u32,
    h: u32,
}

impl Default for Dib {
    fn default() -> Self {
        Dib {
            dc: std::ptr::null_mut(),
            bitmap: std::ptr::null_mut(),
            old: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            w: 0,
            h: 0,
        }
    }
}

impl Dib {
    unsafe fn ensure(&mut self, w: u32, h: u32) {
        if self.w == w && self.h == h && !self.bits.is_null() {
            return;
        }
        unsafe {
            self.free();
            self.dc = CreateCompatibleDC(std::ptr::null_mut());
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            self.bitmap = CreateDIBSection(
                self.dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            self.bits = bits as *mut u8;
            self.old = SelectObject(self.dc, self.bitmap);
            self.w = w;
            self.h = h;
        }
    }

    unsafe fn present(&mut self, hwnd: HWND, pm: &Pixmap, pos: POINT) {
        let (w, h) = (pm.width(), pm.height());
        unsafe {
            self.ensure(w, h);
            if self.bits.is_null() {
                return;
            }
            // tiny-skia: premultiplied RGBA, GDI: premultiplied BGRA.
            let dst = std::slice::from_raw_parts_mut(self.bits, (w * h * 4) as usize);
            for (d, s) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pm.data().as_chunks::<4>().0)
            {
                *d = [s[2], s[1], s[0], s[3]];
            }
            let size = SIZE {
                cx: w as i32,
                cy: h as i32,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd,
                std::ptr::null_mut(),
                &pos,
                &size,
                self.dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
        }
    }

    unsafe fn free(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                SelectObject(self.dc, self.old);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
        // Can't use `*self = Dib::default()`: it drops the old value, which would call `free` again.
        self.dc = std::ptr::null_mut();
        self.bitmap = std::ptr::null_mut();
        self.old = std::ptr::null_mut();
        self.bits = std::ptr::null_mut();
        self.w = 0;
        self.h = 0;
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe { self.free() };
    }
}

/// "Start with Windows": a value under HKCU\...\Run.
mod autostart {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "Dost";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open(write: bool) -> Option<HKEY> {
        let mut key: HKEY = std::ptr::null_mut();
        let access = if write {
            KEY_SET_VALUE | KEY_QUERY_VALUE
        } else {
            KEY_QUERY_VALUE
        };
        let r =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(KEY).as_ptr(), 0, access, &mut key) };
        (r == ERROR_SUCCESS).then_some(key)
    }

    pub fn enabled() -> bool {
        let Some(key) = open(false) else { return false };
        let r = unsafe {
            RegQueryValueExW(
                key,
                wide(NAME).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        unsafe { RegCloseKey(key) };
        r == ERROR_SUCCESS
    }

    pub fn set(on: bool) -> Result<(), String> {
        let key = open(true).ok_or("could not open the registry")?;
        let r = if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = wide(&format!("\"{}\"", exe.display()));
            unsafe {
                RegSetValueExW(
                    key,
                    wide(NAME).as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(key, wide(NAME).as_ptr()) }
        };
        unsafe { RegCloseKey(key) };
        if r == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("registry error {r}"))
        }
    }
}

mod clipboard {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;

    const CF_UNICODETEXT: u32 = 13;

    pub fn get_text() -> Option<String> {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            let mut out = None;
            if !h.is_null() {
                let p = GlobalLock(h) as *const u16;
                if !p.is_null() {
                    let mut n = 0;
                    while *p.add(n) != 0 {
                        n += 1;
                    }
                    out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                    GlobalUnlock(h);
                }
            }
            CloseClipboard();
            out
        }
    }

    pub fn set_text(text: &str) -> bool {
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return false;
            }
            EmptyClipboard();
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            let mut ok = false;
            if !mem.is_null() {
                let p = GlobalLock(mem) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                    GlobalUnlock(mem);
                    ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
                }
            }
            CloseClipboard();
            ok
        }
    }
}

mod sound {
    use std::sync::Mutex;

    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    /// Data played with `SND_ASYNC | SND_MEMORY` must live until playback ends;
    /// the last played sound is kept here (a new call stops the previous one).
    static PLAYING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    pub fn play(wav: Vec<u8>) {
        let Ok(mut slot) = PLAYING.lock() else { return };
        unsafe {
            // Stop the old one first, then swap the buffer.
            PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
            *slot = Some(wav);
            if let Some(buf) = slot.as_ref() {
                PlaySoundW(
                    buf.as_ptr().cast(),
                    std::ptr::null_mut(),
                    SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
                );
            }
        }
    }
}

/// A window that shows a [`Panel`]: either the popover (a borderless, per-pixel-alpha popup
/// above the tray icon that closes when it loses focus, like a flyout) or the big settings
/// window (a normal, resizable top-level window painted in `WM_PAINT`).
#[derive(Default)]
struct PanelWindow {
    hwnd: Option<HWND>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    dib: Dib,
    /// The settings window's last frame as BGRA, for `WM_PAINT`.
    bgra: Vec<u8>,
    pos: POINT,
    redraw: bool,
    tracking: bool,
}

/// `WM_MOUSELEAVE` (winuser.h); windows-sys keeps it in a module we do not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
const PANEL_CLASS: &str = "DostPanelWindow";
const SETTINGS_CLASS: &str = "DostSettingsWindow";
/// Gap between the panel and the taskbar / screen edge.
const PANEL_MARGIN: i32 = 12;
/// The settings window's smallest client size (logical).
const SETTINGS_MIN: (f32, f32) = (640.0, 460.0);
const VK_BACK: WPARAM = 0x08;
const VK_RETURN: WPARAM = 0x0D;
const VK_ESCAPE: WPARAM = 0x1B;

impl PanelWindow {
    fn is_open(&self) -> bool {
        self.hwnd.is_some()
    }

    fn open(&mut self, anchor: POINT) {
        let ui = monitor_scale(anchor);
        let mut panel = Panel::new(ui);
        let mon = monitor_info(anchor);
        let work = mon.rcWork;
        let margin = (PANEL_MARGIN as f32 * ui) as i32;
        let (w, h) = panel.size((work.bottom - work.top - 2 * margin).max(200) as u32);
        // Above the tray icon when the taskbar is at the bottom; clamp into the work area.
        let x = (anchor.x - w as i32 / 2).clamp(work.left + margin, work.right - w as i32 - margin);
        let y = if anchor.y >= work.bottom - margin {
            work.bottom - h as i32 - margin
        } else {
            (anchor.y + margin).clamp(work.top + margin, work.bottom - h as i32 - margin)
        };
        panel.set_ui_scale(ui);
        let hwnd = unsafe { create_panel_window() };
        if hwnd.is_null() {
            return;
        }
        self.hwnd = Some(hwnd);
        self.pixmap = Pixmap::new(w, h);
        self.panel = Some(panel);
        self.pos = POINT { x, y };
        self.redraw = true;
        self.tracking = false;
    }

    /// Opens the big settings window, centred on the screen under the cursor.
    fn open_window(&mut self) {
        let at = cursor_pos();
        let ui = monitor_scale(at);
        let work = work_area(at);
        let mut panel = Panel::new_window(ui);
        let (w, h) = panel.size((work.bottom - work.top).max(200) as u32);
        let dpi = (ui * 96.0).round() as u32;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        unsafe { AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, 0, WS_EX_APPWINDOW, dpi) };
        let (ww, wh) = (
            (r.right - r.left).min(work.right - work.left),
            (r.bottom - r.top).min(work.bottom - work.top),
        );
        let x = work.left + (work.right - work.left - ww) / 2;
        let y = work.top + (work.bottom - work.top - wh) / 2;
        let hwnd = unsafe { create_settings_window(x, y, ww, wh) };
        if hwnd.is_null() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            panel.set_ui_scale(dpi as f32 / 96.0);
        }
        self.hwnd = Some(hwnd);
        self.panel = Some(panel);
        self.pixmap = None;
        self.bgra.clear();
        self.pos = POINT { x: 0, y: 0 };
        self.tracking = false;
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        // `WM_SIZE` came while the app was busy (we are called from inside it): size here.
        self.fit_client();
    }

    /// The settings window: makes the pixmap the size of the client area.
    fn fit_client(&mut self) {
        let Some(hwnd) = self.hwnd else { return };
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe { GetClientRect(hwnd, &mut r) };
        self.resize(r.right - r.left, r.bottom - r.top);
    }

    fn resize(&mut self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (w, h) = (w as u32, h as u32);
        if self.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
            self.pixmap = Pixmap::new(w, h);
        }
        self.redraw = true;
    }

    fn close(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            unsafe { DestroyWindow(hwnd) };
        }
        self.panel = None;
        self.pixmap = None;
        self.bgra = Vec::new();
    }

    fn paint(&mut self, r: &mut Renderer, view: &panel::View) {
        let (Some(hwnd), Some(panel), Some(pm)) =
            (self.hwnd, self.panel.as_mut(), self.pixmap.as_mut())
        else {
            return;
        };
        panel.draw(pm, r, view);
        if panel.is_window() {
            // Opaque: premultiplied RGBA is plain RGBA; GDI wants BGRA.
            self.bgra.clear();
            self.bgra.reserve(pm.data().len());
            for s in pm.data().as_chunks::<4>().0 {
                self.bgra.extend_from_slice(&[s[2], s[1], s[0], 255]);
            }
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            return;
        }
        unsafe {
            self.dib.present(hwnd, pm, self.pos);
            ShowWindow(hwnd, SW_SHOW);
        }
    }

    /// The settings window's `WM_PAINT`: copies the last frame to the window.
    unsafe fn blit(&self, dc: HDC) {
        let Some(pm) = self.pixmap.as_ref() else {
            return;
        };
        let (w, h) = (pm.width(), pm.height());
        if self.bgra.len() != (w * h * 4) as usize {
            return;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..unsafe { std::mem::zeroed() }
            },
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            SetDIBitsToDevice(
                dc,
                0,
                0,
                w,
                h,
                0,
                0,
                0,
                h,
                self.bgra.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
            );
        }
    }

    fn height(&self) -> u32 {
        self.pixmap.as_ref().map_or(0, Pixmap::height)
    }
}

unsafe fn register_class(name: &[u16], proc: WNDPROC) -> HINSTANCE {
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc,
            hInstance: hinstance,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            // The paw from dost.exe's resources (build.rs), id 1.
            hIcon: LoadIconW(hinstance, 1 as _),
            lpszClassName: name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        hinstance
    }
}

unsafe fn create_panel_window() -> HWND {
    let class: Vec<u16> = PANEL_CLASS.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hinstance = register_class(&class, Some(panel_proc));
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if !hwnd.is_null() {
            SetForegroundWindow(hwnd);
        }
        hwnd
    }
}

unsafe fn create_settings_window(x: i32, y: i32, w: i32, h: i32) -> HWND {
    let class: Vec<u16> = SETTINGS_CLASS.encode_utf16().chain(Some(0)).collect();
    let title = if dost::i18n::is_turkish() {
        "Dost Ayarları"
    } else {
        "Dost Settings"
    };
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hinstance = register_class(&class, Some(settings_proc));
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    }
}

/// The key a `WM_KEYDOWN` / `WM_CHAR` message means for a panel's text field (Esc is left to
/// the caller); Ctrl+V arrives as `WM_CHAR` 0x16: paste.
fn panel_key_of(msg: u32, wp: WPARAM) -> Option<panel::Key> {
    match (msg, wp) {
        (WM_KEYDOWN, VK_BACK) => Some(panel::Key::Backspace),
        (WM_KEYDOWN, VK_RETURN) => Some(panel::Key::Enter),
        (WM_CHAR, 0x16) => clipboard::get_text().map(panel::Key::Text),
        (WM_CHAR, _) => char::from_u32(wp as u32)
            .filter(|c| !c.is_control())
            .map(|c| panel::Key::Text(c.to_string())),
        _ => None,
    }
}

/// Mouse and keyboard for either panel window; `None` for messages it does not handle.
fn panel_input(which: Which, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let (x, y) = (
        (lp & 0xffff) as i16 as f32,
        ((lp >> 16) & 0xffff) as i16 as f32,
    );
    // Actions may close the panel (and this window), so they run after the panel's borrow.
    let act = |action: Option<panel::Action>| {
        if let Some(action) = action {
            with_app(|a| a.panel_action(which, action));
        }
    };
    match msg {
        WM_MOUSEMOVE => {
            let action = with_app(|a| {
                let settings = a.core.settings.clone();
                let w = a.shell.surface(which);
                if !w.tracking {
                    w.tracking = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tme) };
                }
                let (redraw, action) = w
                    .panel
                    .as_mut()
                    .map_or((false, None), |p| p.pointer_move(x, y, &settings));
                w.redraw |= redraw;
                action
            })
            .flatten();
            act(action);
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                let w = a.shell.surface(which);
                w.tracking = false;
                if w.panel.as_mut().is_some_and(Panel::pointer_leave) {
                    w.redraw = true;
                }
            });
        }
        WM_LBUTTONDOWN => {
            unsafe { SetCapture(hwnd) };
            let action = with_app(|a| {
                let w = a.shell.surface(which);
                w.redraw = true;
                w.panel.as_mut().and_then(|p| p.pointer_down(x, y))
            })
            .flatten();
            act(action);
        }
        WM_LBUTTONUP => {
            unsafe { ReleaseCapture() };
            let action = with_app(|a| {
                let view = a.core.panel_view(&a.shell);
                a.shell
                    .surface(which)
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_up(x, y, &view))
            })
            .flatten();
            act(action);
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16 as f32 / 120.0;
            with_app(|a| {
                let w = a.shell.surface(which);
                let h = w.height();
                if let Some(p) = w.panel.as_mut() {
                    p.scroll_by(delta, h);
                    w.redraw = true;
                }
            });
        }
        WM_KEYDOWN if wp == VK_ESCAPE => {
            // Esc: cancels editing a field, or closes the popover.
            with_app(|a| {
                let w = a.shell.surface(which);
                if w.panel.as_ref().is_some_and(Panel::is_editing) {
                    a.panel_key(which, panel::Key::Escape);
                } else if which == Which::Popover {
                    a.shell.panel.close();
                }
            });
        }
        WM_KEYDOWN | WM_CHAR => {
            let key = panel_key_of(msg, wp)?;
            with_app(|a| a.panel_key(which, key));
        }
        _ => return None,
    }
    Some(0)
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = panel_input(Which::Popover, hwnd, msg, wp, lp) {
        return r;
    }
    match msg {
        WM_ACTIVATE if (wp & 0xffff) as u32 == WA_INACTIVE => {
            // Clicked elsewhere: close like a flyout. Deferred so we are not inside DestroyWindow.
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            0
        }
        WM_CLOSE => {
            with_app(|a| {
                if a.shell.panel.hwnd == Some(hwnd) {
                    a.close_panel();
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = panel_input(Which::Settings, hwnd, msg, wp, lp) {
        return r;
    }
    match msg {
        WM_GETMINMAXINFO => {
            let dpi = match unsafe { GetDpiForWindow(hwnd) } {
                0 => 96,
                d => d,
            };
            let k = dpi as f32 / 96.0;
            let mut r = RECT {
                left: 0,
                top: 0,
                right: (SETTINGS_MIN.0 * k) as i32,
                bottom: (SETTINGS_MIN.1 * k) as i32,
            };
            unsafe {
                AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, 0, WS_EX_APPWINDOW, dpi);
                let mmi = &mut *(lp as *mut MINMAXINFO);
                mmi.ptMinTrackSize = POINT {
                    x: r.right - r.left,
                    y: r.bottom - r.top,
                };
            }
            0
        }
        WM_SIZE => {
            if wp as u32 != SIZE_MINIMIZED {
                let (w, h) = ((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
                with_app(|a| {
                    if a.shell.settings.hwnd == Some(hwnd) {
                        a.shell.settings.resize(w, h);
                        // Right away, so live resizing does not show stale frames.
                        a.paint_panel(Which::Settings);
                    }
                });
            }
            0
        }
        WM_DPICHANGED => {
            let ui = (wp & 0xffff) as f32 / 96.0;
            with_app(|a| {
                if let Some(p) = a.shell.settings.panel.as_mut() {
                    p.set_ui_scale(ui);
                    a.shell.settings.redraw = true;
                }
            });
            // The suggested rect; its WM_SIZE needs the app, so not inside `with_app`.
            let r = unsafe { &*(lp as *const RECT) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                with_app(|a| a.shell.settings.blit(dc));
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_CLOSE => {
            with_app(|a| {
                if a.shell.settings.hwnd == Some(hwnd) {
                    a.close_window(Which::Settings);
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
