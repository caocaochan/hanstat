//! The book's images (or a dictionary's): sizes read from their headers when
//! a chapter is laid out, pixels decoded in the background at the size they
//! are drawn (a cover is several megapixels; a page shows a fraction of that),
//! kept as textures within a memory budget.

use super::epub::Epub;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Texture memory kept (RGBA bytes) before the least recently drawn go.
const BUDGET: usize = 384 << 20;
/// Compressed image bytes kept between `dims()` and the first `texture()` decode.
const RAW_BUDGET: usize = 32 << 20;

/// State of an image texture requested for painting.
pub enum TexState {
    Ready(egui::TextureId),
    Pending,
    Failed,
}

struct Tex {
    handle: TextureHandle,
    /// Pixel size of the texture.
    size: [u32; 2],
    /// Decoded at the image's full size (no larger version exists).
    full: bool,
    used: u64,
}

type Decoded = (u64, String, bool, Option<(ColorImage, bool)>);

/// Reads a file's bytes by path.
pub type Source = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

pub struct Images {
    read: Source,
    /// Show greyscale images light on dark (a dictionary's black glyph
    /// images in the dark popup).
    invert_grey: bool,
    /// Texture names start with it, so two sources never share one.
    name: String,
    dims: HashMap<String, Option<[u32; 2]>>,
    /// Compressed bytes read during `dims()`, reused once by the first decode
    /// so we don't read the same ZIP entry twice.
    raw: HashMap<String, Vec<u8>>,
    raw_bytes: usize,
    tex: HashMap<String, Tex>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    /// Bumped on `clear()` so in-flight decodes from earlier sources are ignored.
    generation: u64,
    /// Decoded: (generation, path, inverted, image and whether it is at full size).
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    frame: u64,
}

impl Images {
    pub fn new(epub: Arc<Epub>) -> Images {
        Images::with_source("reader", Arc::new(move |p: &str| epub.read(p)))
    }

    /// Greyscale images inverted (light on dark), or not: the images
    /// decoded the other way are dropped and decoded again when drawn.
    pub fn set_invert_grey(&mut self, on: bool) {
        if self.invert_grey != on {
            self.invert_grey = on;
            self.tex.clear();
            self.pending.clear();
        }
    }

    /// Drop every cached dimension, texture and in-flight decode (e.g. when
    /// dictionaries are reloaded or removed).
    pub fn clear(&mut self) {
        self.generation += 1;
        self.dims.clear();
        self.raw.clear();
        self.raw_bytes = 0;
        self.tex.clear();
        self.pending.clear();
        self.failed.clear();
    }

    pub fn with_source(name: &str, read: Source) -> Images {
        let (tx, rx) = channel();
        Images {
            read,
            invert_grey: false,
            name: name.to_owned(),
            dims: HashMap::new(),
            raw: HashMap::new(),
            raw_bytes: 0,
            tex: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            generation: 0,
            tx,
            rx,
            frame: 0,
        }
    }

    /// Pixel size from the file's header (cheap: no decoding).
    pub fn dims(&mut self, path: &str) -> Option<[u32; 2]> {
        if let Some(d) = self.dims.get(path) {
            return *d;
        }
        let d = (self.read)(path).and_then(|b| {
            let dims = image::ImageReader::new(Cursor::new(&b))
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()
                .map(|(w, h)| [w, h]);
            if dims.is_some()
                && b.len() <= RAW_BUDGET
                && self.raw_bytes + b.len() <= RAW_BUDGET
                && !self.raw.contains_key(path)
            {
                self.raw_bytes += b.len();
                self.raw.insert(path.to_owned(), b);
            }
            dims
        });
        self.dims.insert(path.to_owned(), d);
        d
    }

    /// Take in decoded images.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut any = false;
        while let Ok((generation, path, inverted, img)) = self.rx.try_recv() {
            if generation != self.generation || inverted != self.invert_grey {
                // Asked before the source or the popup's theme changed.
                continue;
            }
            self.pending.remove(&path);
            any = true;
            match img {
                Some((img, full)) => {
                    let size = [img.size[0] as u32, img.size[1] as u32];
                    let handle = ctx.load_texture(
                        format!("{}:{path}", self.name),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.tex.insert(
                        path,
                        Tex {
                            handle,
                            size,
                            full,
                            used: self.frame,
                        },
                    );
                }
                None => {
                    self.failed.insert(path);
                }
            }
        }
        if any {
            self.evict();
        }
    }

    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// The texture to draw `path` with at `px` physical pixels, asking for a
    /// sharper decode when the one there is too small. Calls
    /// `ctx.request_repaint()` from the worker once decoding finishes.
    pub fn texture(&mut self, ctx: &egui::Context, path: &str, px: [u32; 2]) -> TexState {
        if self.failed.contains(path) {
            return TexState::Failed;
        }
        let want_more = match self.tex.get(path) {
            Some(t) => {
                !t.full
                    && (px[0] as f32 > t.size[0] as f32 * 1.15
                        || px[1] as f32 > t.size[1] as f32 * 1.15)
            }
            None => true,
        };
        if want_more && !self.pending.contains(path) {
            self.pending.insert(path.to_owned());
            let cached = self.raw.remove(path).inspect(|b| self.raw_bytes -= b.len());
            let read = self.read.clone();
            let invert = self.invert_grey;
            let generation = self.generation;
            let tx = self.tx.clone();
            let p = path.to_owned();
            let ctx = ctx.clone();
            rayon::spawn(move || {
                let bytes = cached.or_else(|| read(&p));
                let img = bytes.and_then(|b| decode(&b, px, invert));
                let _ = tx.send((generation, p, invert, img));
                ctx.request_repaint();
            });
        }
        match self.tex.get_mut(path) {
            Some(t) => {
                t.used = self.frame;
                TexState::Ready(t.handle.id())
            }
            None => TexState::Pending,
        }
    }

    /// Decode now (for rendering pages to files, where nothing waits a frame).
    #[cfg(test)]
    pub fn load_now(&mut self, ctx: &egui::Context, path: &str, px: [u32; 2]) {
        if let Some((img, full)) = (self.read)(path).and_then(|b| decode(&b, px, self.invert_grey))
        {
            let size = [img.size[0] as u32, img.size[1] as u32];
            let handle =
                ctx.load_texture(format!("{}:{path}", self.name), img, TextureOptions::LINEAR);
            self.tex.insert(
                path.to_owned(),
                Tex {
                    handle,
                    size,
                    full,
                    used: self.frame,
                },
            );
        }
    }

    fn evict(&mut self) {
        let mut total: usize = self
            .tex
            .values()
            .map(|t| t.size[0] as usize * t.size[1] as usize * 4)
            .sum();
        if total <= BUDGET {
            return;
        }
        let mut by_age: Vec<(u64, String)> =
            self.tex.iter().map(|(k, t)| (t.used, k.clone())).collect();
        by_age.sort();
        for (used, k) in by_age {
            if total <= BUDGET || used + 2 >= self.frame {
                break;
            }
            if let Some(t) = self.tex.remove(&k) {
                total -= t.size[0] as usize * t.size[1] as usize * 4;
            }
        }
    }
}

/// Decode and scale down to fit `px` (keeping the aspect ratio).
fn decode(bytes: &[u8], px: [u32; 2], invert_grey: bool) -> Option<(ColorImage, bool)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let scale = (px[0].max(1) as f32 / w as f32).min(px[1].max(1) as f32 / h as f32);
    let (img, full) = if scale < 0.95 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        (
            img.resize_exact(nw, nh, image::imageops::FilterType::CatmullRom),
            false,
        )
    } else {
        (img, true)
    };
    let mut rgba = img.to_rgba8();
    if invert_grey {
        let px: Vec<&image::Rgba<u8>> = rgba.pixels().filter(|p| p[3] > 32).collect();
        let grey = px
            .iter()
            .filter(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) < 24)
            .count();
        if !px.is_empty() && grey * 10 >= px.len() * 9 {
            for p in rgba.pixels_mut() {
                for c in &mut p.0[..3] {
                    *c = 255 - *c;
                }
            }
        }
    }
    Some((
        ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ),
        full,
    ))
}
