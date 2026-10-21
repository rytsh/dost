//! macOS shell: a borderless, transparent, always-on-top window for the dog above the Dock,
//! a menu bar icon (left click: settings panel, right click: short menu) and the settings
//! panel window. Behaviour lives in [`dost::app::Core`].
//!
//! Frames are rendered with tiny-skia into a pixmap and shown through an `NSImageView`.
//! AppKit uses a y-up coordinate system with the origin at the bottom-left of the main
//! screen; the core uses y-down physical pixels, so everything here converts between them.

use std::cell::RefCell;
use std::path::Path;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapFormat,
    NSBitmapImageRep, NSColor, NSEvent, NSEventMask, NSEventType, NSImage, NSImageScaling,
    NSImageView, NSPasteboard, NSPasteboardTypeString, NSScreen, NSSound, NSStatusWindowLevel,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL};
use tiny_skia::Pixmap;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use dost::app::{self, Core, EDGE, Platform};
use dost::dog::Bounds;
use dost::panel::{self, Panel};
use dost::render::{Renderer, Scale};
use dost::sound::{BARK_WAV, scaled_wav};

const FRAME: f64 = 1.0 / 60.0;
const DRAG_THRESHOLD: f64 = 4.0;
/// Gap between the panel and the menu bar / screen edge, points.
const PANEL_MARGIN: f64 = 8.0;

/// A rectangle in the core's coordinates: physical pixels, y down from the top of the
/// main screen.
#[derive(Clone, Copy, Debug)]
struct PxRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    /// Backing scale factor of the screen (2.0 on Retina).
    scale: f64,
}

struct Drag {
    start: NSPoint,
    grab_dx: f32,
    grab_dy: f32,
    moved: bool,
}

struct App {
    core: Core,
    shell: Shell,
}

struct Shell {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    pixmap: Pixmap,
    drag: Option<Drag>,
    _tray: TrayIcon,
    items: MenuItems,
    panel: PanelWindow,
    sound: Option<Retained<NSSound>>,
    quit: bool,
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
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let ns_app = NSApplication::sharedApplication(mtm);
    // Menu bar app: no Dock icon, no app menu.
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let renderer = Renderer::new(true, Vec::new());
    let (menu, items) = build_menu();
    let icon = Icon::from_rgba(renderer.icon_rgba(36), 36, 36).expect("tray icon");
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Dost")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .expect("menu bar icon");

    let (window, view) = make_window(mtm, false);
    window.setIgnoresMouseEvents(false);
    window.orderFrontRegardless();

    let screen = main_screen(mtm);
    let start_x = screen.left + (screen.right - screen.left) * 0.75;
    let core = Core::new(renderer, start_x as f32, screen.bottom as f32);
    let shell = Shell {
        mtm,
        window,
        view,
        pixmap: Pixmap::new(1, 1).unwrap(),
        drag: None,
        _tray: tray,
        items,
        panel: PanelWindow::default(),
        sound: None,
        quit: false,
    };
    APP.with(|a| *a.borrow_mut() = Some(App { core, shell }));
    with_app(App::sync_menu);

    install_mouse_monitor();
    let tick = RcBlock::new(|_timer: NonNull<NSTimer>| {
        let quit = with_app(|a| {
            a.tick();
            a.shell.quit
        });
        if quit == Some(true) {
            let mtm = MainThreadMarker::new().unwrap();
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    });
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(FRAME, true, &tick) };
    ns_app.run();
}

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

/// A borderless, transparent, shadowless window with an image view filling it.
fn make_window(mtm: MainThreadMarker, panel: bool) -> (Retained<NSWindow>, Retained<NSImageView>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(panel);
    window.setLevel(NSStatusWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.setAcceptsMouseMovedEvents(true);
    let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
    view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
    window.setContentView(Some(&view));
    (window, view)
}

/// Converts a pixmap (premultiplied RGBA) to an `NSImage` of `points` size.
fn to_image(pm: &Pixmap, points: NSSize) -> Option<Retained<NSImage>> {
    let (w, h) = (pm.width() as isize, pm.height() as isize);
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            &NSString::from_str("NSDeviceRGBColorSpace"),
            NSBitmapFormat::empty(),
            w * 4,
            32,
        )?
    };
    let dst = rep.bitmapData();
    if dst.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(pm.data().as_ptr(), dst, pm.data().len()) };
    rep.setSize(points);
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&rep);
    Some(image)
}

/// Every screen as (visible area, full area) in core coordinates.
fn screens(mtm: MainThreadMarker) -> Vec<(PxRect, PxRect)> {
    let all = NSScreen::screens(mtm);
    let Some(main) = all.firstObject() else {
        return Vec::new();
    };
    let main_h = main.frame().size.height;
    all.iter()
        .map(|s| {
            let k = s.backingScaleFactor();
            let conv = |r: NSRect| PxRect {
                left: r.origin.x * k,
                top: (main_h - r.origin.y - r.size.height) * k,
                right: (r.origin.x + r.size.width) * k,
                bottom: (main_h - r.origin.y) * k,
                scale: k,
            };
            (conv(s.visibleFrame()), conv(s.frame()))
        })
        .collect()
}

fn main_screen(mtm: MainThreadMarker) -> PxRect {
    screens(mtm).first().map_or(
        PxRect {
            left: 0.0,
            top: 0.0,
            right: 1440.0,
            bottom: 900.0,
            scale: 1.0,
        },
        |(visible, _)| *visible,
    )
}

/// The screen containing the physical point (x, y), or the nearest one.
fn screen_at(mtm: MainThreadMarker, x: f64, y: f64) -> (PxRect, PxRect) {
    let all = screens(mtm);
    let dist = |r: &PxRect| {
        let dx = (r.left - x).max(0.0).max(x - r.right);
        let dy = (r.top - y).max(0.0).max(y - r.bottom);
        dx * dx + dy * dy
    };
    all.iter()
        .copied()
        .min_by(|a, b| dist(&a.1).total_cmp(&dist(&b.1)))
        .unwrap_or((main_screen(mtm), main_screen(mtm)))
}

/// Main-screen height in points (AppKit y flips around it).
fn main_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(900.0, |s| s.frame().size.height)
}

/// Places `window` so its top-left is at physical (x, y) and it is `w`×`h` physical pixels.
fn place(mtm: MainThreadMarker, window: &NSWindow, x: f64, y: f64, w: f64, h: f64, scale: f64) {
    let (pw, ph) = (w / scale, h / scale);
    let origin = NSPoint::new(x / scale, main_height(mtm) - y / scale - ph);
    window.setFrame_display(NSRect::new(origin, NSSize::new(pw, ph)), false);
}

/// The mouse in core coordinates (physical pixels, y down).
fn mouse_px(mtm: MainThreadMarker) -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    let y = main_height(mtm) - p.y;
    let (_, full) = screen_at(mtm, p.x, y);
    (p.x * full.scale, y * full.scale)
}

impl Platform for Shell {
    fn play_bark(&mut self, volume: u8) {
        let Some(wav) = scaled_wav(BARK_WAV, volume) else {
            return;
        };
        let data = NSData::with_bytes(&wav);
        if let Some(old) = self.sound.take() {
            old.stop();
        }
        if let Some(s) = NSSound::initWithData(NSSound::alloc(), &data) {
            s.play();
            self.sound = Some(s);
        }
    }

    fn copy_text(&mut self, text: &str) -> bool {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    }

    fn autostart_enabled(&self) -> bool {
        autostart::enabled()
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        autostart::set(on)
    }

    fn open_folder(&mut self, path: &Path) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace().openURL(&url);
    }

    fn set_dog_visible(&mut self, visible: bool) {
        if visible {
            self.window.orderFrontRegardless();
        } else {
            self.window.orderOut(None);
        }
    }

    fn close_panel(&mut self) {
        self.panel.close();
    }

    fn quit(&mut self) {
        self.quit = true;
    }

    fn autostart_label(&self) -> &'static str {
        "Open at login"
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
                // tray-icon reports physical pixels from the top of the main screen.
                let anchor = (
                    rect.position.x + rect.size.width as f64 / 2.0,
                    rect.position.y + rect.size.height as f64,
                );
                self.toggle_panel(Some(anchor));
            }
        }
        self.drag_update();
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
    }

    fn dog_screen(&self) -> (PxRect, PxRect) {
        screen_at(
            self.shell.mtm,
            self.core.dog.x as f64,
            self.core.dog.y as f64 - 10.0,
        )
    }

    fn bounds(&self) -> Bounds {
        let (visible, full) = self.dog_screen();
        let ui = full.scale as f32;
        let px = Scale::new(ui, self.core.settings.size).px as f32;
        Bounds {
            // The visible frame excludes the Dock and the menu bar: its bottom is the Dock's top.
            ground: app::ground_line(
                &self.core.settings,
                visible.bottom as f32,
                full.top as f32,
                full.bottom as f32,
                ui,
                px,
            ),
            left: visible.left as f32 + EDGE * px,
            right: visible.right as f32 - EDGE * px,
            scale: px,
        }
    }

    fn paint(&mut self) {
        let (_, full) = self.dog_screen();
        let scale = self.core.scale(full.scale as f32);
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
        let x = (core.dog.x as f64 - w as f64 / 2.0)
            .clamp(full.left, (full.right - w as f64).max(full.left))
            .round();
        let y = (core.dog.y - Renderer::ground_offset(h, scale)).round() as f64;
        core.hits = core.renderer.draw(
            &mut sh.pixmap,
            core.dog.x - x as f32,
            &frame,
            bubble.as_ref(),
            scale,
        );
        let points = NSSize::new(w as f64 / full.scale, h as f64 / full.scale);
        if let Some(image) = to_image(&sh.pixmap, points) {
            sh.view.setImage(Some(&image));
        }
        place(sh.mtm, &sh.window, x, y, w as f64, h as f64, full.scale);
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

    fn toggle_panel(&mut self, anchor: Option<(f64, f64)>) {
        if self.shell.panel.is_open() {
            self.shell.panel.close();
            return;
        }
        let at = anchor.unwrap_or_else(|| mouse_px(self.shell.mtm));
        self.shell.panel.open(self.shell.mtm, at);
    }

    fn paint_panel(&mut self) {
        if !self.shell.panel.is_open() || !self.shell.panel.redraw {
            return;
        }
        self.shell.panel.redraw = false;
        let view = self.core.panel_view(&self.shell);
        self.shell.panel.paint(&mut self.core.renderer, &view);
    }

    /// Mouse events on our windows, from the local event monitor. Returns true if consumed.
    fn on_mouse(&mut self, kind: NSEventType, window: Option<&NSWindow>, scroll: f64) -> bool {
        let mtm = self.shell.mtm;
        let (mx, my) = mouse_px(mtm);
        let on_dog = window.is_some_and(|w| std::ptr::eq(w, &*self.shell.window));
        let on_panel = window.is_some_and(|w| {
            self.shell
                .panel
                .window
                .as_deref()
                .is_some_and(|p| std::ptr::eq(w, p))
        });

        if on_panel {
            let (px, py) = self.shell.panel.local(mtm, mx, my);
            let p = &mut self.shell.panel;
            let Some(panel) = p.panel.as_mut() else {
                return false;
            };
            match kind {
                NSEventType::MouseMoved | NSEventType::LeftMouseDragged => {
                    let (redraw, action) = panel.pointer_move(px, py, &self.core.settings);
                    p.redraw |= redraw;
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::LeftMouseDown => {
                    let action = panel.pointer_down(px, py);
                    p.redraw = true;
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::LeftMouseUp => {
                    let view = self.core.panel_view(&self.shell);
                    let action = self
                        .shell
                        .panel
                        .panel
                        .as_mut()
                        .and_then(|p| p.pointer_up(px, py, &view));
                    self.shell.panel.redraw = true;
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::ScrollWheel => {
                    let h = p.pixmap.as_ref().map_or(0, Pixmap::height);
                    panel.scroll_by(scroll as f32 / 3.0, h);
                    p.redraw = true;
                }
                _ => return false,
            }
            return true;
        }

        if on_dog {
            match kind {
                NSEventType::LeftMouseDown => {
                    self.shell.drag = Some(Drag {
                        start: NSEvent::mouseLocation(),
                        grab_dx: mx as f32 - self.core.dog.x,
                        grab_dy: my as f32 - self.core.dog.y,
                        moved: false,
                    });
                    return true;
                }
                NSEventType::RightMouseUp => {
                    self.toggle_panel(None);
                    return true;
                }
                _ => {}
            }
        }
        if kind == NSEventType::LeftMouseUp && self.shell.drag.is_some() {
            let drag = self.shell.drag.take().unwrap();
            if drag.moved {
                let ground = self.bounds().ground;
                if self.core.dog.y > ground {
                    self.core.dog.y = ground;
                }
                self.core.dog.release();
            } else {
                // Click position relative to the dog's window, physical pixels.
                let frame = self.shell.window.frame();
                let (_, full) = self.dog_screen();
                let wx = mx - frame.origin.x * full.scale;
                let wy = my - (main_height(mtm) - frame.origin.y - frame.size.height) * full.scale;
                self.core.click(&mut self.shell, wx as f32, wy as f32);
            }
            return true;
        }
        false
    }

    /// While dragging, follow the mouse (events may arrive outside our window).
    fn drag_update(&mut self) {
        let mtm = self.shell.mtm;
        let Some(drag) = self.shell.drag.as_mut() else {
            return;
        };
        let p = NSEvent::mouseLocation();
        if !drag.moved
            && ((p.x - drag.start.x).abs() > DRAG_THRESHOLD
                || (p.y - drag.start.y).abs() > DRAG_THRESHOLD)
        {
            drag.moved = true;
            self.core.dog.grab();
        }
        if drag.moved {
            let (mx, my) = mouse_px(mtm);
            let (dx, dy) = (drag.grab_dx, drag.grab_dy);
            self.core.dog.drag_to(mx as f32 - dx, my as f32 - dy);
        }
        if NSEvent::pressedMouseButtons() & 1 == 0
            && self.shell.drag.as_ref().is_some_and(|d| d.moved)
        {
            self.shell.drag = None;
            let ground = self.bounds().ground;
            if self.core.dog.y > ground {
                self.core.dog.y = ground;
            }
            self.core.dog.release();
        }
    }
}

fn install_mouse_monitor() {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseUp
        | NSEventMask::LeftMouseDragged
        | NSEventMask::MouseMoved
        | NSEventMask::RightMouseUp
        | NSEventMask::ScrollWheel
        | NSEventMask::KeyDown;
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { event.as_ref() };
        let mtm = MainThreadMarker::new().unwrap();
        let kind = ev.r#type();
        if kind == NSEventType::KeyDown && ev.keyCode() == 53 {
            // Esc closes the panel.
            with_app(|a| a.shell.panel.close());
            return event.as_ptr();
        }
        let window = ev.window(mtm);
        let consumed = with_app(|a| a.on_mouse(kind, window.as_deref(), ev.scrollingDeltaY()))
            .unwrap_or(false);
        if consumed {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    std::mem::forget(monitor);

    // Clicking anywhere else closes the panel, like a menu.
    let outside = RcBlock::new(|_event: NonNull<NSEvent>| {
        with_app(|a| a.shell.panel.close());
    });
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
        &outside,
    );
    std::mem::forget(monitor);
}

/// The settings panel: a borderless window with a shadow showing [`Panel`].
#[derive(Default)]
struct PanelWindow {
    window: Option<Retained<NSWindow>>,
    view: Option<Retained<NSImageView>>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    /// Top-left on screen and the screen's scale.
    pos: (f64, f64, f64),
    redraw: bool,
}

impl PanelWindow {
    fn is_open(&self) -> bool {
        self.window.is_some()
    }

    /// Opens below the menu bar icon at physical (`x`, `y`).
    fn open(&mut self, mtm: MainThreadMarker, (x, y): (f64, f64)) {
        let (visible, full) = screen_at(mtm, x, y);
        let k = full.scale;
        let panel = Panel::new(k as f32);
        let margin = PANEL_MARGIN * k;
        let (w, h) = panel.size((visible.bottom - visible.top - 2.0 * margin).max(200.0) as u32);
        let px =
            (x - w as f64 / 2.0).clamp(visible.left + margin, visible.right - w as f64 - margin);
        let py = (visible.top + margin)
            .max(y + margin)
            .min(visible.bottom - h as f64 - margin);
        let (window, view) = make_window(mtm, true);
        place(mtm, &window, px, py, w as f64, h as f64, k);
        window.orderFrontRegardless();
        self.window = Some(window);
        self.view = Some(view);
        self.pixmap = Pixmap::new(w, h);
        self.panel = Some(panel);
        self.pos = (px, py, k);
        self.redraw = true;
    }

    fn close(&mut self) {
        if let Some(w) = self.window.take() {
            w.orderOut(None);
            w.close();
        }
        self.view = None;
        self.panel = None;
        self.pixmap = None;
    }

    /// Physical mouse position relative to the panel's top-left.
    fn local(&self, _mtm: MainThreadMarker, mx: f64, my: f64) -> (f32, f32) {
        ((mx - self.pos.0) as f32, (my - self.pos.1) as f32)
    }

    fn paint(&mut self, r: &mut Renderer, view: &panel::View) {
        let (Some(v), Some(panel), Some(pm)) = (
            self.view.as_ref(),
            self.panel.as_mut(),
            self.pixmap.as_mut(),
        ) else {
            return;
        };
        panel.draw(pm, r, view);
        let k = self.pos.2;
        if let Some(image) = to_image(
            pm,
            NSSize::new(pm.width() as f64 / k, pm.height() as f64 / k),
        ) {
            v.setImage(Some(&image));
        }
    }
}

/// "Open at login": a LaunchAgent that starts this executable.
mod autostart {
    use std::path::PathBuf;

    const LABEL: &str = "com.rytsh.dost";

    fn plist() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist"))
        })
    }

    pub fn enabled() -> bool {
        plist().is_some_and(|p| p.exists())
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = plist().ok_or("HOME is not set")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;");
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, xml).map_err(|e| e.to_string())
    }
}
