//! Pages of a real book drawn to PNG files, to look at the layout without a
//! window. Run by hand:
//!
//! ```text
//! READER_EPUB=book.epub READER_OUT=dir READER_PAGES=3:0,3:1 \
//!   cargo test --bin hanstat reader::preview -- --ignored --nocapture
//! ```
//!
//! `READER_PAGES` is `chapter:page` pairs (spine indices, linear items only);
//! `READER_SIZE`, `READER_W`/`READER_H` (window points), `READER_SPREAD=0`, `READER_SERIF` /
//! `READER_SANS` (family names), `READER_SANS_DEFAULT=1` and `READER_TRAD=1`
//! change the setup.

use super::*;
use std::time::Instant;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

#[test]
#[ignore]
fn render_pages() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let out = std::path::PathBuf::from(
        env("READER_OUT").unwrap_or_else(|| std::env::temp_dir().display().to_string()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let spec: Vec<(usize, usize)> = env("READER_PAGES")
        .unwrap_or_else(|| "0:0,1:0,2:0,3:0,3:1".into())
        .split(',')
        .filter_map(|p| {
            let (a, b) = p.split_once(':')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .collect();
    let w: f32 = env("READER_W")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1280.0);
    let h: f32 = env("READER_H")
        .and_then(|v| v.parse().ok())
        .unwrap_or(860.0);
    let mut settings = Settings::default();
    if let Some(s) = env("READER_SIZE").and_then(|v| v.parse().ok()) {
        settings.size = s;
    }
    settings.sans_default = env("READER_SANS_DEFAULT").is_some();
    if let Some(v) = env("READER_SPREAD") {
        settings.spread = v != "0";
    }
    let trad = env("READER_TRAD").is_some();

    let window = egui::Context::default();
    crate::install_fonts(&window);
    window
        .run_ui(Default::default(), |_| {})
        .textures_delta
        .clear();
    let t = Instant::now();
    let fams = loop {
        if let Some(f) = fonts::families() {
            break f;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    eprintln!("font scan: {:?}, {} families", t.elapsed(), fams.len());
    let serif = env("READER_SERIF").unwrap_or_else(|| fonts::default_choice(&fams, true, trad));
    let sans = env("READER_SANS").unwrap_or_else(|| fonts::default_choice(&fams, false, trad));
    eprintln!("serif {serif:?}, sans {sans:?}");
    let (defs, installed) = fonts::definitions(&fams, &serif, &sans, 1);

    let t = Instant::now();
    let epub = Arc::new(Epub::open(Path::new(&book)).unwrap());
    eprintln!(
        "open: {:?}, {} chapters, {} toc entries",
        t.elapsed(),
        epub.spine.len(),
        epub.toc.len()
    );
    let mut sheets = tree::SheetCache::default();
    let area = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
    let g = geometry(&settings, area);
    let step = if g.spread { 2 } else { 1 };
    for (spine, page) in spec {
        let Some(path) = epub.spine.get(spine) else {
            continue;
        };
        let t = Instant::now();
        let ch = tree::build(&epub, path, &mut sheets);
        let t_tree = t.elapsed();
        let mut images = images::Images::new(epub.clone());
        let params = Params {
            base: settings.size,
            content_w: g.content.x,
            page_h: g.content.y,
            line_spacing: settings.line_spacing,
            fonts: installed.clone(),
            serif_default: !settings.sans_default,
            text: theme::INK,
            link: theme::BLUE,
            rule: theme::BASE1,
        };
        let mut info = String::new();
        let scene = crate::raster::lay_out(
            &window,
            w,
            2.0,
            theme::DESK,
            |d| *d = defs.clone(),
            |s| s.visuals = visuals(),
            |_| (),
            |ui, _| {
                let ctx = ui.ctx().clone();
                let t = Instant::now();
                let l = ctx.fonts_mut(|f| layout::layout(&ch, &params, f, &mut images));
                let t_layout = t.elapsed();
                // Twice: the second with egui's glyph cache warm, as on a resize.
                let t = Instant::now();
                let _ = ctx.fonts_mut(|f| layout::layout(&ch, &params, f, &mut images));
                info = format!(
                    "{spine}:{page} {path}: tree {t_tree:?}, layout {t_layout:?} (again {:?}), {} pages, {} chars",
                    t.elapsed(),
                    l.pages.len(),
                    ch.text_len
                );
                ui.painter().rect_filled(area, 0.0, theme::DESK);
                let first = page - page % step;
                for side in 0..step {
                    let page_rect = g.pages[side];
                    draw_page(ui, page_rect, side, g.spread);
                    let idx = first + side;
                    if idx >= l.pages.len() {
                        continue;
                    }
                    let origin = page_rect.min + g.inset;
                    let mut off = origin.to_vec2();
                    if l.pages[idx].center {
                        off.y += ((g.content.y - l.page_height(idx)) / 2.0).max(0.0);
                    }
                    let (items, shift) = l.page_items(idx);
                    let off = off + shift;
                    let clip = Rect::from_min_size(origin, g.content).expand(4.0);
                    let painter = ui.painter_at(clip.expand2(vec2(g.inset.x * 0.5, 0.0)));
                    for i in &items {
                        if let Item::Image { rect, path } = &l.items[*i] {
                            let r = rect.translate(off);
                            images.load_now(
                                &ctx,
                                path,
                                [(r.width() * 2.0) as u32, (r.height() * 2.0) as u32],
                            );
                        }
                    }
                    for i in items {
                        paint_item(&mut images, &painter, &l.items[i], off, 2.0);
                    }
                    let small = egui::FontId::proportional((settings.size * 0.62).max(11.0));
                    ui.painter().text(
                        pos2(page_rect.center().x, page_rect.bottom() - g.inset.y * 0.5),
                        egui::Align2::CENTER_CENTER,
                        format!("{} / {}", idx + 1, l.pages.len()),
                        small,
                        theme::BASE1,
                    );
                }
                ui.allocate_rect(area, egui::Sense::hover());
            },
        );
        eprintln!("{info}");
        let file = out.join(format!("{spine:03}-{page:02}.png"));
        scene.write_png(&file).unwrap();
    }
}

/// The reader's own UI, driven frame by frame without a window: it opens,
/// finds its fonts, lays out, turns pages with the keys and across
/// chapters, and closes on Esc.
#[test]
#[ignore]
fn drive_ui() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let ctx = egui::Context::default();
    crate::install_fonts(&ctx);
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
    let frame = |reader: &mut Reader, keys: &[Key]| -> bool {
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events: keys
                .iter()
                .map(|k| egui::Event::Key {
                    key: *k,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
            ..Default::default()
        };
        let mut open = true;
        let mut out = ctx.run_ui(raw, |ui| open = reader.ui(ui));
        out.textures_delta.clear();
        open
    };
    let mut r = Reader::open(Path::new(&book), Some("drive-ui-test"), false, &ctx).unwrap();
    r.persist = false;
    r.spine = 0;
    r.target = Some(Target::Offset(0));
    let t = std::time::Instant::now();
    let mut frames = 0;
    while !r.layouts.contains_key(&r.spine) || r.target.is_some() {
        assert!(frame(&mut r, &[]));
        frames += 1;
        assert!(t.elapsed().as_secs() < 30, "never laid out");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    eprintln!("first page after {frames} frames, {:?}", t.elapsed());
    let mut seen = vec![(r.spine, r.page)];
    for _ in 0..12 {
        frame(&mut r, &[Key::ArrowRight]);
        for _ in 0..3 {
            frame(&mut r, &[]);
        }
        seen.push((r.spine, r.page));
    }
    eprintln!("forward: {seen:?}");
    assert!(
        seen.windows(2).all(|w| w[1] > w[0]),
        "pages move forward: {seen:?}"
    );
    let at = (r.spine, r.page);
    frame(&mut r, &[Key::ArrowLeft]);
    for _ in 0..3 {
        frame(&mut r, &[]);
    }
    assert!(
        (r.spine, r.page) < at,
        "back from {at:?} to {:?}",
        (r.spine, r.page)
    );
    assert!(!frame(&mut r, &[Key::Escape]), "Esc closes");
    r.close(&ctx);
}
