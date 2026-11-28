//! egui output drawn to a PNG without a window: for the image exports,
//! which are taller than any screen. A page is laid out in an offscreen
//! [`egui::Context`] with the window's fonts, tessellated, and its
//! triangles filled here strip by strip, blended in gamma space as egui's
//! own renderers do, and written to the file as they are done.

use eframe::egui::{
    self, Color32, Pos2, Rect, TexturesDelta,
    epaint::{ClippedPrimitive, ImageData, Primitive, TextureId},
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// Rows filled at a time by one thread.
const STRIP: usize = 128;

struct Texture {
    size: [usize; 2],
    /// Premultiplied sRGBA.
    pixels: Vec<Color32>,
}

impl Texture {
    /// Bilinear, premultiplied, 0–1.
    fn sample(&self, uv: Pos2) -> [f32; 4] {
        let [w, h] = self.size;
        if w == 0 || h == 0 {
            return [1.0; 4];
        }
        let x = (uv.x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (uv.y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let px = |x: usize, y: usize| self.pixels[y * w + x].to_array();
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] as f32 * (1.0 - fx) + b[i] as f32 * fx;
            let bottom = c[i] as f32 * (1.0 - fx) + d[i] as f32 * fx;
            out[i] = (top * (1.0 - fy) + bottom * fy) / 255.0;
        }
        out
    }
}

/// A laid-out page, ready to be drawn.
pub(crate) struct Scene {
    primitives: Vec<ClippedPrimitive>,
    textures: HashMap<TextureId, Texture>,
    pixels_per_point: f32,
    size: [usize; 2],
    background: Color32,
}

/// Lay out a page `width` points wide with `draw`, at up to
/// `pixels_per_point`, on `background`. The window's fonts and style are
/// changed by `fonts` and `style`; `prepare` runs on the new context
/// before the page is drawn (to load images). The page is as tall as what
/// `draw` adds.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn lay_out<T>(
    window: &egui::Context,
    width: f32,
    pixels_per_point: f32,
    background: Color32,
    fonts: impl FnOnce(&mut egui::FontDefinitions),
    style: impl FnOnce(&mut egui::Style),
    prepare: impl FnOnce(&egui::Context) -> T,
    draw: impl FnMut(&mut egui::Ui, &T),
) -> Scene {
    let defs = window.fonts(|f| f.definitions().clone());
    let theme = window.theme();
    let base_style = (*window.global_style()).clone();
    lay_out_from(
        defs,
        theme,
        base_style,
        width,
        pixels_per_point,
        background,
        fonts,
        style,
        prepare,
        draw,
    )
}

/// Same as [`lay_out`], from a cloned snapshot of window fonts/theme/style so
/// the entire offscreen layout, cover decoding, and tessellation can run off
/// the UI thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lay_out_from<T>(
    mut defs: egui::FontDefinitions,
    theme: egui::Theme,
    mut s: egui::Style,
    width: f32,
    pixels_per_point: f32,
    background: Color32,
    fonts: impl FnOnce(&mut egui::FontDefinitions),
    style: impl FnOnce(&mut egui::Style),
    prepare: impl FnOnce(&egui::Context) -> T,
    mut draw: impl FnMut(&mut egui::Ui, &T),
) -> Scene {
    let ctx = egui::Context::default();
    fonts(&mut defs);
    ctx.set_fonts(defs);
    ctx.set_theme(theme);
    style(&mut s);
    ctx.set_global_style(s);
    let data = prepare(&ctx);

    let run = |ppp: f32, draw: &mut dyn FnMut(&mut egui::Ui, &T)| {
        let mut raw = egui::RawInput {
            // Taller than any page: its height is what is drawn.
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(width, 1_000_000.0),
            )),
            max_texture_side: Some(16_384),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        let mut height = 0.0f32;
        let out = ctx.run_ui(raw, |ui| {
            draw(ui, &data);
            height = ui.min_rect().bottom();
        });
        (out, height)
    };
    // The width is fixed, however tall the page: it is written strip by strip.
    let (mut out, height) = run(pixels_per_point, &mut draw);
    let ppp = out.pixels_per_point;
    let mut textures = HashMap::new();
    apply(&mut textures, &out.textures_delta);
    // Applied here: egui checks that no delta is dropped unseen.
    out.textures_delta.clear();
    let primitives = ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
    let w = (width * ppp).round() as usize;
    let h = ((height * ppp).ceil() as usize).max(1);
    Scene {
        primitives,
        textures,
        pixels_per_point: ppp,
        size: [w, h],
        background,
    }
}

/// A window's frames, run by hand (in tests), kept as pictures: its
/// textures followed across frames, any frame written as a PNG.
#[cfg(test)]
pub(crate) struct Screen {
    textures: HashMap<TextureId, Texture>,
}

#[cfg(test)]
impl Screen {
    pub(crate) fn new() -> Self {
        Screen {
            textures: HashMap::new(),
        }
    }

    /// Take in a frame's texture changes (every frame must come here).
    pub(crate) fn absorb(&mut self, out: &mut egui::FullOutput) {
        apply(&mut self.textures, &out.textures_delta);
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        out.textures_delta.clear();
    }

    /// The frame `out` (absorbed) as a picture of `size` points.
    pub(crate) fn picture(
        &self,
        ctx: &egui::Context,
        out: &mut egui::FullOutput,
        size: egui::Vec2,
    ) -> Scene {
        let ppp = out.pixels_per_point;
        let primitives = ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
        Scene {
            primitives,
            textures: self
                .textures
                .iter()
                .map(|(k, t)| {
                    (
                        *k,
                        Texture {
                            size: t.size,
                            pixels: t.pixels.clone(),
                        },
                    )
                })
                .collect(),
            pixels_per_point: ppp,
            size: [
                (size.x * ppp).round() as usize,
                (size.y * ppp).round() as usize,
            ],
            background: Color32::BLACK,
        }
    }
}

fn apply(textures: &mut HashMap<TextureId, Texture>, delta: &TexturesDelta) {
    for (id, d) in delta
        .set
        .iter()
        .flat_map(|(id, ds)| ds.iter().map(move |d| (id, d)))
    {
        let ImageData::Color(img) = &d.image;
        match d.pos {
            None => {
                textures.insert(
                    *id,
                    Texture {
                        size: img.size,
                        pixels: img.pixels.clone(),
                    },
                );
            }
            Some([x, y]) => {
                let Some(t) = textures.get_mut(id) else {
                    continue;
                };
                let [w, _] = img.size;
                for (row, line) in img.pixels.chunks(w.max(1)).enumerate() {
                    let ty = y + row;
                    if ty >= t.size[1] {
                        break;
                    }
                    let start = ty * t.size[0] + x;
                    let n = line.len().min(t.size[0].saturating_sub(x));
                    t.pixels[start..start + n].copy_from_slice(&line[..n]);
                }
            }
        }
    }
}

/// One triangle, in pixels, with what it is drawn with.
struct Tri {
    p: [[f32; 2]; 3],
    color: [[f32; 4]; 3],
    uv: [Pos2; 3],
    texture: TextureId,
    /// Pixels it may touch: x0, y0, x1, y1 (exclusive), within its clip rect.
    bounds: [usize; 4],
}

impl Scene {
    pub(crate) fn size(&self) -> [usize; 2] {
        self.size
    }

    fn triangles(&self) -> Vec<Tri> {
        let k = self.pixels_per_point;
        let [w, h] = self.size;
        let mut out = Vec::new();
        for cp in &self.primitives {
            let Primitive::Mesh(mesh) = &cp.primitive else {
                continue;
            };
            let clip = cp.clip_rect;
            let cx0 = (clip.min.x * k).round().clamp(0.0, w as f32) as usize;
            let cy0 = (clip.min.y * k).round().clamp(0.0, h as f32) as usize;
            let cx1 = (clip.max.x * k).round().clamp(0.0, w as f32) as usize;
            let cy1 = (clip.max.y * k).round().clamp(0.0, h as f32) as usize;
            if cx0 >= cx1 || cy0 >= cy1 {
                continue;
            }
            for t in mesh.indices.as_chunks::<3>().0 {
                let v = [
                    &mesh.vertices[t[0] as usize],
                    &mesh.vertices[t[1] as usize],
                    &mesh.vertices[t[2] as usize],
                ];
                if v.iter().all(|v| v.color.a() == 0) {
                    continue;
                }
                let p = v.map(|v| [v.pos.x * k, v.pos.y * k]);
                let minx = p.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
                let maxx = p.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
                let miny = p.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                let maxy = p.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
                let x0 = (minx.floor().max(0.0) as usize).max(cx0);
                let y0 = (miny.floor().max(0.0) as usize).max(cy0);
                let x1 = ((maxx.ceil().max(0.0) as usize) + 1).min(cx1);
                let y1 = ((maxy.ceil().max(0.0) as usize) + 1).min(cy1);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                out.push(Tri {
                    p,
                    color: v.map(|v| v.color.to_array().map(|c| c as f32 / 255.0)),
                    uv: v.map(|v| v.uv),
                    texture: mesh.texture_id,
                    bounds: [x0, y0, x1, y1],
                });
            }
        }
        out
    }

    /// Fill rows `y0..y0 + rows` (RGB) with `tris`, in order.
    fn strip(&self, tris: &[&Tri], y0: usize, rows: usize) -> Vec<u8> {
        let w = self.size[0];
        let bg = self.background.to_array();
        let mut buf: Vec<[f32; 3]> = vec![
            [
                bg[0] as f32 / 255.0,
                bg[1] as f32 / 255.0,
                bg[2] as f32 / 255.0
            ];
            w * rows
        ];
        for t in tris {
            draw(t, self.textures.get(&t.texture), &mut buf, w, y0, y0 + rows);
        }
        buf.iter()
            .flat_map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
            .collect()
    }

    /// The colour drawn at `p` (points).
    #[cfg(test)]
    pub(crate) fn pixel(&self, p: egui::Pos2) -> Color32 {
        let k = self.pixels_per_point;
        let (x, y) = ((p.x * k) as usize, (p.y * k) as usize);
        let tris = self.triangles();
        let on_row: Vec<&Tri> = tris
            .iter()
            .filter(|t| t.bounds[1] <= y && y < t.bounds[3])
            .collect();
        let row = self.strip(&on_row, y, 1);
        Color32::from_rgb(row[3 * x], row[3 * x + 1], row[3 * x + 2])
    }

    /// Rasterise the scene into an in-memory [`egui::ColorImage`] (parallelised
    /// across strips, using the same pipeline as [`Self::write_png`]).
    pub(crate) fn to_color_image(&self) -> egui::ColorImage {
        let [w, h] = self.size;
        let tris = self.triangles();
        let strips = h.div_ceil(STRIP);
        let mut buckets: Vec<Vec<&Tri>> = vec![Vec::new(); strips];
        for t in &tris {
            let [_, y0, _, y1] = t.bounds;
            for b in &mut buckets[y0 / STRIP..=((y1 - 1) / STRIP).min(strips - 1)] {
                b.push(t);
            }
        }
        let rows: Vec<Vec<u8>> = (0..strips)
            .into_par_iter()
            .map(|s| {
                let y0 = s * STRIP;
                self.strip(&buckets[s], y0, STRIP.min(h - y0))
            })
            .collect();
        let mut rgb = Vec::with_capacity(w * h * 3);
        for chunk in rows {
            rgb.extend_from_slice(&chunk);
        }
        egui::ColorImage::from_rgb([w, h], &rgb)
    }

    /// Write the page to `path`.
    pub(crate) fn write_png(&self, path: &Path) -> Result<(), String> {
        let [w, h] = self.size;
        let tris = self.triangles();
        // Each strip's triangles, in drawing order.
        let strips = h.div_ceil(STRIP);
        let mut buckets: Vec<Vec<&Tri>> = vec![Vec::new(); strips];
        for t in &tris {
            let [_, y0, _, y1] = t.bounds;
            for b in &mut buckets[y0 / STRIP..=((y1 - 1) / STRIP).min(strips - 1)] {
                b.push(t);
            }
        }
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut out = std::io::BufWriter::new(file);
        let io = |e: std::io::Error| e.to_string();
        out.write_all(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
            .map_err(io)?;
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend((w as u32).to_be_bytes());
        ihdr.extend((h as u32).to_be_bytes());
        // 8-bit RGB, deflate, adaptive filtering, not interlaced.
        ihdr.extend([8, 2, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr).map_err(io)?;
        // The scale it was drawn at, so viewers that read it can show it at
        // its size in points.
        let dpm = (self.pixels_per_point * 96.0 / 0.0254).round() as u32;
        let mut phys = Vec::with_capacity(9);
        phys.extend(dpm.to_be_bytes());
        phys.extend(dpm.to_be_bytes());
        phys.push(1);
        chunk(&mut out, b"pHYs", &phys).map_err(io)?;
        let mut z = flate2::write::ZlibEncoder::new(
            Idat {
                out: &mut out,
                buf: Vec::new(),
            },
            flate2::Compression::new(6),
        );
        // A batch of strips at a time, so the whole image is never in memory.
        let stride = w * 3;
        let mut prev = vec![0u8; stride];
        let mut line = vec![0u8; stride + 1];
        let batch = rayon::current_num_threads().max(1) * 2;
        for first in (0..strips).step_by(batch) {
            let rows: Vec<Vec<u8>> = (first..(first + batch).min(strips))
                .into_par_iter()
                .map(|s| {
                    let y0 = s * STRIP;
                    self.strip(&buckets[s], y0, STRIP.min(h - y0))
                })
                .collect();
            for row in rows.iter().flat_map(|r| r.chunks_exact(stride)) {
                filter(row, &prev, &mut line);
                z.write_all(&line).map_err(io)?;
                prev.copy_from_slice(row);
            }
        }
        z.finish().map_err(io)?.flush().map_err(io)?;
        chunk(&mut out, b"IEND", &[]).map_err(io)?;
        out.flush().map_err(io)
    }
}

/// A PNG chunk: length, type, data and the CRC of type and data.
fn chunk(out: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    out.write_all(&(data.len() as u32).to_be_bytes())?;
    out.write_all(kind)?;
    out.write_all(data)?;
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.write_all(&crc.sum().to_be_bytes())
}

/// The compressed image data, written as IDAT chunks of up to 256 KiB.
struct Idat<'a, W: Write> {
    out: &'a mut W,
    buf: Vec<u8>,
}

impl<W: Write> Idat<'_, W> {
    const SIZE: usize = 256 * 1024;
}

impl<W: Write> Write for Idat<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        while self.buf.len() >= Self::SIZE {
            chunk(self.out, b"IDAT", &self.buf[..Self::SIZE])?;
            self.buf.drain(..Self::SIZE);
        }
        Ok(data.len())
    }

    /// Only once, at the end: the rest, as the last IDAT chunk.
    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            chunk(self.out, b"IDAT", &self.buf)?;
            self.buf.clear();
        }
        self.out.flush()
    }
}

/// `row` (RGB) as a filtered scanline in `out`: the filter byte, then the
/// row under whichever of None, Sub, Up and Paeth leaves the smallest
/// values, as libpng chooses.
fn filter(row: &[u8], prev: &[u8], out: &mut [u8]) {
    const BPP: usize = 3;
    let paeth = |a: u8, b: u8, c: u8| {
        let p = a as i16 + b as i16 - c as i16;
        let (pa, pb, pc) = (
            (p - a as i16).abs(),
            (p - b as i16).abs(),
            (p - c as i16).abs(),
        );
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    };
    let apply = |kind: u8, i: usize| -> u8 {
        let x = row[i];
        let a = if i >= BPP { row[i - BPP] } else { 0 };
        let b = prev[i];
        let c = if i >= BPP { prev[i - BPP] } else { 0 };
        match kind {
            1 => x.wrapping_sub(a),
            2 => x.wrapping_sub(b),
            4 => x.wrapping_sub(paeth(a, b, c)),
            _ => x,
        }
    };
    let cost = |kind: u8| -> u64 {
        (0..row.len())
            .map(|i| (apply(kind, i) as i8).unsigned_abs() as u64)
            .sum()
    };
    let best = [0u8, 1, 2, 4].into_iter().min_by_key(|&k| cost(k)).unwrap();
    out[0] = best;
    for i in 0..row.len() {
        out[i + 1] = apply(best, i);
    }
}

fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Whether a pixel centre exactly on edge a→b is the triangle's: of two
/// triangles sharing an edge, which run it in opposite directions, exactly
/// one, so a translucent shape split in two has no seam.
fn owns(a: [f32; 2], b: [f32; 2]) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    dy < 0.0 || (dy == 0.0 && dx > 0.0)
}

fn draw(t: &Tri, tex: Option<&Texture>, buf: &mut [[f32; 3]], w: usize, y0: usize, y1: usize) {
    let [mut a, mut b, c] = t.p;
    let (mut ia, mut ib) = (0, 1);
    let mut area = edge(a, b, c);
    if area == 0.0 {
        return;
    }
    // One winding for all, so that shared edges go opposite ways.
    if area < 0.0 {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut ia, &mut ib);
        area = -area;
    }
    let ic = 2;
    let own = [owns(b, c), owns(c, a), owns(a, b)];
    let flat_color = t.color[0] == t.color[1] && t.color[1] == t.color[2];
    let flat_uv = t.uv[0] == t.uv[1] && t.uv[1] == t.uv[2];
    let texel = |uv: Pos2| tex.map_or([1.0; 4], |x| x.sample(uv));
    let fixed_texel = flat_uv.then(|| texel(t.uv[0]));
    let [x0, by0, x1, by1] = t.bounds;
    for y in by0.max(y0)..by1.min(y1) {
        let py = y as f32 + 0.5;
        let row = (y - y0) * w;
        for x in x0..x1 {
            let p = [x as f32 + 0.5, py];
            let w0 = edge(b, c, p);
            let w1 = edge(c, a, p);
            let w2 = edge(a, b, p);
            let inside = |v: f32, o: bool| v > 0.0 || (v == 0.0 && o);
            if !(inside(w0, own[0]) && inside(w1, own[1]) && inside(w2, own[2])) {
                continue;
            }
            let l = [w0 / area, w1 / area, w2 / area];
            let idx = [ia, ib, ic];
            let color = if flat_color {
                t.color[0]
            } else {
                let mut c = [0.0; 4];
                for (k, &i) in idx.iter().enumerate() {
                    for (ch, v) in c.iter_mut().enumerate() {
                        *v += l[k] * t.color[i][ch];
                    }
                }
                c
            };
            let tx = fixed_texel.unwrap_or_else(|| {
                let mut u = 0.0;
                let mut v = 0.0;
                for (k, &i) in idx.iter().enumerate() {
                    u += l[k] * t.uv[i].x;
                    v += l[k] * t.uv[i].y;
                }
                texel(Pos2::new(u, v))
            });
            let src = [
                color[0] * tx[0],
                color[1] * tx[1],
                color[2] * tx[2],
                color[3] * tx[3],
            ];
            if src[3] <= 0.0 && src[0] <= 0.0 && src[1] <= 0.0 && src[2] <= 0.0 {
                continue;
            }
            let d = &mut buf[row + x];
            let k = 1.0 - src[3];
            d[0] = src[0] + d[0] * k;
            d[1] = src[1] + d[1] * k;
            d[2] = src[2] + d[2] * k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page drawn and written decodes to the size laid out, with its
    /// background, an opaque shape and translucent halves with no seam.
    #[test]
    fn page_round_trips_through_png() {
        // A window's context has fonts after its first pass.
        let window = egui::Context::default();
        window
            .run_ui(Default::default(), |_| {})
            .drop_without_applying_deltas();
        let bg = Color32::from_rgb(10, 20, 30);
        let scene = lay_out(
            &window,
            100.0,
            1.5,
            bg,
            |_| {},
            |_| {},
            |_| (),
            |ui, _| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(100.0, 80.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(
                    Rect::from_min_size(r.min, egui::vec2(40.0, 40.0)),
                    0.0,
                    Color32::RED,
                );
                // Two triangles sharing a diagonal, half transparent.
                p.rect_filled(
                    Rect::from_min_size(r.min + egui::vec2(50.0, 0.0), egui::vec2(40.0, 40.0)),
                    0.0,
                    Color32::from_white_alpha(128),
                );
            },
        );
        assert_eq!(scene.size(), [150, 120]);
        let path = std::env::temp_dir().join(format!("hanstat-raster-{}.png", std::process::id()));
        scene.write_png(&path).unwrap();
        let img = image::open(&path).unwrap().into_rgb8();
        let _ = std::fs::remove_file(&path);
        assert_eq!(img.dimensions(), (150, 120));
        assert_eq!(img.get_pixel(20, 20).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(140, 110).0, bg.to_array()[..3]);
        // Every pixel of the translucent square alike, the diagonal included.
        let first = img.get_pixel(80, 5).0;
        for y in 2..58 {
            for x in 77..133 {
                assert_eq!(img.get_pixel(x, y).0, first, "at {x}, {y}");
            }
        }
        assert!(first[0] > 120 && first[0] < 140, "{first:?}");
    }
}
