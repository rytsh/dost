//! Draws the dog (pixel-art sprite atlas) and the speech bubble. Platform-independent.

use std::f32::consts::PI;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Hinting, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{
    Color, ColorU8, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke,
    Transform,
};

use crate::dog::{Anim, DogFrame};

/// Output of `art/pack_atlas.py`: each row is an animation (in `Anim` order), each column a frame.
const ATLAS_PNG: &[u8] = include_bytes!("../assets/dog.png");
/// Text uses the system UI font; these families are tried in order.
const SYSTEM_FAMILIES: [&str; 7] = [
    "Segoe UI",
    "SF Pro Text",
    ".AppleSystemUIFont",
    "Helvetica Neue",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];
include!("../assets/dog_atlas.rs");

/// Logical space reserved for the bubble width.
pub const WIDTH: f32 = 280.0;
const BUBBLE_MAX_TEXT: f32 = 220.0;
const BUBBLE_PAD: f32 = 10.0;
const BUBBLE_GAP: f32 = 2.0;
const BUBBLE_TAIL: f32 = 9.0;
/// Space left above the dog for the "z" letters (logical).
const HEADROOM: f32 = 26.0;

const OUTLINE: ColorU8 = rgb(0x4a, 0x2e, 0x1b);
const INK: ColorU8 = rgb(0x2b, 0x2b, 0x33);
const MUTED: ColorU8 = rgb(0x5d, 0x5d, 0x6b);

const fn rgb(r: u8, g: u8, b: u8) -> ColorU8 {
    ColorU8::from_rgba(r, g, b, 255)
}

fn color(c: ColorU8) -> Color {
    Color::from_rgba8(c.red(), c.green(), c.blue(), c.alpha())
}

/// `ui`: DPI scale for text and the bubble. `px`: how many physical pixels one sprite pixel covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub ui: f32,
    pub px: u32,
    /// Extra multiplier for bubble text (text size from settings, 1.0 = normal).
    pub text: f32,
}

impl Scale {
    /// `size`: dog size from settings, percent (100 = 1:1 at 100% screen scale).
    /// Rounded to a whole number so pixels don't get blurry or uneven.
    pub fn new(ui: f32, size: u32) -> Self {
        Scale {
            ui,
            px: (ui * size as f32 / 100.0).round().max(1.0) as u32,
            text: 1.0,
        }
    }

    pub fn with_text(mut self, text: f32) -> Self {
        self.text = text;
        self
    }

    /// Total scale used for the bubble and text.
    fn bubble(&self) -> f32 {
        self.ui * self.text
    }
}

/// Text to show in the bubble.
pub struct Bubble<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// Number of other notifications waiting in the queue.
    pub pending: usize,
}

/// Regions of the drawn frame needed for hit testing (physical pixels, relative to the window).
#[derive(Clone, Copy, Debug, Default)]
pub struct Hits {
    pub bubble: Option<Rect>,
}

pub struct Renderer {
    atlas: Pixmap,
    /// Height of each animation's highest point above the ground (sprite pixels).
    heights: [u32; Anim::ALL.len()],
    text: Option<Text>,
}

/// Text layout: system fonts, shaping for complex scripts (Arabic, Hindi...),
/// right-to-left text and automatic font fallback for missing characters (CJK, emoji...).
struct Text {
    fonts: FontSystem,
    cache: SwashCache,
    buffer: Buffer,
    /// Content and scale of the last bubble text layout; not laid out again if unchanged.
    laid_out: Option<(String, String, usize, u32)>,
    /// System UI font family used for all text.
    family: String,
    /// x of the leftmost glyph in the layout. Right-to-left lines are aligned to the right of the box;
    /// when drawing they are shifted left by this much.
    left: f32,
}

impl Renderer {
    /// `fonts`: whether to load system fonts (can be disabled in tests and previews).
    /// `extra_fonts`: additional font files (e.g. if the system has no fonts).
    pub fn new(system_fonts: bool, extra_fonts: Vec<Vec<u8>>) -> Self {
        let atlas = Pixmap::decode_png(ATLAS_PNG).expect("assets/dog.png");
        let heights = Anim::ALL.map(|a| sprite_height(&atlas, a));
        let sources = extra_fonts
            .into_iter()
            .map(|d| cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(d)));
        let mut fonts = if system_fonts {
            FontSystem::new_with_fonts(sources)
        } else {
            let mut db = cosmic_text::fontdb::Database::new();
            for s in sources {
                db.load_font_source(s);
            }
            FontSystem::new_with_locale_and_db("en-US".into(), db)
        };
        let text = (!fonts.db().is_empty()).then(|| {
            let mut buffer = Buffer::new(&mut fonts, Metrics::new(13.0, 17.0));
            // Snap glyphs to whole pixels so thresholded edges stay clean.
            buffer.set_hinting(Hinting::Enabled);
            Text {
                fonts,
                cache: SwashCache::new(),
                buffer,
                laid_out: None,
                family: String::new(),
                left: 0.0,
            }
        });
        let mut text = text;
        if let Some(t) = text.as_mut() {
            let db = t.fonts.db();
            t.family = SYSTEM_FAMILIES
                .into_iter()
                .find(|f| {
                    db.faces()
                        .any(|face| face.families.iter().any(|(n, _)| n == f))
                })
                .or_else(|| db.faces().next().map(|f| f.families[0].0.as_str()))
                .unwrap_or("sans-serif")
                .to_string();
        }
        Renderer {
            atlas,
            heights,
            text,
        }
    }

    /// Window size needed for the given content (physical pixels).
    pub fn size(&mut self, anim: Anim, bubble: Option<&Bubble>, s: Scale) -> (u32, u32) {
        let px = s.px as f32;
        let dog = self.heights[anim.row() as usize] as f32 * px;
        let extra = match bubble {
            Some(b) => {
                let (_, h) = self.layout_text(b, s.bubble());
                h + (BUBBLE_PAD * 2.0 + BUBBLE_TAIL + BUBBLE_GAP + 4.0) * s.bubble()
            }
            None => HEADROOM * s.ui,
        };
        let w = (WIDTH * s.bubble()).max((CELL.0 * s.px) as f32 + 8.0);
        let h = Self::below(s) + dog + extra;
        (w.ceil() as u32, h.ceil() as u32)
    }

    fn below(s: Scale) -> f32 {
        ((CELL.1 - ANCHOR.1) * s.px) as f32
    }

    /// Distance of the ground line from the top of the window (physical pixels).
    pub fn ground_offset(height: u32, s: Scale) -> f32 {
        height as f32 - Self::below(s)
    }

    /// Draws the frame. `anchor_x`: x of the dog's feet point inside the window (physical).
    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        anchor_x: f32,
        frame: &DogFrame,
        bubble: Option<&Bubble>,
        s: Scale,
    ) -> Hits {
        pixmap.fill(Color::TRANSPARENT);
        let ground = Self::ground_offset(pixmap.height(), s);
        let ax = anchor_x.round() as i32;
        blit_sprite(
            pixmap,
            &self.atlas,
            frame,
            ax,
            ground.round() as i32,
            s.px as i32,
        );

        if frame.anim == Anim::Sleep {
            self.draw_zzz(pixmap, anchor_x, ground, frame, s);
        }

        let mut hits = Hits::default();
        if let Some(b) = bubble {
            let top = ground - (self.heights[frame.anim.row() as usize] * s.px) as f32;
            hits.bubble = self.draw_bubble(pixmap, anchor_x, top, b, s.bubble());
        }
        hits
    }

    /// Tray icon: first frame of the sitting dog (straight, non-premultiplied RGBA).
    pub fn icon_rgba(&self, size: u32) -> Vec<u8> {
        let (x0, y0, x1, y1) = opaque_box(&self.atlas, Anim::Sit, 0);
        let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let k = size as f32 / w.max(h);
        let mut pm = Pixmap::new(size, size).expect("icon size");
        let t = Transform::from_translate((size as f32 - w * k) / 2.0, size as f32 - h * k)
            .pre_scale(k, k)
            .pre_translate(-(x0 as f32), -(y0 as f32));
        let paint = tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        };
        pm.draw_pixmap(0, 0, self.atlas.as_ref(), &paint, t, None);
        pm.pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }

    fn draw_zzz(&mut self, pm: &mut Pixmap, ax: f32, ground: f32, frame: &DogFrame, s: Scale) {
        let Some(text) = &mut self.text else { return };
        let px = s.px as f32;
        let head_x = ax + 50.0 * px * frame.facing;
        let head_y = ground - self.heights[Anim::Sleep.row() as usize] as f32 * px;
        for i in 0..3 {
            let p = (frame.time * 0.35 + i as f32 / 3.0).fract();
            let x = head_x + p * 16.0 * s.ui * frame.facing;
            let y = head_y - (14.0 + p * 18.0) * s.ui;
            let alpha = ((p * PI).sin() * 0.9 * 255.0) as u8;
            let size = (9.0 + p * 6.0) * s.ui;
            let c = cosmic_text::Color::rgba(MUTED.red(), MUTED.green(), MUTED.blue(), alpha);
            let attrs = Attrs::new()
                .family(Family::Name(&text.family))
                .weight(Weight::BOLD);
            text.buffer.set_metrics(Metrics::new(size, size * 1.2));
            text.buffer.set_size(None, None);
            text.buffer.set_text("z", &attrs, Shaping::Basic, None);
            text.laid_out = None;
            text.left = 0.0;
            text.draw(pm, x, y, c);
        }
    }

    /// Lays out the bubble text and returns (width, height) (physical pixels).
    fn layout_text(&mut self, b: &Bubble, scale: f32) -> (f32, f32) {
        let Some(text) = &mut self.text else {
            return (40.0 * scale, 14.0 * scale);
        };
        let key = (
            b.title.to_string(),
            b.body.to_string(),
            b.pending,
            scale.to_bits(),
        );
        if text.laid_out.as_ref() != Some(&key) {
            let base = Attrs::new()
                .family(Family::Name(&text.family))
                .color(cc(INK));
            let title = base
                .clone()
                .weight(Weight::BOLD)
                .metrics(Metrics::new(15.0 * scale, 20.0 * scale));
            let body = base
                .clone()
                .metrics(Metrics::new(14.0 * scale, 19.0 * scale));
            let more = base
                .clone()
                .color(cc(MUTED))
                .metrics(Metrics::new(12.0 * scale, 17.0 * scale));
            let body_text: String = b.body.chars().take(400).collect();
            let more_text = format!("+{} more", b.pending);
            let mut spans: Vec<(&str, Attrs)> = Vec::new();
            for (t, a) in [
                (b.title, title),
                (body_text.as_str(), body),
                (more_text.as_str(), more),
            ] {
                let skip = t.is_empty() || (std::ptr::eq(t, more_text.as_str()) && b.pending == 0);
                if skip {
                    continue;
                }
                if !spans.is_empty() {
                    spans.push(("\n", a.clone()));
                }
                spans.push((t, a));
            }
            text.buffer
                .set_metrics(Metrics::new(14.0 * scale, 19.0 * scale));
            text.buffer.set_wrap(Wrap::WordOrGlyph);
            text.buffer.set_size(Some(BUBBLE_MAX_TEXT * scale), None);
            text.buffer
                .set_rich_text(spans, &base, Shaping::Advanced, None);
            text.buffer.shape_until_scroll(&mut text.fonts, false);
            text.laid_out = Some(key);
        }
        let (mut x0, mut x1, mut h) = (f32::MAX, 0.0f32, 0.0f32);
        for run in text.buffer.layout_runs() {
            for g in run.glyphs {
                x0 = x0.min(g.x);
                x1 = x1.max(g.x + g.w);
            }
            h = h.max(run.line_top + run.line_height);
        }
        text.left = if x0 == f32::MAX { 0.0 } else { x0.floor() };
        ((x1 - text.left).ceil().max(30.0 * scale), h.ceil())
    }

    fn draw_bubble(
        &mut self,
        pm: &mut Pixmap,
        ax: f32,
        dog_top: f32,
        b: &Bubble,
        scale: f32,
    ) -> Option<Rect> {
        let (tw, th) = self.layout_text(b, scale);
        let pad = BUBBLE_PAD * scale;
        let w = tw + pad * 2.0;
        let h = th + pad * 2.0;
        let bottom = dog_top - (BUBBLE_GAP + BUBBLE_TAIL) * scale;
        let top = bottom - h;
        let margin = 2.0 * scale;
        let left = (ax - w / 2.0).clamp(margin, (pm.width() as f32 - w - margin).max(margin));
        let rect = Rect::from_xywh(left, top, w, h)?;

        let tail_x = ax.clamp(left + 14.0 * scale, left + w - 14.0 * scale);
        let path = bubble_path(rect, 9.0 * scale, tail_x, BUBBLE_TAIL * scale, 7.0 * scale)?;

        let shadow_t = Transform::from_translate(0.0, 2.0 * scale);
        let mut shade = Color::BLACK;
        shade.set_alpha(0.18);
        pm.fill_path(&path, &paint(shade), FillRule::Winding, shadow_t, None);
        pm.fill_path(
            &path,
            &paint(Color::WHITE),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: scale.round().max(1.0),
            ..Stroke::default()
        };
        pm.stroke_path(
            &path,
            &paint(color(OUTLINE)),
            &stroke,
            Transform::identity(),
            None,
        );

        if let Some(text) = &mut self.text {
            let x = left + pad - text.left;
            text.draw(pm, x, top + pad, cc(INK));
        }

        let full = Rect::from_ltrb(left, top, left + w, bottom + BUBBLE_TAIL * scale)?;
        Some(full)
    }
}

/// Single-line text style for other surfaces such as the panel.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    /// Physical pixels.
    pub size: f32,
    pub bold: bool,
    pub color: ColorU8,
}

impl Renderer {
    /// Measures a line of text: (width, height), physical pixels.
    pub fn measure(&mut self, text: &str, style: TextStyle) -> (f32, f32) {
        let Some(t) = self.text.as_mut() else {
            return (
                text.chars().count() as f32 * style.size * 0.55,
                style.size * 1.3,
            );
        };
        t.layout_line(text, style);
        let mut w = 0.0f32;
        for run in t.buffer.layout_runs() {
            w = w.max(run.line_w);
        }
        (w.ceil(), (style.size * 1.3).ceil())
    }

    /// Draws a line of text with its top-left corner at (x, y); returns its width.
    pub fn draw_text(
        &mut self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        text: &str,
        style: TextStyle,
    ) -> f32 {
        let (w, _) = self.measure(text, style);
        if let Some(t) = self.text.as_mut() {
            t.left = 0.0;
            t.draw(pm, x, y, cc(style.color));
        }
        w
    }

    /// Draws the first frame of the sitting dog at `px` scale, with its feet point at (`x`, `ground`).
    pub fn draw_dog(
        &self,
        pm: &mut Pixmap,
        anim: Anim,
        frame: usize,
        x: i32,
        ground: i32,
        px: i32,
    ) {
        blit_sprite(
            pm,
            &self.atlas,
            &DogFrame::still(anim, frame),
            x,
            ground,
            px,
        );
    }

    /// Draws the dog fitted into a `size`-pixel square with its top-left corner at (x, y).
    /// Downscaling uses a box filter; edge pixels are thresholded so they stay sharp.
    pub fn draw_dog_icon(&self, pm: &mut Pixmap, anim: Anim, x: i32, y: i32, size: u32) {
        let (x0, y0, x1, y1) = opaque_box(&self.atlas, anim, 0);
        let (w, h) = (x1 - x0, y1 - y0);
        let k = size as f32 / w.max(h) as f32;
        let (ow, oh) = (
            ((w as f32) * k).round() as u32,
            ((h as f32) * k).round() as u32,
        );
        let (ox, oy) = (x + (size - ow) as i32 / 2, y + (size - oh) as i32);
        let src = self.atlas.pixels();
        let aw = self.atlas.width();
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let dst = pm.pixels_mut();
        for dy in 0..oh {
            for dx in 0..ow {
                // Source area: the atlas pixels covered by this target pixel.
                let sx0 = x0 + (dx as f32 / k) as u32;
                let sy0 = y0 + (dy as f32 / k) as u32;
                let sx1 = (x0 + ((dx + 1) as f32 / k).ceil() as u32).clamp(sx0 + 1, x1);
                let sy1 = (y0 + ((dy + 1) as f32 / k).ceil() as u32).clamp(sy0 + 1, y1);
                let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = src[(sy * aw + sx) as usize];
                        r += p.red() as u32;
                        g += p.green() as u32;
                        b += p.blue() as u32;
                        a += p.alpha() as u32;
                        n += 1;
                    }
                }
                if n == 0 || a / n < 128 {
                    continue;
                }
                // Convert the premultiplied average to a fully opaque color.
                let c = |v: u32| (v * 255 / a.max(1)).min(255) as u8;
                let (tx, ty) = (ox + dx as i32, oy + dy as i32);
                if tx < 0 || ty < 0 || tx >= pw || ty >= ph {
                    continue;
                }
                if let Some(p) = PremultipliedColorU8::from_rgba(c(r), c(g), c(b), 255) {
                    dst[(ty * pw + tx) as usize] = p;
                }
            }
        }
    }

    /// Height of an animation above the ground (sprite pixels).
    pub fn dog_height(&self, anim: Anim) -> u32 {
        self.heights[anim.row() as usize]
    }
}

impl Text {
    fn layout_line(&mut self, text: &str, style: TextStyle) {
        let family = self.family.clone();
        let size = style.size;
        let weight = if style.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        };
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(weight)
            .color(cc(style.color));
        self.buffer
            .set_metrics(Metrics::new(size, (size * 1.3).ceil()));
        self.buffer.set_wrap(Wrap::None);
        self.buffer.set_size(None, None);
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.fonts, false);
        self.laid_out = None;
    }

    /// Draws the laid-out text with its top-left corner at (x, y).
    fn draw(&mut self, pm: &mut Pixmap, x: f32, y: f32, color: cosmic_text::Color) {
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let pixels = pm.pixels_mut();
        self.buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            color,
            |gx, gy, w, h, c| {
                let [r, g, b, a] = c.as_rgba();
                if a == 0 {
                    return;
                }
                let Some(src) = ColorU8::from_rgba(r, g, b, a).premultiply().into() else {
                    return;
                };
                for yy in (oy + gy).max(0)..(oy + gy + h as i32).min(ph) {
                    for xx in (ox + gx).max(0)..(ox + gx + w as i32).min(pw) {
                        let d = &mut pixels[(yy * pw + xx) as usize];
                        *d = over(src, *d);
                    }
                }
            },
        );
    }
}

fn cc(c: ColorU8) -> cosmic_text::Color {
    cosmic_text::Color::rgba(c.red(), c.green(), c.blue(), c.alpha())
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn bubble_path(
    r: Rect,
    rad: f32,
    tail_x: f32,
    tail_h: f32,
    tail_w: f32,
) -> Option<tiny_skia::Path> {
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = 0.5523 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    pb.line_to(tail_x + tail_w, b);
    pb.line_to(tail_x, b + tail_h);
    pb.line_to(tail_x - tail_w, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// Bounding box of a cell's opaque pixels (atlas coordinates, right/bottom exclusive).
fn opaque_box(atlas: &Pixmap, anim: Anim, frame: usize) -> (u32, u32, u32, u32) {
    let (cx, cy) = (frame as u32 * CELL.0, anim.row() * CELL.1);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in cy..cy + CELL.1 {
        for x in cx..cx + CELL.0 {
            if atlas.pixel(x, y).is_some_and(|p| p.alpha() > 0) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0, y0, x1, y1)
}

/// Height above the ground of the highest point across all frames of an animation.
fn sprite_height(atlas: &Pixmap, anim: Anim) -> u32 {
    (0..anim.frames())
        .map(|f| {
            let (_, y0, _, _) = opaque_box(atlas, anim, f);
            (anim.row() * CELL.1 + ANCHOR.1).saturating_sub(y0)
        })
        .max()
        .unwrap_or(ANCHOR.1)
}

/// Draws a sprite frame at `px` scale with nearest-neighbor; if `facing < 0`, mirrors it around the feet point.
pub fn blit_sprite(pm: &mut Pixmap, atlas: &Pixmap, f: &DogFrame, ax: i32, ground: i32, px: i32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let (cx, cy) = (f.frame as u32 * CELL.0, f.anim.row() * CELL.1);
    let flip = f.facing < 0.0;
    let src = atlas.pixels();
    let aw = atlas.width();
    let dst = pm.pixels_mut();
    for sy in 0..CELL.1 {
        let y0 = ground + (sy as i32 - ANCHOR.1 as i32) * px;
        for sx in 0..CELL.0 {
            let p = src[((cy + sy) * aw + cx + sx) as usize];
            if p.alpha() == 0 {
                continue;
            }
            let dx = sx as i32 - ANCHOR.0 as i32;
            let x0 = if flip {
                ax - (dx + 1) * px
            } else {
                ax + dx * px
            };
            for y in y0.max(0)..(y0 + px).min(ph) {
                for x in x0.max(0)..(x0 + px).min(pw) {
                    let d = &mut dst[(y * pw + x) as usize];
                    *d = over(p, *d);
                }
            }
        }
    }
}

fn over(s: PremultipliedColorU8, d: PremultipliedColorU8) -> PremultipliedColorU8 {
    if s.alpha() == 255 {
        return s;
    }
    let inv = 255 - s.alpha() as u32;
    let m = |a: u8, b: u8| (a as u32 + (b as u32 * inv + 127) / 255).min(255) as u8;
    let a = m(s.alpha(), d.alpha());
    PremultipliedColorU8::from_rgba(
        m(s.red(), d.red()).min(a),
        m(s.green(), d.green()).min(a),
        m(s.blue(), d.blue()).min(a),
        a,
    )
    .unwrap_or(s)
}
