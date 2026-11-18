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

struct Tex {
    handle: TextureHandle,
    /// Pixel size of the texture.
    size: [u32; 2],
    /// Decoded at the image's full size (no larger version exists).
    full: bool,
    used: u64,
}

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
    tex: HashMap<String, Tex>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    tx: Sender<(String, Option<(ColorImage, bool)>)>,
    rx: Receiver<(String, Option<(ColorImage, bool)>)>,
    frame: u64,
}

impl Images {
    pub fn new(epub: Arc<Epub>) -> Images {
        Images::with_source("reader", Arc::new(move |p: &str| epub.read(p)))
    }

    /// Greyscale images inverted (light on dark).
    pub fn invert_grey(mut self) -> Images {
        self.invert_grey = true;
        self
    }

    pub fn with_source(name: &str, read: Source) -> Images {
        let (tx, rx) = channel();
        Images {
            read,
            invert_grey: false,
            name: name.to_owned(),
            dims: HashMap::new(),
            tex: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
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
            image::ImageReader::new(Cursor::new(b))
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()
                .map(|(w, h)| [w, h])
        });
        self.dims.insert(path.to_owned(), d);
        d
    }

    /// Take in decoded images; `true` if any arrived.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut any = false;
        while let Ok((path, img)) = self.rx.try_recv() {
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
        any
    }

    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// The texture to draw `path` with at `px` physical pixels, asking for a
    /// sharper decode when the one there is too small.
    pub fn texture(&mut self, path: &str, px: [u32; 2]) -> Option<egui::TextureId> {
        if self.failed.contains(path) {
            return None;
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
            let read = self.read.clone();
            let invert = self.invert_grey;
            let tx = self.tx.clone();
            let p = path.to_owned();
            rayon::spawn(move || {
                let img = read(&p).and_then(|b| decode(&b, px, invert));
                let _ = tx.send((p, img));
            });
        }
        let t = self.tex.get_mut(path)?;
        t.used = self.frame;
        Some(t.handle.id())
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
