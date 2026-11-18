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

/// The reader in a window-less egui context: frames run by hand with
/// pointer and key events, any frame saved as a PNG.
struct Driver {
    ctx: egui::Context,
    screen: crate::raster::Screen,
    size: Vec2,
    last: Option<egui::FullOutput>,
    out_dir: Option<std::path::PathBuf>,
    /// Seconds: 60 frames a second, so animations (popups fading in) run.
    time: f64,
}

impl Driver {
    fn new() -> Driver {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        Driver {
            ctx,
            screen: crate::raster::Screen::new(),
            size: vec2(1200.0, 800.0),
            last: None,
            out_dir: env("READER_OUT").map(std::path::PathBuf::from),
            time: 0.0,
        }
    }

    fn open(&self, book: &str) -> Reader {
        let mut r = Reader::open(Path::new(book), Some("ui-test"), false, &self.ctx).unwrap();
        r.persist = false;
        r
    }

    /// One frame; `false` once the reader closed.
    fn frame(&mut self, r: &mut Reader, events: Vec<egui::Event>) -> bool {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            events,
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let mut open = true;
        let mut out = self.ctx.run_ui(raw, |ui| open = r.ui(ui));
        self.screen.absorb(&mut out);
        self.last = Some(out);
        open
    }

    fn idle(&mut self, r: &mut Reader, n: usize) {
        for _ in 0..n {
            self.frame(r, vec![]);
        }
    }

    /// Until the page is shown and every chapter is built.
    fn ready(&mut self, r: &mut Reader) {
        let t = Instant::now();
        while fonts::families().is_none()
            || !r.layouts.contains_key(&r.spine)
            || r.target.is_some()
            || r.progress().is_none()
        {
            self.frame(r, vec![]);
            assert!(t.elapsed().as_secs() < 30, "never laid out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.idle(r, 2);
    }

    fn click(&mut self, r: &mut Reader, p: Pos2) -> bool {
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let a = self.frame(r, vec![egui::Event::PointerMoved(p), button(true)]);
        let b = self.frame(r, vec![button(false)]);
        self.idle(r, 30);
        a && b
    }

    fn key(&mut self, r: &mut Reader, key: Key) -> bool {
        let open = self.frame(
            r,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        self.idle(r, 30);
        open
    }

    /// Save the last frame as `name`.png in `READER_OUT`, if set.
    fn snap(&mut self, name: &str) {
        let (Some(dir), Some(mut out)) = (self.out_dir.clone(), self.last.take()) else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        let scene = self.screen.picture(&self.ctx, &mut out, self.size);
        scene.write_png(&dir.join(format!("{name}.png"))).unwrap();
    }
}

/// The reader's own UI, driven frame by frame without a window: it opens,
/// finds its fonts, lays out, turns pages with the keys and across
/// chapters, and closes on Esc.
#[test]
#[ignore]
fn drive_ui() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.spine = 0;
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    let mut seen = vec![(r.spine, r.page)];
    for _ in 0..12 {
        d.key(&mut r, Key::ArrowRight);
        seen.push((r.spine, r.page));
    }
    eprintln!("forward: {seen:?}");
    assert!(
        seen.windows(2).all(|w| w[1] > w[0]),
        "pages move forward: {seen:?}"
    );
    let at = (r.spine, r.page);
    d.key(&mut r, Key::ArrowLeft);
    assert!(
        (r.spine, r.page) < at,
        "back from {at:?} to {:?}",
        (r.spine, r.page)
    );
    assert!(!d.key(&mut r, Key::Escape), "Esc closes");
    r.close(&d.ctx);
}

/// The settings panel used with the mouse: the Aa button opens it, the
/// serif box opens its list without closing the panel, a family picked in
/// the list is applied with the panel still open, a click on the page closes
/// the panel without turning the page, and Esc closes the panel, not the
/// reader.
#[test]
#[ignore]
fn settings_panel_clicks() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let mut d = Driver::new();
    let mut r = d.open(&book);
    d.ready(&mut r);
    let page = (r.spine, r.page);
    let aa = r.probe.aa.expect("Aa drawn");
    assert!(r.probe.serif_box.is_none(), "panel starts closed");
    d.click(&mut r, aa.center());
    let serif_box = r.probe.serif_box.expect("Aa opens the panel");
    d.snap("panel");
    d.click(&mut r, serif_box.center());
    assert!(
        r.probe.serif_box.is_some(),
        "the panel stays open when the font box is clicked"
    );
    let (item, family) = r.probe.serif_item.clone().expect("the font list opens");
    d.snap("panel-font-list");
    d.click(&mut r, item.center());
    assert_eq!(
        r.settings.serif.as_deref(),
        Some(family.as_str()),
        "the family clicked is chosen"
    );
    assert!(
        r.probe.serif_box.is_some(),
        "the panel stays open after choosing"
    );
    assert!(
        r.probe.serif_item.is_none(),
        "the list closes after choosing"
    );
    d.idle(&mut r, 5);
    d.snap("panel-font-chosen");
    let page = (r.spine, r.page).max(page);
    let on_page = pos2(d.size.x - 200.0, d.size.y / 2.0);
    d.click(&mut r, on_page);
    assert!(
        r.probe.serif_box.is_none(),
        "a click on the page closes the panel"
    );
    assert_eq!((r.spine, r.page), page, "and doesn't turn the page");
    d.click(&mut r, on_page);
    assert_ne!((r.spine, r.page), page, "the next click turns it");
    d.click(&mut r, aa.center());
    assert!(r.probe.serif_box.is_some(), "Aa opens the panel again");
    assert!(
        d.key(&mut r, Key::Escape),
        "Esc with the panel open keeps the reader"
    );
    assert!(r.probe.serif_box.is_none(), "Esc closes the panel");
    r.close(&d.ctx);
}

/// The contents menu: opens on a click, shows the entries (numbered when the
/// book repeats names) with their lengths, goes to the entry clicked, and Esc
/// closes the menu but not the reader.
#[test]
#[ignore]
fn contents_clicks() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.spine = 0;
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    let contents = r.probe.contents.expect("contents button drawn");
    d.click(&mut r, contents.center());
    let item = r.probe.toc_item.expect("the menu opens with its entries");
    d.snap("contents");
    let want = r.epub.toc[5].spine;
    let lengths = r
        .toc_lengths()
        .expect("lengths once every chapter is built");
    eprintln!(
        "entries 1-8: {:?}",
        (0..8.min(lengths.len()))
            .map(|i| (&r.toc_labels[i], lengths[i]))
            .collect::<Vec<_>>()
    );
    d.click(&mut r, item.center());
    d.ready(&mut r);
    assert_eq!(r.spine, want, "the entry clicked is opened");
    assert!(r.probe.toc_item.is_none(), "the menu closes");
    d.snap("contents-chosen");
    d.click(&mut r, contents.center());
    assert!(r.probe.toc_item.is_some(), "it opens again");
    assert!(
        d.key(&mut r, Key::Escape),
        "Esc with the menu open keeps the reader"
    );
    assert!(r.probe.toc_item.is_none(), "Esc closes the menu");
    r.close(&d.ctx);
}

#[test]
fn repeated_contents_names_are_numbered() {
    let e = |label: &str, depth| epub::TocEntry {
        label: label.into(),
        depth,
        spine: 0,
        fragment: None,
    };
    assert_eq!(toc_labels(&[e("一", 0), e("二", 0)]), ["一", "二"]);
    assert_eq!(
        toc_labels(&[
            e("偷偷", 0),
            e("偷偷", 0),
            e("卷二", 0),
            e("夜", 1),
            e("夜", 1)
        ]),
        ["1. 偷偷", "2. 偷偷", "3. 卷二", "1. 夜", "2. 夜"]
    );
}

#[test]
fn digits_grouped() {
    assert_eq!(group_digits(0), "0");
    assert_eq!(group_digits(999), "999");
    assert_eq!(group_digits(1000), "1,000");
    assert_eq!(group_digits(1234567), "1,234,567");
}
