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
        self.frame_with(r, events, egui::Modifiers::NONE)
    }

    /// A frame with `modifiers` held (egui keeps them from frame to frame).
    fn frame_with(
        &mut self,
        r: &mut Reader,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> bool {
        let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            events: all,
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

/// The dictionary popup with real dictionaries (those of `LOCALAPPDATA`'s
/// hanstat folder: point it at a scratch copy). Shift over a word opens the
/// popup in the same frame, with the word highlighted; Esc closes it and not
/// the reader; a click on the page closes it without turning the page; a
/// page turn closes it. Prints the frame times of 60 lookups.
#[test]
#[ignore]
fn dictionary_popup() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.spine = env("READER_SPINE")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    let t = Instant::now();
    while !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        assert!(
            t.elapsed().as_secs() < 30,
            "no dictionaries in {:?}",
            hanstat::dict::root()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    eprintln!(
        "{} dictionaries, scan {} characters",
        r.dict.dicts.len(),
        r.dict.scan_len
    );
    let l = r.layouts[&r.spine].clone();
    let ch = r.chapter(r.spine).unwrap();
    let (idx, off) = r.probe.sides[0];
    // Han characters of the first page, their boxes on screen.
    let spots: Vec<(u32, Rect)> = (0..ch.chars.len() as u32)
        .filter(|&s| hanstat::chars::is_han(ch.chars[s as usize]))
        .filter_map(|s| {
            l.char_rects(idx, s..s + 1)
                .first()
                .map(|b| (s, b.translate(off)))
        })
        .collect();
    assert!(spots.len() > 60, "a page of text");
    let shift = egui::Modifiers::SHIFT;
    let page = (r.spine, r.page);
    // Lookups: each in the frame that moves the pointer there.
    let mut frames = Vec::new();
    let mut micros = Vec::new();
    let mut layouts = Vec::new();
    let (mut shown, mut late, mut none, mut covered) = (0, Vec::new(), Vec::new(), 0);
    for (k, (src, rect)) in spots
        .iter()
        .step_by((spots.len() / 60).max(1))
        .take(60)
        .enumerate()
    {
        // Under the popup the pointer is on the popup, not the page (as in Yomitan).
        if r.dict.contains(rect.center()) {
            covered += 1;
            continue;
        }
        let t = Instant::now();
        d.frame_with(
            &mut r,
            vec![egui::Event::PointerMoved(rect.center())],
            shift,
        );
        frames.push(t.elapsed().as_secs_f64() * 1e3);
        if r.dict.highlight(r.spine).is_some_and(|(hs, _)| hs == *src) {
            micros.push(r.dict.last_micros);
            layouts.push(r.dict.last_layout_ms);
            shown += 1;
        } else {
            // Not in this frame: a slow lookup, or nothing found.
            let mut frames_more = 0;
            while frames_more < 60 && !r.dict.highlight(r.spine).is_some_and(|(hs, _)| hs == *src) {
                d.frame_with(&mut r, vec![], shift);
                std::thread::sleep(std::time::Duration::from_millis(2));
                frames_more += 1;
            }
            let c = ch.chars[*src as usize];
            if r.dict.highlight(r.spine).is_some_and(|(hs, _)| hs == *src) {
                late.push((c, frames_more, r.dict.last_micros));
            } else {
                none.push(c);
            }
        }
        if k == 3 {
            d.snap("popup");
        }
    }
    eprintln!("late: {late:?}; nothing found: {none:?}; under the popup: {covered}");
    frames.sort_by(|a, b| a.total_cmp(b));
    micros.sort();
    layouts.sort_by(|a, b| a.total_cmp(b));
    eprintln!(
        "popup entries built and laid out: median {:.2} ms, max {:.2} ms",
        layouts[layouts.len() / 2],
        layouts.last().unwrap()
    );
    {
        use std::sync::atomic::Ordering::Relaxed;
        let s = &super::render::STAGES;
        let n = s[4].load(Relaxed).max(1) as f64;
        eprintln!(
            "{} entries built: HTML {:.0} µs, parse {:.0} µs, style {:.0} µs, layout {:.0} µs each",
            n,
            s[0].load(Relaxed) as f64 / n,
            s[1].load(Relaxed) as f64 / n,
            s[2].load(Relaxed) as f64 / n,
            s[3].load(Relaxed) as f64 / n
        );
    }
    eprintln!(
        "{shown}/{} shown in the same frame; frame with lookup and popup: median {:.2} ms, p99 {:.2} ms, max {:.2} ms; lookup thread median {} µs, max {} µs",
        frames.len(),
        frames[frames.len() / 2],
        frames[frames.len() * 99 / 100],
        frames.last().unwrap(),
        micros[micros.len() / 2],
        micros.last().unwrap()
    );
    assert!(late.is_empty(), "shown in the frame that points at them");
    assert!(none.is_empty(), "every Han character is in some dictionary");
    // A word at the foot of the page: its popup goes above it, and with the
    // pointer held there it stays one popup on that word (it once looked up
    // a word of an entry scrolled out of view, under the pointer, and
    // flickered between the two).
    let (last_src, last_rect) = *spots.last().unwrap();
    for i in 0..20 {
        d.frame_with(
            &mut r,
            vec![egui::Event::PointerMoved(last_rect.center())],
            shift,
        );
        assert_eq!(r.dict.levels.len(), 1, "one popup (frame {i})");
        assert_eq!(
            r.dict.highlight(r.spine).map(|h| h.0),
            Some(last_src),
            "frame {i}"
        );
    }
    let pr = r.dict.levels[0].rect.unwrap();
    assert!(pr.bottom() <= last_rect.top(), "above its word");
    // Its top-right grip (away from the word) resizes it: up and right.
    let grip = pr.right_top() + vec2(-6.0, 6.0);
    let before = pr.size();
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    d.frame(&mut r, vec![egui::Event::PointerMoved(grip)]);
    d.frame(&mut r, vec![button(grip, true)]);
    for k in 1..=5 {
        d.frame(
            &mut r,
            vec![egui::Event::PointerMoved(
                grip + vec2(8.0 * k as f32, -6.0 * k as f32),
            )],
        );
    }
    d.frame(&mut r, vec![button(grip + vec2(40.0, -30.0), false)]);
    d.idle(&mut r, 3);
    let after = r.dict.levels[0].rect.unwrap().size();
    eprintln!(
        "resized {before:?} -> {after:?}; saved {:?}",
        r.settings.popup_size
    );
    assert!((after.x - before.x - 40.0).abs() < 2.0, "wider by the drag");
    assert!(
        (after.y - before.y - 30.0).abs() < 2.0,
        "taller by the drag"
    );
    assert!(
        r.settings.popup_size.is_some(),
        "the size goes in the settings"
    );
    assert!(r.dict.is_open(), "resizing keeps the popup");
    // Reset: the default size again.
    r.settings.popup_size = None;
    d.idle(&mut r, 3);
    let reset = r.dict.levels[0].rect.unwrap().size();
    assert!((reset - before).length() < 2.0, "{reset:?} {before:?}");
    // Without Shift, moving the pointer changes nothing.
    let before = r.dict.highlight(r.spine);
    d.frame(&mut r, vec![egui::Event::PointerMoved(spots[0].1.center())]);
    assert_eq!(r.dict.highlight(r.spine), before);
    // A word in the popup, with Shift: a child popup beside it.
    let pr = r.dict.levels[0].rect.expect("drawn");
    let mut child_at = None;
    'find: for y in (0..)
        .map(|k| pr.top() + 30.0 + k as f32 * 9.0)
        .take_while(|y| *y < pr.bottom() - 10.0)
    {
        for x in (0..)
            .map(|k| pr.left() + 16.0 + k as f32 * 11.0)
            .take_while(|x| *x < pr.right() - 16.0)
        {
            d.frame_with(&mut r, vec![egui::Event::PointerMoved(pos2(x, y))], shift);
            if r.dict.levels.len() == 2 {
                child_at = Some(pos2(x, y));
                break 'find;
            }
        }
    }
    let child_at = child_at.expect("a word in the popup opens a child popup");
    eprintln!(
        "child popup for {:?} at {child_at:?}",
        r.dict.levels[1].result.text
    );
    // Drawn in the frame that asked it (egui knew its area already).
    assert!(
        r.dict.levels[1]
            .rect
            .is_some_and(|c| c.is_positive() && c.left() > -1000.0),
        "drawn"
    );
    assert!(
        d.ctx
            .memory(|m| m.areas().visible_last_frame(&egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique(("reader-dict-popup", 1usize))
            ))),
        "visible in the frame that asked it"
    );
    d.snap("popup-child");
    // The child stays above its parent, even after the pointer was on the parent.
    let cr = r.dict.levels[1].rect.expect("child drawn");
    let both = cr.intersect(pr);
    if both.is_positive() {
        let off_child = [
            pr.left_top() + vec2(6.0, 6.0),
            pr.right_bottom() - vec2(30.0, 6.0),
        ]
        .into_iter()
        .find(|p| !cr.contains(*p))
        .unwrap();
        d.frame(&mut r, vec![egui::Event::PointerMoved(off_child)]);
        d.idle(&mut r, 2);
        let child = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique(("reader-dict-popup", 1usize)),
        );
        assert_eq!(
            d.ctx.layer_id_at(both.center()),
            Some(child),
            "the child is on top"
        );
    }
    // A click on the parent (off the child) closes the child.
    let cr = r.dict.levels[1].rect.expect("child drawn");
    let on_parent = [
        pr.left_top() + vec2(6.0, 6.0),
        pr.right_top() + vec2(-6.0, 6.0),
        pr.left_bottom() + vec2(6.0, -6.0),
        pr.right_bottom() + vec2(-6.0, -6.0),
    ]
    .into_iter()
    .find(|p| !cr.contains(*p))
    .expect("some of the parent is not under the child");
    d.click(&mut r, on_parent);
    assert_eq!(
        r.dict.levels.len(),
        1,
        "a click on the parent closes the child"
    );
    assert_eq!((r.spine, r.page), page, "and doesn't turn the page");
    // Open again; Esc closes the child only.
    d.frame_with(&mut r, vec![egui::Event::PointerMoved(child_at)], shift);
    assert_eq!(r.dict.levels.len(), 2, "the child opens again");
    // After that click raised the parent, the new child is still on top.
    d.idle(&mut r, 2);
    let (pr2, cr2) = (
        r.dict.levels[0].rect.unwrap(),
        r.dict.levels[1].rect.unwrap(),
    );
    let both = cr2.intersect(pr2);
    if both.is_positive() {
        let child = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique(("reader-dict-popup", 1usize)),
        );
        assert_eq!(
            d.ctx.layer_id_at(both.center()),
            Some(child),
            "a new child is above a clicked parent"
        );
    }
    assert!(d.key(&mut r, Key::Escape), "the reader stays open");
    assert_eq!(r.dict.levels.len(), 1, "Esc closes the child only");
    // ↓ scrolls the popup; the page stays.
    assert!(d.key(&mut r, Key::ArrowDown));
    assert!(r.dict.is_open(), "↓ scrolls the popup");
    assert_eq!((r.spine, r.page), page, "and doesn't turn the page");
    for _ in 0..4 {
        d.key(&mut r, Key::ArrowDown);
    }
    assert!(r.dict.levels[0].scroll_y > 0.0, "scrolled down");
    // Another word's popup starts at the top.
    let other = spots
        .iter()
        .find(|(s, rect)| {
            !r.dict.contains(rect.center()) && r.dict.highlight(r.spine).is_none_or(|h| h.0 != *s)
        })
        .unwrap();
    d.frame_with(
        &mut r,
        vec![egui::Event::PointerMoved(other.1.center())],
        shift,
    );
    d.idle(&mut r, 2);
    assert_eq!(
        r.dict.highlight(r.spine).map(|h| h.0),
        Some(other.0),
        "another word"
    );
    assert_eq!(
        r.dict.levels[0].scroll_y, 0.0,
        "a new popup starts at the top"
    );
    // Esc closes the popup, not the reader.
    assert!(r.dict.is_open());
    assert!(d.key(&mut r, Key::Escape), "the reader stays open");
    assert!(!r.dict.is_open(), "Esc closes the popup");
    // A click on the page closes it without turning the page.
    d.frame_with(
        &mut r,
        vec![egui::Event::PointerMoved(spots[10].1.center())],
        shift,
    );
    assert!(r.dict.is_open());
    d.click(&mut r, pos2(d.size.x - 60.0, d.size.y - 40.0));
    assert!(!r.dict.is_open(), "a click closes the popup");
    assert_eq!((r.spine, r.page), page, "and doesn't turn the page");
    // A page turn closes it.
    d.frame_with(
        &mut r,
        vec![egui::Event::PointerMoved(spots[20].1.center())],
        shift,
    );
    assert!(r.dict.is_open());
    d.key(&mut r, Key::ArrowRight);
    assert!(!r.dict.is_open(), "turning the page closes the popup");
    assert_ne!((r.spine, r.page), page);
    r.close(&d.ctx);
}

/// Popups of words picked by hand, to look at: `READER_WORDS` (comma-
/// separated) are shown over the first page, all dictionaries open, one PNG
/// each in `READER_OUT`.
#[test]
#[ignore]
fn popup_words() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let words = env("READER_WORDS").unwrap_or_else(|| "说话,行,画蛇添足,北京,一".into());
    let mut d = Driver::new();
    d.size = vec2(1200.0, 900.0);
    let mut r = d.open(&book);
    r.spine = env("READER_SPINE")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    while !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let l = r.layouts[&r.spine].clone();
    let ch = r.chapter(r.spine).unwrap();
    let (idx, off) = r.probe.sides[0];
    let first = (0..ch.chars.len() as u32)
        .filter(|&s| hanstat::chars::is_han(ch.chars[s as usize]))
        .find_map(|s| {
            l.char_rects(idx, s..s + 1)
                .first()
                .map(|b| b.translate(off))
        })
        .unwrap();
    let shift = egui::Modifiers::SHIFT;
    for (i, w) in words.split(',').enumerate() {
        d.frame_with(
            &mut r,
            vec![egui::Event::PointerMoved(first.center())],
            shift,
        );
        // The first lookup can miss its frame while the files are cold.
        for _ in 0..100 {
            if r.dict.is_open() {
                break;
            }
            d.frame_with(&mut r, vec![], shift);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(r.dict.is_open());
        r.dict.show_word(w);
        for _ in 0..100 {
            if r.dict.levels[0].result.text == w {
                break;
            }
            d.frame_with(&mut r, vec![], egui::Modifiers::NONE);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(r.dict.levels[0].result.text, w);
        r.dict.open_all();
        let t = Instant::now();
        d.frame_with(&mut r, vec![], egui::Modifiers::NONE);
        eprintln!(
            "{w}: {} groups, frame {:?}, entries {:.2} ms",
            r.dict.levels[0].result.groups.len(),
            t.elapsed(),
            r.dict.last_layout_ms
        );
        d.idle(&mut r, 3);
        if let Some(n) = env("READER_SCROLL").and_then(|v| v.parse().ok()) {
            r.dict.scroll_lines = n;
            // Images arrive decoded a few frames later.
            for _ in 0..20 {
                d.frame(&mut r, vec![]);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        d.snap(&format!("word-{i}-{w}"));
    }
    r.close(&d.ctx);
}

/// The popup's Anki buttons against a running Anki (read only: it asks
/// whether notes could be added, never adds one). Point `LOCALAPPDATA` at a
/// scratch folder whose `reader.json` has its `anki` settings set; words in
/// `READER_WORDS` (one already in Anki, one not) are shown in turn, their
/// statuses printed once Anki answers, one PNG each in `READER_OUT`.
#[test]
#[ignore]
fn anki_buttons() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let words = env("READER_WORDS").unwrap_or_else(|| "养精蓄锐,说话".into());
    let mut d = Driver::new();
    d.size = vec2(1200.0, 900.0);
    let mut r = d.open(&book);
    assert!(
        r.settings.anki.ready(),
        "set the anki settings in reader.json"
    );
    r.spine = env("READER_SPINE")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    while !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let l = r.layouts[&r.spine].clone();
    let ch = r.chapter(r.spine).unwrap();
    let (idx, off) = r.probe.sides[0];
    let first = (0..ch.chars.len() as u32)
        .filter(|&s| hanstat::chars::is_han(ch.chars[s as usize]))
        .find_map(|s| {
            l.char_rects(idx, s..s + 1)
                .first()
                .map(|b| b.translate(off))
        })
        .unwrap();
    let shift = egui::Modifiers::SHIFT;
    d.frame_with(
        &mut r,
        vec![egui::Event::PointerMoved(first.center())],
        shift,
    );
    assert!(r.dict.is_open());
    let lv = &r.dict.levels[0];
    let g = &lv.result.groups[0];
    eprintln!(
        "page word {}: {:?}, sentence {:?}",
        g.headword,
        r.dict.cards.status(g),
        lv.sentence_text()
    );
    for (i, w) in words.split(',').enumerate() {
        r.dict.show_word(w);
        let t = Instant::now();
        loop {
            d.frame_with(&mut r, vec![], egui::Modifiers::NONE);
            let res = &r.dict.levels[0].result;
            if res.text == w
                && !matches!(
                    r.dict.cards.status(&res.groups[0]),
                    None | Some(super::anki::Status::Checking)
                )
            {
                break;
            }
            assert!(t.elapsed().as_secs() < 15, "Anki never answered");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let res = r.dict.levels[0].result.clone();
        let st: Vec<String> = res
            .groups
            .iter()
            .map(|g| format!("{} {:?}", g.headword, r.dict.cards.status(g)))
            .collect();
        eprintln!("{w}: answered in {:?}: {st:?}", t.elapsed());
        d.idle(&mut r, 3);
        d.snap(&format!("anki-{i}-{w}"));
    }
    // A click on the first entry's +, with nothing listening at the address:
    // it tries to add, and fails (nothing is written anywhere).
    r.settings.anki.url = "http://127.0.0.1:9".into();
    r.dict.sync_cards(&r.settings.anki);
    d.idle(&mut r, 2);
    let rect = r.dict.levels[0].rect.unwrap();
    let side = (r.settings.size * 0.8).max(13.0) * 1.45;
    let plus = pos2(
        rect.max.x - 13.0 - side / 2.0,
        rect.min.y + 11.0 + side / 2.0,
    );
    d.frame(&mut r, vec![egui::Event::PointerMoved(plus)]);
    d.click(&mut r, plus);
    let t = Instant::now();
    loop {
        d.frame(&mut r, vec![]);
        let g = &r.dict.levels[0].result.groups[0];
        match r.dict.cards.status(g) {
            Some(super::anki::Status::Failed(e)) => {
                eprintln!("clicked +: {e}");
                break;
            }
            s => assert!(t.elapsed().as_secs() < 10, "no add tried: {s:?}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The setup window, with Anki's lists.
    r.settings.anki.url = hanstat::anki_connect::DEFAULT_URL.into();
    r.dict.sync_cards(&r.settings.anki);
    r.dict.close();
    r.dict.cards.open_setup();
    let t = Instant::now();
    while r.dict.cards.lists.is_none() || r.dict.cards.fields_of.is_none() {
        d.frame(&mut r, vec![]);
        assert!(t.elapsed().as_secs() < 10, "no lists");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    d.idle(&mut r, 3);
    eprintln!(
        "setup: {} fields, ready {}",
        r.settings.anki.fields.len(),
        r.settings.anki.ready()
    );
    d.snap("anki-setup");
    r.close(&d.ctx);
}

/// The Dictionaries window from an empty start (point `LOCALAPPDATA` at an
/// empty scratch folder; `READER_DICTS` is the folder of zips): CC-CEDICT and
/// Danmei imported through it, then looked up; CC-CEDICT turned off (no
/// definitions left), Danmei deleted.
#[test]
#[ignore]
fn dictionaries_window() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let zips = std::path::PathBuf::from(env("READER_DICTS").expect("set READER_DICTS"));
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.spine = 5;
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    assert!(r.dicts.listed().is_empty(), "an empty start");
    r.dicts.open = true;
    d.idle(&mut r, 3);
    d.snap("dicts-empty");
    let t = Instant::now();
    r.dicts
        .import(vec![zips.join("CC-CEDICT.zip"), zips.join("Danmei.zip")]);
    while r.dicts.busy() || !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        assert!(t.elapsed().as_secs() < 60, "imports finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    eprintln!(
        "imported in {:?}: {:?}; {:?}",
        t.elapsed(),
        r.dicts.listed(),
        r.dicts.notes()
    );
    assert_eq!(r.dicts.listed().len(), 2);
    d.idle(&mut r, 3);
    d.snap("dicts-two");
    r.dicts.open = false;
    // egui finds what is under the pointer from the last frame's windows.
    d.idle(&mut r, 2);
    // Looked up, with the rank.
    let l = r.layouts[&r.spine].clone();
    let ch = r.chapter(r.spine).unwrap();
    let (idx, off) = r.probe.sides[0];
    let spot = (0..ch.chars.len() as u32)
        .filter(|&s| hanstat::chars::is_han(ch.chars[s as usize]))
        .find_map(|s| {
            l.char_rects(idx, s..s + 1)
                .first()
                .map(|b| b.translate(off))
        })
        .unwrap();
    d.frame_with(
        &mut r,
        vec![egui::Event::PointerMoved(spot.center())],
        egui::Modifiers::SHIFT,
    );
    assert!(r.dict.is_open(), "found in CC-CEDICT");
    let g = &r.dict.levels[0].result.groups[0];
    assert!(!g.freqs.is_empty(), "with a Danmei rank");
    d.key(&mut r, Key::Escape);
    // CC-CEDICT set to start collapsed: the popup is told.
    {
        let id = r
            .dicts
            .listed()
            .iter()
            .find(|x| x.1.starts_with("CC-CEDICT"))
            .unwrap()
            .0
            .clone();
        let settings = r.settings.clone();
        r.dicts.edit(&r.dict, &settings, |m| {
            m.dicts.iter_mut().find(|e| e.id == id).unwrap().collapsed = true;
        });
        let t = Instant::now();
        while !r.dict.dicts.iter().any(|d| d.collapsed) {
            d.frame(&mut r, vec![]);
            assert!(t.elapsed().as_secs() < 10, "the popup sees the setting");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        r.dicts.open = true;
        d.idle(&mut r, 3);
        d.snap("dicts-collapsed");
        r.dicts.open = false;
        d.idle(&mut r, 2);
    }
    // CC-CEDICT off: nothing to look up in.
    let (cedict, danmei) = {
        let l = r.dicts.listed();
        (
            l.iter()
                .find(|x| x.1.starts_with("CC-CEDICT"))
                .unwrap()
                .0
                .clone(),
            l.iter().find(|x| x.1 == "Danmei").unwrap().0.clone(),
        )
    };
    let settings = r.settings.clone();
    r.dicts.edit(&r.dict, &settings, |m| {
        m.dicts.iter_mut().find(|e| e.id == cedict).unwrap().enabled = false;
    });
    let t = Instant::now();
    while r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        assert!(t.elapsed().as_secs() < 10);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Danmei deleted: its folder is gone.
    r.dicts.delete(&danmei, &mut r.dict, &settings);
    assert_eq!(r.dicts.listed().len(), 1);
    assert!(!hanstat::dict::root().join(&danmei).exists());
    r.dicts.open = true;
    d.idle(&mut r, 3);
    d.snap("dicts-after");
    r.close(&d.ctx);
}

/// Over the network, from an empty app folder (point `LOCALAPPDATA` at
/// one): the reader downloads CC-CEDICT by itself; once deleted, it is not
/// downloaded again.
#[test]
#[ignore]
fn default_dictionary() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let mut d = Driver::new();
    let mut r = d.open(&book);
    d.ready(&mut r);
    let t = Instant::now();
    while r.dicts.busy() || !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        assert!(
            t.elapsed().as_secs() < 120,
            "downloaded: {:?}",
            r.dicts.notes()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    eprintln!("installed in {:?}: {:?}", t.elapsed(), r.dicts.notes());
    let listed = r.dicts.listed();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].1.starts_with("CC-CEDICT"), "{listed:?}");
    let settings = r.settings.clone();
    r.dicts.delete(&listed[0].0, &mut r.dict, &settings);
    r.close(&d.ctx);
    // Opened again: nothing downloaded.
    let mut r = d.open(&book);
    d.ready(&mut r);
    d.idle(&mut r, 10);
    assert!(
        !r.dicts.busy() && r.dicts.listed().is_empty(),
        "a deleted default stays deleted"
    );
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

/// Reading aloud through Azure with a real key, the sound device open but
/// silent: `HANSTAT_TTS_TEST` is a file with the key on its first line and
/// the region on its second (the key is used, never stored). Run with
/// `LOCALAPPDATA` at a scratch folder (the audio cache goes there) and
/// `HANSTAT_NO_UPDATE=1`; `READER_SPINE` picks the chapter.
///
/// From the page's first sentence, the sentence heard is the one
/// highlighted and the reading goes on; a click on a word of a later
/// sentence reads from that one without turning the page; the popup pauses
/// and resumes it; the page turns when the sentence heard leaves it; Stop
/// stops; a short chapter made ahead of time has every sentence saved.
#[test]
#[ignore]
fn read_aloud() {
    use std::sync::atomic::Ordering::Relaxed;
    use tts::PlayState;
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let file = env("HANSTAT_TTS_TEST").expect("set HANSTAT_TTS_TEST");
    let text = std::fs::read_to_string(file).unwrap();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let (key, region) = (lines.next().unwrap(), lines.next().unwrap());
    tts::audio::MUTE.store(true, Relaxed);
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.settings.tts.region = region.to_owned();
    r.tts.use_key(key);
    r.spine = env("READER_SPINE")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    let spine = r.spine;
    let page = r.page;
    let l = r.layouts[&spine].clone();
    let ch = r.chapter(spine).unwrap();
    let sents = tts::sentences(&ch.chars);
    let text_of = |i: usize| -> String {
        ch.chars[sents[i].start as usize..sents[i].end as usize]
            .iter()
            .collect()
    };
    let wait =
        |d: &mut Driver, r: &mut Reader, secs: u64, what: &str, ok: &dyn Fn(&Reader) -> bool| {
            let t = Instant::now();
            while !ok(r) {
                d.frame(r, vec![]);
                assert!(
                    t.elapsed().as_secs() < secs,
                    "{what}: state {:?}, at {:?}",
                    r.tts.state(),
                    r.tts.at()
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
    let index = |r: &Reader| r.tts.at().map(|(_, rg)| tts::sentence_at(&sents, rg.start));

    // From the page.
    let first = l.page_first_src(page).unwrap();
    let i0 = tts::sentence_at(&sents, first);
    let t = Instant::now();
    r.tts.play_from(spine, first, &r.settings.tts);
    wait(&mut d, &mut r, 30, "starts", &|r| {
        r.tts.state() == PlayState::Playing
    });
    eprintln!(
        "first audio after {:.2} s: {:?}",
        t.elapsed().as_secs_f32(),
        text_of(i0)
    );
    assert_eq!(r.tts.at(), Some((spine, sents[i0].clone())));
    d.idle(&mut r, 2);
    d.snap("tts-reading");
    wait(&mut d, &mut r, 60, "goes on", &|r| {
        index(r).is_some_and(|i| i > i0)
    });
    eprintln!(
        "then: sentence {} {:?} (started at {i0})",
        index(&r).unwrap(),
        text_of(index(&r).unwrap())
    );

    // A click on a later sentence of the page.
    let on_page = |i: usize| l.page_of_src(sents[i].start) == Some(page);
    let k = (i0 + 3..sents.len())
        .find(|&i| on_page(i) && sents[i].len() > 2)
        .expect("a later sentence on the page");
    let (_, off) = *r.probe.sides.iter().find(|(p, _)| *p == page).unwrap();
    let at = l.char_rects(page, sents[k].start + 1..sents[k].start + 2)[0]
        .translate(off)
        .center();
    d.click(&mut r, at);
    assert_eq!(
        (r.spine, r.page),
        (spine, page),
        "the click doesn't turn the page"
    );
    wait(&mut d, &mut r, 30, "reads the clicked sentence", &|r| {
        r.tts.state() == PlayState::Playing && index(r) == Some(k)
    });
    eprintln!("clicked: {:?}", text_of(k));
    d.snap("tts-clicked");

    // The dictionary popup (a word looked up with Shift) pauses the reading,
    // and closing it (Esc) resumes it.
    let t = Instant::now();
    while !r.dict.has_terms() {
        d.frame(&mut r, vec![]);
        assert!(
            t.elapsed().as_secs() < 30,
            "no dictionaries in {:?}",
            hanstat::dict::root()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let word = (sents[k].start..sents[k].end)
        .find(|&s| hanstat::chars::is_han(ch.chars[s as usize]))
        .unwrap();
    let at = l.char_rects(page, word..word + 1)[0]
        .translate(off)
        .center();
    let shift = egui::Modifiers::SHIFT;
    d.frame_with(&mut r, vec![egui::Event::PointerMoved(at)], shift);
    // The first lookup can take a few frames in a debug build.
    let t = Instant::now();
    while !r.dict.is_open() && t.elapsed().as_secs() < 5 {
        d.frame_with(&mut r, vec![], shift);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    d.frame(&mut r, vec![]);
    assert!(r.dict.is_open(), "the popup opened");
    wait(&mut d, &mut r, 5, "paused by the popup", &|r| {
        r.tts.state() == PlayState::Paused
    });
    let held = r.tts.at();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    d.idle(&mut r, 5);
    assert_eq!(r.tts.at(), held, "nothing heard while paused");
    d.snap("tts-popup");
    assert!(
        d.key(&mut r, Key::Escape),
        "Esc closes the popup, not the reader"
    );
    assert!(!r.dict.is_open());
    wait(&mut d, &mut r, 10, "resumes", &|r| {
        r.tts.state() == PlayState::Playing
    });
    // Not with the setting off.
    r.settings.tts.pause_for_popup = false;
    d.frame_with(&mut r, vec![egui::Event::PointerMoved(at)], shift);
    for _ in 0..5 {
        d.frame_with(&mut r, vec![], shift);
    }
    d.idle(&mut r, 10);
    assert!(r.dict.is_open());
    assert_ne!(r.tts.state(), PlayState::Paused, "the setting is off");
    d.key(&mut r, Key::Escape);
    r.settings.tts.pause_for_popup = true;

    // The page turns with the reading: from the last sentence starting on it.
    let step = if r.last_geom.is_some_and(|g| g.spread) {
        2
    } else {
        1
    };
    let last = (0..sents.len())
        .rev()
        .find(|&i| {
            l.page_of_src(sents[i].start)
                .is_some_and(|p| p < page + step)
        })
        .unwrap();
    r.tts.play_from(spine, sents[last].start, &r.settings.tts);
    wait(&mut d, &mut r, 90, "the page follows", &|r| {
        (r.spine, r.page) > (spine, page) && r.target.is_none()
    });
    eprintln!(
        "followed to {:?} reading {:?}",
        (r.spine, r.page),
        r.tts.at().map(|(s, rg)| (s, rg.start))
    );
    d.idle(&mut r, 2);
    d.snap("tts-followed");
    r.tts.stop();
    d.idle(&mut r, 20);
    assert!(!r.tts.active());
    assert_eq!(r.tts.at(), None);

    // Ahead of time: the shortest chapter with something to read.
    let target = (0..r.epub.spine.len())
        .filter_map(|i| {
            let c = r.chapter(i)?;
            let n = tts::sentences(&c.chars).len();
            (n >= 3).then_some((n, i))
        })
        .min()
        .unwrap()
        .1;
    let s = r.settings.tts.clone();
    r.tts.pregenerate(&s, vec![target], "test chapter");
    let b = r.tts.batch().unwrap();
    wait(&mut d, &mut r, 300, "made ahead", &|_| b.finished());
    let (total, cached, done, failed) = (
        b.total.load(Relaxed),
        b.cached.load(Relaxed),
        b.done.load(Relaxed),
        b.failed.load(Relaxed),
    );
    eprintln!(
        "ahead of time, chapter {target}: {total} sentences, {cached} saved before, {done} made, {failed} failed, ~{} characters billed",
        b.billed.load(Relaxed)
    );
    assert_eq!(failed, 0, "{:?}", b.error.lock().unwrap());
    let c = r.chapter(target).unwrap();
    let speech = tts::Speech {
        voice: s.voice(false),
        rate: s.rate,
    };
    for rg in tts::sentences(&c.chars) {
        let t: String = c.chars[rg.start as usize..rg.end as usize].iter().collect();
        assert!(
            tts::clip_path(r.tts.dir(), speech.key(&t)).exists(),
            "saved in the book's folder: {t}"
        );
    }
    // The panel, with the voices fetched from Azure.
    r.tts.panel_open = true;
    let t = Instant::now();
    while t.elapsed().as_secs() < 10 {
        d.frame(&mut r, vec![]);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    d.idle(&mut r, 3);
    d.snap("tts-panel");
    r.close(&d.ctx);
}
