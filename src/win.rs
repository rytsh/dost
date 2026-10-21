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
    _tray: TrayIcon,
    menu: Menu,
    items: MenuItems,
    panel: PanelWindow,
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
    let icon = Icon::from_rgba(renderer.icon_rgba(32), 32, 32).expect("tray icon");
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
        _tray: tray,
        menu,
        items,
        panel: PanelWindow::default(),
    };
    APP.with(|a| *a.borrow_mut() = Some(App { core, shell }));
    with_app(App::sync_menu);

    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_ID, FRAME_MS, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
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
    fn play_bark(&mut self, volume: u8) {
        sound::play(volume);
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
}

impl App {
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
        if self.core.visible {
            self.paint();
        }
        if self.core.panel_dirty {
            self.core.panel_dirty = false;
            self.sync_menu();
        }
        self.paint_panel();

        let now = std::time::Instant::now();
        if now - self.shell.last_topmost > Duration::from_secs(2) {
            // Clicking the taskbar raises it above us; put the dog back on top.
            self.shell.last_topmost = now;
            unsafe {
                SetWindowPos(
                    self.shell.hwnd,
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

    fn bounds(&self) -> Bounds {
        let pt = self.dog_point();
        let work = work_area(pt);
        let mon = monitor_rect(pt);
        let ui = monitor_scale(pt);
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
        let bubble = core.shown.as_ref().map(|s| dost::render::Bubble {
            title: &s.note.title,
            body: &s.note.body,
            pending: core.queue.len(),
        });
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
        self.core.on_action(&mut self.shell, action);
        self.sync_menu();
    }

    /// Opens the panel next to `anchor` (the tray icon), or near the cursor; closes it if open.
    fn toggle_panel(&mut self, anchor: Option<POINT>) {
        if self.shell.panel.is_open() {
            self.shell.panel.close();
            return;
        }
        self.shell.panel.open(anchor.unwrap_or_else(cursor_pos));
        self.shell.panel.redraw = true;
    }

    fn paint_panel(&mut self) {
        if !self.shell.panel.is_open() || !self.shell.panel.redraw {
            return;
        }
        self.shell.panel.redraw = false;
        let view = self.core.panel_view(&self.shell);
        self.shell.panel.paint(&mut self.core.renderer, &view);
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
            with_app(|a| a.mouse_down(client()));
            0
        }
        WM_MOUSEMOVE => {
            with_app(App::mouse_move);
            0
        }
        WM_LBUTTONUP => {
            with_app(|a| a.mouse_up(client()));
            0
        }
        WM_CAPTURECHANGED => {
            with_app(|a| {
                if a.shell.drag.take().is_some_and(|d| d.moved) {
                    a.core.dog.release();
                }
            });
            0
        }
        WM_RBUTTONUP => {
            // The menu runs its own modal loop (WM_TIMER still arrives), so it must not be
            // shown while App is borrowed.
            if let Some(menu) = with_app(|a| a.shell.menu.clone()) {
                unsafe { menu.show_context_menu_for_hwnd(hwnd as isize, None) };
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
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
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..std::mem::zeroed()
        };
        GetMonitorInfoW(mon, &mut mi);
        mi
    }
}

/// Work area excluding the taskbar; `bottom` is the taskbar's top edge.
fn work_area(pt: POINT) -> RECT {
    monitor_info(pt).rcWork
}

fn monitor_rect(pt: POINT) -> RECT {
    monitor_info(pt).rcMonitor
}

fn monitor_scale(pt: POINT) -> f32 {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
            return 1.0;
        }
        dx as f32 / 96.0
    }
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
            for (d, s) in dst.chunks_exact_mut(4).zip(pm.data().chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
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

    pub fn play(volume: u8) {
        if volume == 0 {
            return;
        }
        let Some(wav) = dost::sound::scaled_wav(dost::sound::BARK_WAV, volume) else {
            return;
        };
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

/// Settings panel window: a borderless, per-pixel-alpha popup that shows [`Panel`].
/// It closes when it loses focus, like a flyout.
#[derive(Default)]
struct PanelWindow {
    hwnd: Option<HWND>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    dib: Dib,
    pos: POINT,
    redraw: bool,
    tracking: bool,
}

/// `WM_MOUSELEAVE` (winuser.h); windows-sys keeps it in a module we do not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
const PANEL_CLASS: &str = "DostPanelWindow";
/// Gap between the panel and the taskbar / screen edge.
const PANEL_MARGIN: i32 = 12;

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

    fn close(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            unsafe { DestroyWindow(hwnd) };
        }
        self.panel = None;
        self.pixmap = None;
    }

    fn paint(&mut self, r: &mut Renderer, view: &panel::View) {
        let (Some(hwnd), Some(panel), Some(pm)) =
            (self.hwnd, self.panel.as_mut(), self.pixmap.as_mut())
        else {
            return;
        };
        panel.draw(pm, r, view);
        unsafe {
            self.dib.present(hwnd, pm, self.pos);
            ShowWindow(hwnd, SW_SHOW);
        }
    }

    fn height(&self) -> u32 {
        self.pixmap.as_ref().map_or(0, Pixmap::height)
    }
}

unsafe fn create_panel_window() -> HWND {
    let class: Vec<u16> = PANEL_CLASS.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(panel_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
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

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pt = || {
        (
            (lp & 0xffff) as i16 as f32,
            ((lp >> 16) & 0xffff) as i16 as f32,
        )
    };
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = pt();
            with_app(|a| {
                if !a.shell.panel.tracking {
                    a.shell.panel.tracking = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tme) };
                }
                let settings = a.core.settings.clone();
                let (redraw, action) = a
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .map_or((false, None), |p| p.pointer_move(x, y, &settings));
                a.shell.panel.redraw |= redraw;
                if let Some(action) = action {
                    a.on_action(action);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                a.shell.panel.tracking = false;
                if a.shell
                    .panel
                    .panel
                    .as_mut()
                    .is_some_and(Panel::pointer_leave)
                {
                    a.shell.panel.redraw = true;
                }
            });
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = pt();
            unsafe { SetCapture(hwnd) };
            with_app(|a| {
                if let Some(action) = a
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_down(x, y))
                {
                    a.on_action(action);
                }
                a.shell.panel.redraw = true;
            });
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = pt();
            unsafe { ReleaseCapture() };
            // Actions may close the panel (and this window), so run them after the borrow.
            let action = with_app(|a| {
                let view = a.core.panel_view(&a.shell);
                a.shell
                    .panel
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_up(x, y, &view))
            })
            .flatten();
            if let Some(action) = action {
                with_app(|a| a.on_action(action));
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16 as f32 / 120.0;
            with_app(|a| {
                let h = a.shell.panel.height();
                if let Some(p) = a.shell.panel.panel.as_mut() {
                    p.scroll_by(delta, h);
                    a.shell.panel.redraw = true;
                }
            });
            0
        }
        WM_KEYDOWN if wp == 0x1B => {
            with_app(|a| a.shell.panel.close());
            0
        }
        WM_ACTIVATE if (wp & 0xffff) as u32 == WA_INACTIVE => {
            // Clicked elsewhere: close like a flyout. Deferred so we are not inside DestroyWindow.
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            0
        }
        WM_CLOSE => {
            with_app(|a| {
                if a.shell.panel.hwnd == Some(hwnd) {
                    a.shell.panel.close();
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
