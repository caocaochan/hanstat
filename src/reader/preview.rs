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
//! `READER_SANS` (family names), `READER_SANS_DEFAULT=1`, `READER_TRAD=1` and
//! `READER_THEME` (a theme id, e.g. `catppuccin-mocha`) change the setup.

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
    let th = &theme::THEMES[theme::index(&env("READER_THEME").unwrap_or_default())];

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
            text: th.ink,
            link: th.accent,
            rule: th.muted,
            page: th.page,
            dark: th.dark,
        };
        let mut info = String::new();
        let scene = crate::raster::lay_out(
            &window,
            w,
            2.0,
            th.desk,
            |d| *d = defs.clone(),
            |s| s.visuals = th.visuals(),
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
                ui.painter().rect_filled(area, 0.0, th.desk);
                let first = page - page % step;
                for side in 0..step {
                    let page_rect = g.pages[side];
                    draw_page(ui, th, page_rect, side, g.spread);
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
                        paint_item(&mut images, th, &painter, &l.items[i], off, 2.0);
                    }
                    let small = egui::FontId::proportional((settings.size * 0.62).max(11.0));
                    ui.painter().text(
                        pos2(page_rect.center().x, page_rect.bottom() - g.inset.y * 0.5),
                        egui::Align2::CENTER_CENTER,
                        format!("{} / {}", idx + 1, l.pages.len()),
                        small,
                        th.muted,
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

    /// `n` frames with no events at all (not even the modifiers, which make
    /// egui ask for the next frame at once), and how long the last one
    /// asked to wait for the next (seconds).
    fn quiet(&mut self, r: &mut Reader, n: usize) -> f64 {
        for _ in 0..n {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                predicted_dt: 1.0 / 60.0,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let mut out = self.ctx.run_ui(raw, |ui| {
                r.ui(ui);
            });
            self.screen.absorb(&mut out);
            self.last = Some(out);
        }
        self.last.as_ref().unwrap().viewport_output[&egui::ViewportId::ROOT]
            .repaint_delay
            .as_secs_f64()
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
        let a = self.press(r, p, egui::PointerButton::Primary);
        let b = self.release(r, p, egui::PointerButton::Primary);
        self.idle(r, 30);
        a && b
    }

    fn press(&mut self, r: &mut Reader, pos: Pos2, button: egui::PointerButton) -> bool {
        self.frame(
            r,
            vec![
                egui::Event::PointerMoved(pos),
                pointer_button(pos, button, true),
            ],
        )
    }

    fn release(&mut self, r: &mut Reader, pos: Pos2, button: egui::PointerButton) -> bool {
        self.frame(r, vec![pointer_button(pos, button, false)])
    }

    fn drag(&mut self, r: &mut Reader, from: Pos2, to: Pos2) {
        self.press(r, from, egui::PointerButton::Primary);
        self.frame(r, vec![egui::Event::PointerMoved(to)]);
        self.release(r, to, egui::PointerButton::Primary);
    }

    fn text_menu(&mut self, r: &mut Reader, p: Pos2) -> [Pos2; 2] {
        self.press(r, p, egui::PointerButton::Secondary);
        self.release(r, p, egui::PointerButton::Secondary);
        self.idle(r, 10);
        r.probe
            .text_menu
            .map(|rect| rect.expect("text menu button").center())
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

    /// The last frame as a picture, saved as `name`.png in `READER_OUT`
    /// too, if set.
    fn shot(&mut self, name: &str) -> crate::raster::Scene {
        let mut out = self.last.take().expect("a frame drawn");
        let scene = self.screen.picture(&self.ctx, &mut out, self.size);
        if let Some(dir) = &self.out_dir {
            std::fs::create_dir_all(dir).unwrap();
            scene.write_png(&dir.join(format!("{name}.png"))).unwrap();
        }
        scene
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

fn pointer_button(pos: Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

const RIGHT_PAGE: &str = "右页开头。另一个句子。";

fn write_zip(path: &Path, files: &[(&str, &str)]) {
    use std::io::Write;

    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for &(name, body) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn selection_book(d: &mut Driver, dir: &Path) -> (Reader, String) {
    let path = dir.join("selection.epub");
    let left = format!("{}第一句。", "山川日月".repeat(10));
    let chapter = format!(
        r#"<html><meta charset="utf-8"><body><p style="background:#eee">{left}<br>换行。<a href="second.xhtml">脚注。</a></p><p style="break-before:page">{RIGHT_PAGE}</p><p style="break-before:page">下一页。</p></body></html>"#
    );
    write_zip(
        &path,
        &[
            (
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
            ),
            (
                "book.opf",
                r#"<package><metadata><title>Text selection test</title></metadata><manifest><item id="a" href="first.xhtml" media-type="application/xhtml+xml"/><item id="b" href="second.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#,
            ),
            ("first.xhtml", &chapter),
            (
                "second.xhtml",
                r#"<html><meta charset="utf-8"><body><p>脚注内容。</p></body></html>"#,
            ),
        ],
    );
    let mut r = d.open(path.to_str().unwrap());
    r.settings = Settings::default();
    r.go_chapter(0, Target::Offset(0));
    d.ready(&mut r);
    assert_eq!(r.probe.sides.len(), 2, "fixture opens in a two-page spread");
    (r, left)
}

fn displayed_char(r: &Reader, src: u32) -> Rect {
    let layout = &r.layouts[&r.spine];
    r.probe
        .sides
        .iter()
        .find_map(|&(page, off)| {
            layout
                .char_rects(page, src..src + 1)
                .first()
                .map(|rect| rect.translate(off))
        })
        .expect("character on the visible spread")
}

fn clipboard_text(d: &Driver) -> Option<&str> {
    d.last
        .as_ref()
        .unwrap()
        .platform_output
        .commands
        .iter()
        .find_map(|command| {
            if let egui::OutputCommand::CopyText(text) = command {
                Some(text.as_str())
            } else {
                None
            }
        })
}

#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn reader_text_selection() {
    let temp = tempfile::tempdir().unwrap();
    let mut d = Driver::new();
    let (mut r, left) = selection_book(&mut d, temp.path());
    let layout = &r.layouts[&0];
    assert!(layout.char_rects(0, 0..left.chars().count() as u32).len() > 1);
    let ch = r.chapter(0).unwrap();
    let sentences = tts::sentences(&ch.chars);
    let target = sentences.last().unwrap();
    let right_sentence = &sentences[sentences.len() - 2];
    assert_eq!(ch.chars[right_sentence.start as usize], '另');
    let start = displayed_char(&r, 0);
    let end = displayed_char(&r, right_sentence.start - 1);
    let from = pos2(start.left() + start.width() * 0.2, start.center().y);
    let to = pos2(end.right() - end.width() * 0.2, end.center().y);
    let expected = format!("{left}\n换行。脚注。\n右页开头。");
    let selected = Some(0..right_sentence.start);
    let primary = egui::PointerButton::Primary;

    d.drag(&mut r, from, to);
    assert_eq!(r.selection, selected);
    assert_eq!((r.spine, r.page), (0, 0));
    d.frame(&mut r, vec![egui::Event::Copy]);
    assert_eq!(clipboard_text(&d), Some(expected.as_str()));

    d.drag(&mut r, to, from);
    assert_eq!(r.selection, selected);
    let right_text = displayed_char(&r, right_sentence.start + 2).center();
    let secondary = egui::PointerButton::Secondary;
    d.press(&mut r, right_text, secondary);
    d.frame(
        &mut r,
        vec![
            pointer_button(right_text, secondary, false),
            egui::Event::PointerMoved(from),
        ],
    );
    d.idle(&mut r, 10);
    assert_eq!(r.selection, selected);
    let menu = r.text_menu.unwrap();
    assert_eq!(menu.position, right_text);
    let spot = menu.target.unwrap();
    assert_eq!((spot.spine, spot.src), (0, right_sentence.start));
    let copy = r.probe.text_menu[0].unwrap().center();
    d.press(&mut r, copy, primary);
    d.release(&mut r, copy, primary);
    assert_eq!(clipboard_text(&d), Some(expected.as_str()));
    assert_eq!(r.selection, selected);

    let page = r.drawn.unwrap().pages[1];
    let margin = pos2(page.right() - 10.0, page.center().y);
    let [_, play] = d.text_menu(&mut r, margin);
    assert!(r.text_menu.unwrap().target.is_none());
    d.click(&mut r, play);
    assert!(!r.tts.active());
    let text_menu_id = egui::Id::unique("reader-text-menu");
    if egui::Popup::is_id_open(&d.ctx, text_menu_id) {
        assert!(d.key(&mut r, Key::Escape));
    }

    r.settings.size = 18.0;
    d.idle(&mut r, 20);
    assert!(r.drawn.unwrap().spread);
    assert_eq!(r.params.as_ref().unwrap().base, 18.0);
    assert_eq!(r.selection, selected, "reflow preserves chapter offsets");
    let right_text = displayed_char(&r, right_sentence.start + 2).center();
    d.text_menu(&mut r, right_text);
    d.click(&mut r, margin);
    assert_eq!(r.page, 0, "outside click only dismisses the menu");
    assert_eq!(r.selection, selected);

    d.click(&mut r, right_text);
    assert_eq!(r.page, 0, "plain text click stays put with TTS stopped");
    assert!(r.selection.is_none());
    let [copy, _] = d.text_menu(&mut r, right_text);
    d.press(&mut r, copy, primary);
    d.release(&mut r, copy, primary);
    assert_eq!(
        clipboard_text(&d),
        None,
        "Copy is disabled without a selection"
    );
    if egui::Popup::is_id_open(&d.ctx, text_menu_id) {
        assert!(d.key(&mut r, Key::Escape));
    }

    let link = r.layouts[&0].page_links(0).next().unwrap().rect;
    let off = r.probe.sides[0].1;
    d.drag(&mut r, link.translate(off).center(), right_text);
    assert_eq!((r.spine, r.page), (0, 0), "dragging a link selects text");
    assert!(r.selection.is_some());
    d.click(&mut r, margin);
    assert_eq!(r.page, 0, "a click on the page doesn't turn it");
    assert!(r.selection.is_none());
    let beside = pos2(page.right() + 9.0, page.center().y);
    d.click(&mut r, beside);
    assert_eq!(r.page, 2, "a click right of the pages turns them");
    assert!(r.selection.is_none());
    assert!(r.layouts[&0].shows_text(r.page, target));
    let left_of = pos2(r.drawn.unwrap().pages[0].left() - 9.0, page.center().y);
    d.click(&mut r, left_of);
    assert_eq!(r.page, 0, "a click left of the pages turns back");
    d.frame(
        &mut r,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, -2.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(r.page, 2);
    d.key(&mut r, Key::ArrowLeft);
    d.click(&mut r, link.translate(off).center());
    assert_eq!(r.spine, 1, "plain click follows the EPUB link");

    use hanstat::dict::{import, manage};
    let dict_zip = temp.path().join("test-dictionary.zip");
    let root = temp.path().join("dictionaries");
    let id = "0000000000000001";
    write_zip(
        &dict_zip,
        &[
            (
                "index.json",
                r#"{"title":"Test","revision":"1","format":3}"#,
            ),
            (
                "term_bank_1.json",
                r#"[["脚注","jiǎozhù","","",1,["footnote"],1,""]]"#,
            ),
        ],
    );
    import::import(
        &dict_zip,
        &manage::tmp_dir(&root, id),
        &import::Progress::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    manage::swap(&root, id).unwrap();
    manage::add_to_manifest(&root, id).unwrap();
    r.dict = popup::Popup::with_root(&d.ctx, None, root);

    let footnote = displayed_char(&r, 0).center();
    let since = Instant::now();
    while !r.dict.is_open() {
        d.frame_with(
            &mut r,
            vec![egui::Event::PointerMoved(footnote)],
            egui::Modifiers::SHIFT,
        );
        assert!(
            since.elapsed().as_secs() < 5,
            "dictionary lookup did not open"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(r.dict.highlight(1), Some((0, 2)));
    r.dict.unloader().unload(vec![id.into()]).unwrap();
}

#[test]
#[ignore = "requires ffmpeg and a default audio output device"]
fn reader_text_menu_playback() {
    let temp = tempfile::tempdir().unwrap();
    let mut d = Driver::new();
    let (mut r, left) = selection_book(&mut d, temp.path());
    r.tts.set_test_cache_dir(temp.path().to_owned());
    r.tts.use_key("");
    r.settings.tts.voice = "zh-CN-YunxiNeural".into();
    r.settings.tts.rate = -15;
    let ch = r.chapter(0).unwrap();
    let sentences = tts::sentences(&ch.chars);
    let target = sentences[sentences.len() - 2].clone();
    let selected_text: String = ch.chars[target.start as usize..target.end as usize]
        .iter()
        .collect();
    assert_eq!(selected_text, "另一个句子。");
    let clicked_text = displayed_char(&r, target.start + 2).center();
    let old_text = displayed_char(&r, 0).center();

    // These clips exist only for the voice and speed in effect when Play is pressed.
    let speech = tts::Speech {
        voice: r.settings.tts.voice.clone(),
        rate: 23,
        local: None,
    };
    let first_clip = tts::clip_path(r.tts.dir(), speech.key(&left), speech.ext());
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=24000:cl=mono",
            "-t",
            "6",
            "-c:a",
            "libmp3lame",
        ])
        .arg(&first_clip)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::copy(
        &first_clip,
        tts::clip_path(r.tts.dir(), speech.key(&selected_text), speech.ext()),
    )
    .unwrap();

    let [_, play] = d.text_menu(&mut r, clicked_text);
    d.frame(&mut r, vec![egui::Event::PointerMoved(play)]);
    r.settings.tts.rate = speech.rate;
    d.click(&mut r, play);

    let wait_for = |d: &mut Driver, r: &mut Reader, sentence: &Range<u32>| {
        let since = Instant::now();
        while r.tts.state() != tts::PlayState::Playing
            || r.tts.at().as_ref().map(|(_, range)| range) != Some(sentence)
        {
            d.frame(r, vec![]);
            assert!(
                since.elapsed().as_secs() < 5,
                "{sentence:?}, {:?}, {:?}",
                r.tts.at(),
                r.tts.error()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    wait_for(&mut d, &mut r, &target);

    d.drag(&mut r, clicked_text, old_text);
    d.idle(&mut r, 2);
    assert!(r.selection.is_some());
    assert_eq!((r.spine, r.page), (0, 0));
    assert_eq!(r.tts.at(), Some((0, target)));
    d.click(&mut r, old_text);
    wait_for(&mut d, &mut r, &sentences[0]);
    assert_eq!((r.spine, r.page), (0, 0));
    assert!(r.selection.is_none());
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
    let g = r.drawn.unwrap();
    let last = if g.spread { g.pages[1] } else { g.pages[0] };
    let beside = pos2(last.right() + 9.0, d.size.y / 2.0);
    d.click(&mut r, beside);
    assert!(
        r.probe.serif_box.is_none(),
        "a click beside the pages closes the panel"
    );
    assert_eq!((r.spine, r.page), page, "and doesn't turn the page");
    d.click(&mut r, beside);
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

/// The theme chooser: the Aa panel's theme box lists every theme, a click
/// on one applies it to the bar and menus and lays the pages out again in its
/// ink.
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn theme_chooser() {
    let temp = tempfile::tempdir().unwrap();
    let mut d = Driver::new();
    let (mut r, _) = selection_book(&mut d, temp.path());
    let first = &theme::THEMES[0];
    assert_eq!(theme::get(&d.ctx).id, first.id);
    assert!(!d.ctx.style_of(d.ctx.theme()).visuals.dark_mode);
    d.snap("theme-0-default");
    let aa = r.probe.aa.expect("Aa drawn");
    d.click(&mut r, aa.center());
    let theme_box = r.probe.theme_box.expect("Aa opens the panel");
    d.click(&mut r, theme_box.center());
    assert_eq!(
        r.probe.theme_items.len(),
        theme::THEMES.len(),
        "the list shows every theme"
    );
    let mocha = theme::index("catppuccin-mocha");
    d.idle(&mut r, 10);
    assert!(
        r.probe.theme_items.iter().all(|(_, _, shown)| *shown),
        "the whole list shows without scrolling: {:?}",
        r.probe.theme_items
    );
    let (item, _, _) = *r
        .probe
        .theme_items
        .iter()
        .find(|(_, i, _)| *i == mocha)
        .unwrap();
    d.snap("theme-1-list");
    d.click(&mut r, item.center());
    assert_eq!(r.settings.theme, "catppuccin-mocha");
    assert!(r.probe.theme_items.is_empty(), "the list closes");
    assert!(r.probe.theme_box.is_some(), "the panel stays open");
    let th = theme::get(&d.ctx);
    assert_eq!(th.id, "catppuccin-mocha");
    assert!(
        d.ctx.style_of(d.ctx.theme()).visuals.dark_mode,
        "the bar and menus go dark"
    );
    d.idle(&mut r, 20);
    d.snap("theme-2-chosen");
    // The page is laid out again in the theme's ink.
    let l = r.layouts[&0].clone();
    let ink: Vec<Color32> = l
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Text { galley, .. } => Some(galley.job.sections[0].format.color),
            _ => None,
        })
        .collect();
    assert!(
        !ink.is_empty() && ink.iter().all(|c| *c == th.ink),
        "{ink:?}"
    );
    let fills: Vec<Color32> = l
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fill { color, rect, .. } if rect.is_positive() => Some(*color),
            _ => None,
        })
        .collect();
    // (The fixture's #eee box is near white, left out on every page.)
    assert!(
        fills.iter().all(|c| c.r() < 0x30),
        "no light boxes on a dark page: {fills:?}"
    );
    d.key(&mut r, Key::Escape);
    d.idle(&mut r, 10);
    d.snap("theme-3-pages");
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

/// Import a small dictionary of `files` as `id` under `root`.
fn import_test_dict(dir: &Path, root: &Path, id: &str, files: &[(&str, &str)]) {
    use hanstat::dict::{import, manage};
    let zip = dir.join(format!("{id}.zip"));
    write_zip(&zip, files);
    import::import(
        &zip,
        &manage::tmp_dir(root, id),
        &import::Progress::default(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    manage::swap(root, id).unwrap();
    manage::add_to_manifest(root, id).unwrap();
}

/// The popup in the themes: the Aa panel's popup theme box lists "Same as
/// pages" and every theme, a click on one saves it and the open popup takes
/// its colours at once. With small dictionaries made here (tags, a list, a
/// cross-reference, a closed `<details>`, a collapsed dictionary, a
/// frequency rank, Anki buttons in each state, a child popup), one PNG per
/// theme in `READER_OUT`: `READER_POPUP_THEMES` (comma-separated ids)
/// adds themes to the light and dark ones always shown.
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn popup_themes() {
    let temp = tempfile::tempdir().unwrap();
    let mut d = Driver::new();
    let (mut r, _) = selection_book(&mut d, temp.path());
    assert_eq!(r.settings.popup_theme, popup::SAME_AS_PAGES, "the default");

    // The chooser: "Same as pages", then every theme.
    let aa = r.probe.aa.expect("Aa drawn");
    d.click(&mut r, aa.center());
    let theme_box = r.probe.theme_box.expect("Aa opens the panel");
    let popup_box = r.probe.popup_theme_box.expect("the popup theme box");
    assert!(
        (popup_box.top() - theme_box.bottom()).abs() < 16.0 && popup_box.left() == theme_box.left(),
        "right under Theme: {theme_box:?} {popup_box:?}"
    );
    d.click(&mut r, popup_box.center());
    assert_eq!(r.probe.popup_theme_items.len(), theme::THEMES.len());
    let same = r.probe.popup_same.expect("Same as pages listed");
    d.idle(&mut r, 10);
    assert!(
        r.probe.popup_theme_items.iter().all(|(_, _, shown)| *shown),
        "the whole list shows without scrolling"
    );
    d.snap("popup-theme-list");
    let mocha = theme::index("catppuccin-mocha");
    let (item, _, _) = *r
        .probe
        .popup_theme_items
        .iter()
        .find(|(_, i, _)| *i == mocha)
        .unwrap();
    d.click(&mut r, item.center());
    assert_eq!(r.settings.popup_theme, "catppuccin-mocha");
    assert!(r.probe.popup_theme_items.is_empty(), "the list closes");
    assert!(r.probe.theme_box.is_some(), "the panel stays open");
    assert_eq!(
        theme::get(&d.ctx).id,
        theme::THEMES[0].id,
        "the pages keep theirs"
    );
    let popup_box = r.probe.popup_theme_box.unwrap();
    d.click(&mut r, popup_box.center());
    assert!(r.probe.popup_same.is_some_and(|s| s == same));
    d.click(&mut r, same.center());
    assert_eq!(r.settings.popup_theme, popup::SAME_AS_PAGES);
    assert!(d.key(&mut r, Key::Escape));
    assert!(r.probe.theme_box.is_none(), "Esc closes the panel");

    // Dictionaries: a rich one, a collapsed one and a frequency list.
    let root = temp.path().join("dictionaries");
    let rich = r#"[{"type":"structured-content","content":[{"tag":"ul","content":[{"tag":"li","content":"mountains and rivers; landscape"},{"tag":"li","content":["see ",{"tag":"a","href":"?query=日月","content":"日月"}]},{"tag":"li","content":"as in 日月山川"}]},{"tag":"details","content":[{"tag":"summary","content":"Examples"},"山川秀丽"]}]}]"#;
    let terms = format!(
        r#"[["山川","shānchuān","n","",9,{rich},1,"★"],["山川","sānchuān","","",8,["mountain and river (literal)"],2,""],["山川","shānzhuān","","",7,["a place name"],3,""],["山川","shānchuan","","",6,["(dialect) hills"],4,""],["日月","rìyuè","n","",5,["sun and moon","time"],5,""]]"#
    );
    import_test_dict(
        temp.path(),
        &root,
        "0000000000000001",
        &[
            (
                "index.json",
                r#"{"title":"Test","revision":"1","format":3}"#,
            ),
            ("term_bank_1.json", &terms),
        ],
    );
    import_test_dict(
        temp.path(),
        &root,
        "0000000000000002",
        &[
            (
                "index.json",
                r#"{"title":"Collapsed","revision":"1","format":3}"#,
            ),
            (
                "term_bank_1.json",
                r#"[["山川","shānchuān","","",1,["hidden until opened"],1,""]]"#,
            ),
        ],
    );
    import_test_dict(
        temp.path(),
        &root,
        "0000000000000003",
        &[
            (
                "index.json",
                r#"{"title":"Freq","revision":"1","format":3}"#,
            ),
            (
                "term_meta_bank_1.json",
                r#"[["山川","freq",1234],["日月","freq",567]]"#,
            ),
        ],
    );
    hanstat::dict::manage::edit_manifest(&root, |m| {
        for e in &mut m.dicts {
            e.collapsed = e.id == "0000000000000002";
        }
    })
    .unwrap();
    r.dict = popup::Popup::with_root(&d.ctx, None, root);
    // Anki buttons, against nothing (a closed port): every check fails.
    r.settings.anki = anki::CardSettings {
        url: "http://127.0.0.1:9".into(),
        deck: "Test".into(),
        model: "Basic".into(),
        fields: vec![("Front".into(), "{expression}".into())],
        ..Default::default()
    };
    r.dict.sync_cards(&r.settings.anki);

    let word = displayed_char(&r, 0).center();
    let shift = egui::Modifiers::SHIFT;
    let since = Instant::now();
    while r
        .dict
        .levels
        .first()
        .is_none_or(|l| l.result.groups.len() < 4)
    {
        d.frame_with(&mut r, vec![egui::Event::PointerMoved(word)], shift);
        assert!(
            since.elapsed().as_secs() < 10,
            "the popup never opened: {} dictionaries, errors {:?}, {:?}",
            r.dict.dicts.len(),
            r.dict.errors,
            r.dict.levels.first().map(|l| l
                .result
                .groups
                .iter()
                .map(|g| (g.headword.clone(), g.reading.clone()))
                .collect::<Vec<_>>())
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let groups = r.dict.levels[0].result.groups.clone();
    while groups
        .iter()
        .any(|g| matches!(r.dict.cards.status(g), None | Some(anki::Status::Checking)))
    {
        d.frame(&mut r, vec![]);
        assert!(since.elapsed().as_secs() < 20, "Anki checks never answered");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for (g, s) in groups.iter().zip([
        anki::Status::CanAdd,
        anki::Status::Added(1),
        anki::Status::Duplicate,
    ]) {
        r.dict.cards.set_status(g, s);
    }
    // A child popup: the cross-reference looked up with Shift.
    let pr = r.dict.levels[0].rect.expect("drawn");
    'find: for y in (0..)
        .map(|k| pr.top() + 20.0 + k as f32 * 6.0)
        .take_while(|y| *y < pr.bottom())
    {
        for x in (0..)
            .map(|k| pr.left() + 16.0 + k as f32 * 8.0)
            .take_while(|x| *x < pr.right() - 16.0)
        {
            d.frame_with(&mut r, vec![egui::Event::PointerMoved(pos2(x, y))], shift);
            if r.dict.levels.len() == 2 {
                break 'find;
            }
        }
    }
    assert_eq!(r.dict.levels.len(), 2, "a child popup");
    // The Anki buttons stay clear of the scroll bar, even hovered (at its
    // widest), in both popups.
    let scroll = d.ctx.style_of(d.ctx.theme()).spacing.scroll;
    for (level, l) in r.dict.levels.iter().enumerate() {
        let (buttons, entries) = &l.drawn;
        assert!(!buttons.is_empty(), "popup {level} has Anki buttons");
        let bar_left = entries.right() - scroll.bar_width;
        for b in buttons {
            assert!(
                b.right() + scroll.bar_inner_margin <= bar_left + 0.5,
                "popup {level}: a button at {b:?} reaches the scroll bar (from x {bar_left})"
            );
        }
    }
    assert_eq!(r.dict.levels[1].result.text, "日月");
    let held = d.ctx.input(|i| i.pointer.hover_pos()).unwrap();
    let mut themes = vec![
        popup::SAME_AS_PAGES.to_owned(),
        "catppuccin-mocha".to_owned(),
        "gruvbox-light".to_owned(),
        "tokyo-night".to_owned(),
    ];
    themes.extend(
        env("READER_POPUP_THEMES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    );
    for id in &themes {
        // Changed with the popups open: they take it in the next frame.
        r.settings.popup_theme = id.clone();
        for _ in 0..3 {
            d.frame_with(&mut r, vec![egui::Event::PointerMoved(held)], shift);
        }
        assert_eq!(r.dict.levels.len(), 2, "{id}: both popups stay");
        let t = popup::popup_theme(&d.ctx, id);
        if id == popup::SAME_AS_PAGES {
            assert_eq!(t.id, theme::THEMES[0].id);
        } else {
            assert_eq!(t.id, *id);
        }
        let rects: Vec<Rect> = r.dict.levels.iter().map(|l| l.rect.unwrap()).collect();
        let shot = d.shot(&format!("popup-theme-{id}"));
        for (level, pr) in rects.iter().enumerate() {
            // In the frame's margin, left of the entries.
            let c = shot.pixel(pos2(pr.left() + 5.0, pr.bottom() - 30.0));
            let diff = |a: u8, b: u8| a.abs_diff(b);
            assert!(
                diff(c.r(), t.page.r()) <= 2
                    && diff(c.g(), t.page.g()) <= 2
                    && diff(c.b(), t.page.b()) <= 2,
                "{id}: popup {level} is {c:?}, the theme's page {:?}",
                t.page
            );
        }
    }
    // The parent alone, scrolled to its last entries' Anki buttons (added,
    // already in Anki, failed).
    assert!(d.key(&mut r, Key::Escape));
    assert_eq!(r.dict.levels.len(), 1, "Esc closes the child");
    r.dict.scroll_lines = 20;
    d.idle(&mut r, 3);
    assert!(r.dict.levels[0].scroll_y > 0.0, "scrolled");
    for id in &themes {
        r.settings.popup_theme = id.clone();
        d.idle(&mut r, 3);
        d.snap(&format!("popup-theme-{id}-end"));
    }
    r.close(&d.ctx);
}

/// The popup theme is saved; settings from before the choice keep the
/// popup's old look.
#[test]
fn popup_theme_setting() {
    assert_eq!(Settings::default().popup_theme, popup::SAME_AS_PAGES);
    let old: Settings = serde_json::from_str(r#"{"theme":"paper"}"#).unwrap();
    assert_eq!(old.popup_theme, "catppuccin-mocha");
    assert_eq!(old.theme, "paper");
    let s = Settings {
        popup_theme: "nord".into(),
        ..Settings::default()
    };
    let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.popup_theme, "nord");
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
        local: None,
    };
    for rg in tts::sentences(&c.chars) {
        let t: String = c.chars[rg.start as usize..rg.end as usize].iter().collect();
        assert!(
            tts::clip_path(r.tts.dir(), speech.key(&t), tts::MP3).exists(),
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

/// Reading aloud with the local voice (Qwen3-TTS on the GPU), the sound
/// device open but silent. Run with `HANSTAT_MODEL_DIR` at a folder holding
/// the model and server, `LOCALAPPDATA` at a
/// scratch folder and `HANSTAT_NO_UPDATE=1`; `READER_SPINE` picks the
/// chapter made ahead of time.
///
/// The panel and the "Generate locally" window are drawn; a preview is
/// made; the chapter is made in chunks, every sentence saved as Ogg Opus
/// with a plausible length (the cuts landed between sentences), and how
/// fast is reported; reading from the next chapter (nothing made) starts
/// within seconds and goes on without stalls; a click reads a saved
/// sentence at once.
#[test]
#[ignore]
fn read_aloud_local() {
    check_local_read_aloud(false);
}

/// As for CustomVoice, but make and keep a real VoiceDesign sample, then
/// switch to Base and read with the clone. Install the models using the
/// download button first. `READER_OUT` receives the settled Design window
/// and the full chapter to listen to.
#[test]
#[ignore]
fn read_aloud_designed() {
    assert_eq!(env("HANSTAT_NO_UPDATE").as_deref(), Some("1"));
    assert!(
        env("LOCALAPPDATA").is_some(),
        "set LOCALAPPDATA to a scratch folder"
    );
    assert!(env("HANSTAT_MODEL_DIR").is_some(), "set HANSTAT_MODEL_DIR");
    assert!(
        env("READER_OUT").is_some(),
        "set READER_OUT for the PNG and chapter audio"
    );
    check_local_read_aloud(true);
}

fn check_local_read_aloud(designed: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    use tts::PlayState;
    let book = env("READER_EPUB").expect("set READER_EPUB");
    tts::audio::MUTE.store(true, Relaxed);
    let downloaded = if designed {
        tts::local::design_downloaded()
    } else {
        tts::local::downloaded()
    };
    assert!(
        downloaded,
        "the models and server must be in HANSTAT_MODEL_DIR"
    );
    let mut d = Driver::new();
    let mut r = d.open(&book);
    r.settings.tts.source = tts::Source::Local;
    r.spine = env("READER_SPINE")
        .and_then(|v| v.parse().ok())
        .unwrap_or(if designed { 5 } else { 2 });
    r.target = Some(Target::Offset(0));
    d.ready(&mut r);
    let spine = r.spine;
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

    let prefix = if designed {
        "tts-designed"
    } else {
        "tts-local"
    };
    if designed {
        let description = "三十岁左右的男声，低沉温和，像深夜电台主持人";
        let text = "天渐渐亮了，河面上还飘着一层薄雾。她沿着石板路慢慢走到桥边，停下脚步，听见远处传来几声清脆的鸟鸣。";
        let seed = 86;
        let start = Instant::now();
        let sample = tts::local::design_sample(description, text, seed).unwrap();
        let sample_secs = sample.pcm.len() as f64 / tts::opus::RATE as f64;
        eprintln!(
            "VoiceDesign: {sample_secs:.1} s sample in {:.2} s (including model load)",
            start.elapsed().as_secs_f64()
        );
        assert!(sample_secs > 1.0);
        let id = tts::local::VoiceStore::default()
            .save_voice("Late-night radio", description, text, seed, &sample.wav)
            .unwrap();
        r.settings.tts.local.designed = Some(id);
        r.tts.open_design_window();
        d.idle(&mut r, 30);
        d.snap("tts-design-window");
        r.tts.close_design_window();
        d.idle(&mut r, 2);

        let start = Instant::now();
        tts::local::server(
            tts::local::Mode::Base,
            r.settings.tts.local.parallel,
            |_| (),
        )
        .unwrap();
        eprintln!("Base loaded in {:.2} s", start.elapsed().as_secs_f64());
    }

    // The panel and the generation window show the voice about to read.
    r.tts.panel_open = true;
    d.idle(&mut r, 30);
    d.snap(&format!("{prefix}-panel"));
    r.tts.panel_open = false;
    r.tts.open_local_window(spine);
    d.idle(&mut r, 30);
    d.snap(&format!("{prefix}-window"));
    r.tts.close_local_window();
    d.idle(&mut r, 2);

    // Base's first preview registers the kept sample.
    let s = r.settings.tts.local.clone();
    let t = Instant::now();
    let pcm = tts::local::preview("这是一段试听。我们听听现在的设置。", &s).unwrap();
    eprintln!(
        "preview: {:.1} s of audio in {:.1} s ({})",
        pcm.len() as f64 / tts::opus::RATE as f64,
        t.elapsed().as_secs_f64(),
        if designed {
            "includes Base voice registration"
        } else {
            "includes model load"
        }
    );
    assert!(pcm.len() > 24_000);

    // The chapter, ahead of time.
    let tset = r.settings.tts.clone();
    let t = Instant::now();
    r.tts.pregenerate(&tset, vec![spine], "test chapter");
    let b = r.tts.batch().unwrap();
    wait(&mut d, &mut r, 1800, "made ahead", &|_| b.finished());
    let secs = t.elapsed().as_secs_f64();
    let (total, cached, done, failed) = (
        b.total.load(Relaxed),
        b.cached.load(Relaxed),
        b.done.load(Relaxed),
        b.failed.load(Relaxed),
    );
    assert_eq!(failed, 0, "{:?}", b.error.lock().unwrap());
    let c = r.chapter(spine).unwrap();
    let speech = tts::Speech {
        voice: String::new(),
        rate: 0,
        local: Some(s.clone()),
    };
    let sents = tts::sentences(&c.chars);
    let (mut audio, mut bytes, mut odd) = (0.0, 0u64, Vec::new());
    let mut chapter_pcm = Vec::new();
    for rg in &sents {
        let text: String = c.chars[rg.start as usize..rg.end as usize].iter().collect();
        let p = tts::clip_path(r.tts.dir(), speech.key(&text), tts::OPUS);
        let file = std::fs::read(&p).unwrap_or_else(|_| panic!("saved: {text}"));
        bytes += file.len() as u64;
        let (samples, rate) = tts::opus::decode(file).unwrap();
        assert_eq!(rate, tts::opus::RATE);
        let len = samples.len() as f64 / rate as f64;
        audio += len;
        if designed {
            chapter_pcm.extend(samples.iter().map(|&x| (x * 32_768.0) as i16));
        }
        // Expected about 3.8 characters a second; a cut in the wrong pause
        // makes one clip far too long and a neighbour far too short.
        let want = tts::local::seconds(rg.len());
        if len > want * 2.5 + 1.5 || len < want * 0.35 - 0.3 {
            odd.push(format!("{len:.1} s for {want:.1} s: {text}"));
        }
    }
    eprintln!(
        "chapter {spine}: {total} sentences ({cached} saved before, {done} made) in {secs:.1} s: \
         {audio:.0} s of audio, {:.1}x real time, {:.0} kbit/s",
        audio / secs,
        bytes as f64 * 8.0 / audio / 1000.0
    );
    eprintln!("clips far from their expected length: {}", odd.len());
    for o in &odd {
        eprintln!("  {o}");
    }
    assert!(
        odd.len() * 20 <= sents.len(),
        "more than 1 in 20 clips implausibly long or short"
    );
    if designed && let Some(dir) = &d.out_dir {
        write_pcm_wav(
            &dir.join("tts-designed-chapter.wav"),
            &chapter_pcm,
            tts::opus::RATE,
        );
    }

    // Reading from where nothing is made: the next chapter with text.
    let next = (spine + 1..r.epub.spine.len())
        .find(|&i| {
            r.chapter(i)
                .is_some_and(|c| tts::sentences(&c.chars).len() > 20)
        })
        .expect("a later chapter");
    let t = Instant::now();
    r.tts.play_from(next, 0, &r.settings.tts);
    wait(&mut d, &mut r, 120, "starts", &|r| {
        r.tts.state() == PlayState::Playing
    });
    let cold = t.elapsed().as_secs_f64();
    eprintln!("cold start: first uncached audio after {cold:.2} s");
    if designed {
        assert!(cold < 5.0, "cold start took {cold:.2} s with Base loaded");
    }
    d.snap(&format!("{prefix}-reading"));
    // A minute of reading: how long it waits for audio after starting.
    let t = Instant::now();
    let mut stalled = 0.0;
    let mut last = Instant::now();
    let first = r.tts.at();
    while t.elapsed().as_secs() < 60 {
        d.frame(&mut r, vec![]);
        assert!(
            r.tts.error().is_none(),
            "playback failed: {:?}",
            r.tts.error()
        );
        let dt = last.elapsed().as_secs_f64();
        last = Instant::now();
        if r.tts.state() == PlayState::Loading {
            stalled += dt;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    eprintln!(
        "a minute of reading: {stalled:.1} s waiting for audio; from {:?} to {:?}",
        first.clone().map(|(s, rg)| (s, rg.start)),
        r.tts.at().map(|(s, rg)| (s, rg.start))
    );
    assert_ne!(r.tts.at(), first, "the reading went on");
    let max_stall = if designed { 0.1 } else { 10.0 };
    assert!(stalled < max_stall, "stalled {stalled:.2} s");

    // A saved sentence plays at once.
    let mid = sents.len() / 2;
    let t = Instant::now();
    r.tts.play_from(spine, sents[mid].start, &r.settings.tts);
    // Still playing the other chapter for a frame or two: wait for this one.
    let want = Some((spine, sents[mid].clone()));
    wait(&mut d, &mut r, 10, "a saved sentence", &|r| {
        r.tts.state() == PlayState::Playing && r.tts.at() == want
    });
    eprintln!(
        "saved sentence: playing after {:.2} s",
        t.elapsed().as_secs_f32()
    );
    assert_eq!(r.tts.at(), Some((spine, sents[mid].clone())));
    r.tts.stop();
    d.idle(&mut r, 20);
    assert!(!r.tts.active());
    r.close(&d.ctx);
}

/// A playable chapter in the reader's sentence order, with the same
/// samples the reader decodes from its saved clips.
fn write_pcm_wav(path: &Path, pcm: &[i16], rate: u32) {
    let len = u32::try_from(pcm.len() * 2).unwrap();
    let mut wav = Vec::with_capacity(44 + len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0"); // PCM mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data"); // 16-bit samples
    wav.extend_from_slice(&len.to_le_bytes());
    wav.extend(pcm.iter().flat_map(|x| x.to_le_bytes()));
    std::fs::write(path, wav).unwrap();
}

/// Takes of the goal-met sound ([`super::goal_sound::CHEER_TEXT`]) read by a
/// designed local voice, to choose the one shipped as
/// `assets/goal-met.opus`. `CHEER_VOICE`: the designed voice's id (its
/// folder in `LOCALAPPDATA`'s hanstat/voices); `CHEER_DIR`: where each take
/// goes, as .wav (to listen to) and .opus; `CHEER_TAKES`: how many (6). The
/// sampling is the reader's for Citrus on 2026-11-25 (temperature 0.7,
/// sub-temperature 0.8); each take another seed. `HANSTAT_MODEL_DIR` with
/// the design/base model.
#[test]
#[ignore]
fn goal_cheer_takes() {
    let dir = std::path::PathBuf::from(env("CHEER_DIR").expect("set CHEER_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = tts::local::LocalSettings {
        designed: Some(env("CHEER_VOICE").expect("set CHEER_VOICE")),
        temperature: 0.7,
        sub_temperature: 0.8,
        ..Default::default()
    };
    let n: u32 = env("CHEER_TAKES").and_then(|v| v.parse().ok()).unwrap_or(6);
    for take in 0..n {
        s.seed = take;
        let pcm = tts::local::preview(super::goal_sound::CHEER_TEXT, &s).unwrap();
        let secs = pcm.len() as f64 / tts::opus::RATE as f64;
        write_pcm_wav(&dir.join(format!("take-{take}.wav")), &pcm, tts::opus::RATE);
        let ogg = tts::opus::encode(&pcm).unwrap();
        std::fs::write(dir.join(format!("take-{take}.opus")), &ogg).unwrap();
        eprintln!("take {take}: {secs:.2} s, {} KB Opus", ogg.len() / 1024);
    }
}

/// How well local-voice chunks are cut into sentences, on real audio: the
/// chunks of `CUT_SPINES` (comma-separated, at most `CUT_CHUNKS` each, 12 by
/// default) are made once with the model in `HANSTAT_MODEL_DIR` and kept in
/// `CUT_DIR`, then cut by [`tts::local::cuts`] and by the first build's
/// [`tts::local::cuts_by_pauses`]. Every sentence clip is written there as
/// 16 kHz WAV, with `manifest.tsv` (algorithm, file, text) for a speech
/// recogniser to check that each clip says its own sentence. `CUT_SPEAKER`
/// and `CUT_INSTRUCT` set the voice.
#[test]
#[ignore]
fn local_cut_eval() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let dir = std::path::PathBuf::from(env("CUT_DIR").expect("set CUT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let max: usize = env("CUT_CHUNKS").and_then(|v| v.parse().ok()).unwrap_or(12);
    let mut s = tts::local::LocalSettings::default();
    if let Some(v) = env("CUT_SPEAKER") {
        s.speaker = v;
    }
    if let Some(v) = env("CUT_INSTRUCT") {
        s.instruct = v;
    }
    let d = Driver::new();
    let r = d.open(&book);
    let spines: Vec<usize> = env("CUT_SPINES")
        .expect("set CUT_SPINES")
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    // Every chunk to cut: (name, texts, paragraphs).
    let mut jobs = Vec::new();
    for &spine in &spines {
        let t = Instant::now();
        let ch = loop {
            if let Some(c) = r.chapter(spine) {
                break c;
            }
            assert!(t.elapsed().as_secs() < 60, "chapter {spine} not built");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let sents = tts::sentences(&ch.chars);
        let paras = tts::local::paragraphs(&ch.chars, &sents);
        let lens: Vec<usize> = sents.iter().map(|r| r.len()).collect();
        for (i, g) in tts::local::chunks(&lens, &paras, 0, |_| s.chunk_chars, max)
            .into_iter()
            .enumerate()
        {
            let texts: Vec<String> = sents[g.clone()]
                .iter()
                .map(|r| ch.chars[r.start as usize..r.end as usize].iter().collect())
                .collect();
            if texts.len() > 1 {
                jobs.push((format!("{spine}-{i:02}"), texts, paras[g].to_vec()));
            }
        }
    }
    // Made once, four at a time.
    let missing: Vec<usize> = (0..jobs.len())
        .filter(|&i| !dir.join(format!("chunk-{}.pcm", jobs[i].0)).exists())
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&i) = missing.get(k) else { break };
                    let (name, texts, paras) = &jobs[i];
                    let mut text = String::new();
                    for (j, t) in texts.iter().enumerate() {
                        if j > 0 && paras[j] != paras[j - 1] {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    let seed = hanstat::dict::xxh64::xxh64(text.as_bytes(), s.seed as u64);
                    let pcm = tts::local::server(tts::local::Mode::Custom, 4, |server| {
                        server.synth(&text, &s, seed)
                    })
                    .unwrap()
                    .unwrap()
                    .0;
                    let bytes: Vec<u8> = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
                    std::fs::write(dir.join(format!("chunk-{name}.pcm")), bytes).unwrap();
                    eprintln!("made chunk {name}");
                }
            });
        }
    });
    tts::local::unload();
    let wav16 = |path: &std::path::Path, pcm: &[i16]| {
        let f: Vec<f32> = pcm.iter().map(|&x| x as f32 / 32768.0).collect();
        let r = tts::audio::resample(&f, 24_000, 16_000);
        let mut w = Vec::with_capacity(44 + r.len() * 2);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + r.len() as u32 * 2).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            16_000u32.to_le_bytes(),
            32_000u32.to_le_bytes(),
            [2, 0, 16, 0],
        ] {
            w.extend_from_slice(&v);
        }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(r.len() as u32 * 2).to_le_bytes());
        for x in r {
            w.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, w).unwrap();
    };
    let mut manifest = String::new();
    for (name, texts, paras) in &jobs {
        let bytes = std::fs::read(dir.join(format!("chunk-{name}.pcm"))).unwrap();
        let pcm: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        let lens: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
        for (alg, cut) in [
            ("new", tts::local::cuts(&pcm, texts, paras)),
            ("old", tts::local::cuts_by_pauses(&pcm, &lens)),
        ] {
            let mut a = 0;
            for (k, &c) in cut.iter().chain(std::iter::once(&pcm.len())).enumerate() {
                let file = format!("{alg}-{name}-{k:02}.wav");
                wav16(&dir.join(&file), &pcm[a..c.max(a)]);
                manifest += &format!("{alg}\t{file}\t{}\n", texts[k]);
                a = c.max(a);
            }
        }
    }
    std::fs::write(dir.join("manifest.tsv"), manifest).unwrap();
    eprintln!("{} chunks cut into {}", jobs.len(), dir.display());
    r.close(&d.ctx);
}

/// Where local-voice chunks meet, with and without a lead-in: chunks
/// `1..=JOIN_N` (6 by default) of chapter `JOIN_SPINE` are made again with
/// the end of the sentence before read first and cut away
/// ([`tts::local::make_chunk`]), and each join is written as the last 6 s
/// of the chunk before and the first 8 s of the chunk after, to
/// `join-old.wav` (chunks as `local_cut_eval` made them, in `CUT_DIR`) and
/// `join-new.wav`, with the silence at each join reported. The new chunks'
/// first clips go to `lead-*.wav` with `lead.tsv`, to check by speech
/// recognition that the lead-in is gone.
#[test]
#[ignore]
fn local_join_eval() {
    let book = env("READER_EPUB").expect("set READER_EPUB");
    let dir = std::path::PathBuf::from(env("CUT_DIR").expect("set CUT_DIR"));
    let spine: usize = env("JOIN_SPINE").expect("set JOIN_SPINE").parse().unwrap();
    let n: usize = env("JOIN_N").and_then(|v| v.parse().ok()).unwrap_or(6);
    let s = tts::local::LocalSettings::default();
    let d = Driver::new();
    let r = d.open(&book);
    let t = Instant::now();
    let ch = loop {
        if let Some(c) = r.chapter(spine) {
            break c;
        }
        assert!(t.elapsed().as_secs() < 60);
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let sents = tts::sentences(&ch.chars);
    let paras = tts::local::paragraphs(&ch.chars, &sents);
    let lens: Vec<usize> = sents.iter().map(|r| r.len()).collect();
    let text = |i: usize| -> String {
        ch.chars[sents[i].start as usize..sents[i].end as usize]
            .iter()
            .collect()
    };
    let groups = tts::local::chunks(&lens, &paras, 0, |_| s.chunk_chars, n + 1);
    let old_clips = |i: usize| -> Vec<Vec<i16>> {
        let g = &groups[i];
        let bytes = std::fs::read(dir.join(format!("chunk-{spine}-{i:02}.pcm")))
            .expect("run local_cut_eval for this chapter first");
        let pcm: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        let texts: Vec<String> = g.clone().map(text).collect();
        let cut = tts::local::cuts(&pcm, &texts, &paras[g.clone()]);
        let mut out = Vec::new();
        let mut a = 0;
        for &c in cut.iter().chain(std::iter::once(&pcm.len())) {
            out.push(pcm[a..c.max(a)].to_vec());
            a = c.max(a);
        }
        out
    };
    // Silence at the join: the quiet run around the meeting point.
    let quiet = |x: &[i16]| {
        let rms = |w: &[i16]| {
            (w.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
        };
        x.chunks(240).map(|w| rms(w) < 0.01).collect::<Vec<bool>>()
    };
    let gap = |before: &[i16], after: &[i16]| {
        let (b, a) = (quiet(before), quiet(after));
        let tail = b.iter().rev().take_while(|q| **q).count();
        let head = a.iter().take_while(|q| **q).count();
        (tail + head) as f64 / 100.0
    };
    let (mut old_wav, mut new_wav) = (Vec::new(), Vec::new());
    let mut lead_tsv = String::new();
    let next = std::sync::atomic::AtomicUsize::new(1);
    let made: Vec<std::sync::Mutex<Option<Vec<Vec<i16>>>>> =
        (0..=n).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i > n || i >= groups.len() {
                        break;
                    }
                    let g = &groups[i];
                    let texts: Vec<String> = g.clone().map(text).collect();
                    let lead = tts::local::Lead {
                        text: tts::local::lead_in(&text(g.start - 1)),
                        same_para: paras[g.start - 1] == paras[g.start],
                    };
                    let clips =
                        tts::local::make_chunk(&texts, &paras[g.clone()], Some(&lead), &s).unwrap();
                    assert_eq!(clips.len(), texts.len());
                    *made[i].lock().unwrap() = Some(clips);
                }
            });
        }
    });
    tts::local::unload();
    let silence = vec![0i16; 36_000];
    let (mut gaps_old, mut gaps_new) = (Vec::new(), Vec::new());
    for i in 1..=n.min(groups.len() - 1) {
        let prev = old_clips(i - 1).concat();
        let tail = &prev[prev.len().saturating_sub(6 * 24_000)..];
        let old = old_clips(i);
        let new = made[i].lock().unwrap().take().unwrap();
        gaps_old.push(gap(tail, &old[0]));
        gaps_new.push(gap(tail, &new[0]));
        for (wav, clips) in [(&mut old_wav, &old), (&mut new_wav, &new)] {
            let head = clips.concat();
            wav.extend_from_slice(tail);
            wav.extend_from_slice(&head[..head.len().min(8 * 24_000)]);
            wav.extend_from_slice(&silence);
        }
        let file = format!("lead-{spine}-{i:02}.pcm");
        let bytes: Vec<u8> = new[0].iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(dir.join(&file), bytes).unwrap();
        lead_tsv += &format!(
            "{file}	{}	{}
",
            text(groups[i].start - 1),
            text(groups[i].start)
        );
    }
    let write = |name: &str, pcm: &[i16]| {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + pcm.len() as u32 * 2).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            24_000u32.to_le_bytes(),
            48_000u32.to_le_bytes(),
            [2, 0, 16, 0],
        ] {
            w.extend_from_slice(&v);
        }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(pcm.len() as u32 * 2).to_le_bytes());
        for x in pcm {
            w.extend_from_slice(&x.to_le_bytes());
        }
        std::fs::write(dir.join(name), w).unwrap();
    };
    write("join-old.wav", &old_wav);
    write("join-new.wav", &new_wav);
    std::fs::write(dir.join("lead.tsv"), lead_tsv).unwrap();
    eprintln!("silence at joins, old: {gaps_old:.2?}");
    eprintln!("silence at joins, new: {gaps_new:.2?}");
    r.close(&d.ctx);
}

fn stats_book(d: &mut Driver, dir: &Path) -> Reader {
    let path = dir.join("stats.epub");
    let ch1 = format!(
        r#"<html><meta charset="utf-8"><body><p>{}。</p><p style="break-before:page">{}。</p><p style="break-before:page">{}。</p></body></html>"#,
        "甲乙丙丁".repeat(10),
        "山川日月".repeat(5),
        "天地".repeat(15)
    );
    let ch2 = format!(
        r#"<html><meta charset="utf-8"><body><p>{} The end.</p></body></html>"#,
        "终章文字".repeat(3)
    );
    write_zip(
        &path,
        &[
            (
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
            ),
            (
                "book.opf",
                r#"<package><metadata><title>Stats test</title></metadata><manifest><item id="a" href="one.xhtml" media-type="application/xhtml+xml"/><item id="b" href="two.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#,
            ),
            ("one.xhtml", &ch1),
            ("two.xhtml", &ch2),
        ],
    );
    let mut r = d.open(path.to_str().unwrap());
    r.settings = Settings {
        spread: false,
        ..Settings::default()
    };
    r.go_chapter(0, Target::Offset(0));
    d.ready(&mut r);
    r
}

/// The reading tracker in the real reader: page turns counted by their
/// Chinese characters, back turns, P, a turn while paused, idle time, the
/// end of the book, the line under the page and the Stats panel. With a
/// clock set by hand; the stats stay in memory (`persist` is off).
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn reading_stats_tracker() {
    use crate::reading_stats as rs;
    use tracker::{Pause, State};
    let temp = tempfile::tempdir().unwrap();
    let t0 = 1_790_000_000.0;
    rs::set_test_clock(Some(t0));
    rs::reset_for_test(rs::ReadingStats::default());
    let mut d = Driver::new();
    let mut r = stats_book(&mut d, temp.path());
    let chars = |r: &Reader| r.tracker.totals.read_chars;
    let base = r.tracker.base().expect("place seen");
    assert_eq!(
        (base.spine, base.page, base.han, base.chapter_han),
        (0, 0, 0, 90)
    );
    assert_eq!(r.chapter(0).unwrap().han_len, 90, "tree and layout agree");
    assert_eq!(r.tracker.state(), State::Running);

    rs::set_test_clock(Some(t0 + 30.0));
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.page, 1);
    assert_eq!(chars(&r), 40);
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(chars(&r), 60);
    d.key(&mut r, Key::ArrowLeft);
    assert_eq!(chars(&r), 40, "back takes them off");
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(chars(&r), 60);
    d.snap("stats-line");
    let line = r.stats_line(rs::now()).expect("line shown");
    assert!(line.left.contains("60 chars"), "{}", line.left);
    assert!(line.right.is_some(), "time left shown");

    // Paused by hand: a turn into the next chapter only moves the place.
    d.key(&mut r, Key::P);
    assert_eq!(r.tracker.state(), State::Paused(Pause::Manual));
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.spine, 1);
    assert_eq!(chars(&r), 60);
    let line = r.stats_line(rs::now()).unwrap();
    assert!(line.left.starts_with("Paused"), "{}", line.left);
    d.key(&mut r, Key::P);
    assert_eq!(r.tracker.state(), State::Running);
    d.key(&mut r, Key::ArrowLeft);
    assert_eq!((r.spine, r.page), (0, 2));
    assert_eq!(chars(&r), 30, "back over the last page of chapter one");
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(chars(&r), 60);

    // Ten minutes with no input: the tracker pauses, from the last input.
    let before = r.tracker.totals.read_secs + r.tracker.open_secs(rs::now()).0;
    rs::set_test_clock(Some(t0 + 630.0));
    d.idle(&mut r, 2);
    assert_eq!(r.tracker.state(), State::Paused(Pause::Idle));
    assert_eq!(r.tracker.totals.read_secs, before);
    d.snap("stats-idle");

    // The end of the book: the last page counts, once.
    rs::set_test_clock(Some(t0 + 640.0));
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.tracker.state(), State::Running);
    assert_eq!(chars(&r), 72);
    assert!(r.take_finished());
    d.key(&mut r, Key::ArrowRight);
    assert!(!r.take_finished());
    assert_eq!(chars(&r), 72);
    let book = rs::with(|s| s.books["ui-test"].clone());
    assert!(book.finished.is_some());
    assert_eq!(book.han_total, 102);
    assert_eq!(book.totals().read_chars, 72);
    assert_eq!(book.sessions.len(), 1);

    // The Stats panel: timing waits while it is open.
    r.stats_open = true;
    d.idle(&mut r, 5);
    assert_eq!(r.tracker.state(), State::Paused(Pause::Menu));
    d.snap("stats-panel");
    r.stats_open = false;
    d.idle(&mut r, 2);
    assert_eq!(r.tracker.state(), State::Running);

    // What the stats add to a frame, against a whole frame.
    let n = 2000;
    let t = Instant::now();
    for _ in 0..n {
        r.stats_frame(&d.ctx, false);
        let l = r.layouts[&r.spine].clone();
        r.stats_place(&l);
        std::hint::black_box(r.stats_line(rs::now()));
    }
    let stats = t.elapsed() / n;
    let t = Instant::now();
    d.idle(&mut r, 200);
    let frame = t.elapsed() / 200;
    eprintln!("stats per frame {stats:?}, whole frame {frame:?}");
    // A frame here is layout and painting without a GPU, so tens of µs.
    assert!(stats.as_micros() < 20, "{stats:?} of {frame:?}");
    rs::set_test_clock(None);
}

/// Every text in the last frame's shapes, tooltips included.
fn shown_text(d: &Driver) -> Vec<String> {
    fn walk(s: &egui::Shape, out: &mut Vec<String>) {
        match s {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &d.last.as_ref().expect("a frame ran").shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// Where the last frame shows `text` (a whole label): its centre.
fn text_center(d: &Driver, text: &str) -> Option<Pos2> {
    fn walk(s: &egui::Shape, text: &str) -> Option<Pos2> {
        match s {
            egui::Shape::Text(t) if t.galley.text() == text => {
                Some(Rect::from_min_size(t.pos, t.galley.size()).center())
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| walk(s, text)),
            _ => None,
        }
    }
    d.last
        .as_ref()?
        .shapes
        .iter()
        .find_map(|c| walk(&c.shape, text))
}

/// The goal-met sound's settings in the Stats panel: the built-in sound
/// first (no message to type), then a voice of the reader's chosen: its
/// message, the count against the limit, Make and Play, and that the
/// built-in sound plays until it is made.
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn goal_sound_settings() {
    use crate::reading_stats as rs;
    let temp = tempfile::tempdir().unwrap();
    rs::set_test_clock(Some(1_790_000_000.0));
    rs::reset_for_test(rs::ReadingStats::default());
    let mut d = Driver::new();
    let mut r = stats_book(&mut d, temp.path());
    r.stats.sound.set_voices(vec![
        ("designed:abc".into(), "Citrus".into()),
        (
            "speaker:vivian".into(),
            "Vivian — bright young woman".into(),
        ),
    ]);
    r.stats_open = true;
    d.idle(&mut r, 10);
    let header = text_center(&d, "Settings").expect("Settings header");
    d.click(&mut r, header);
    let texts = shown_text(&d);
    d.snap("goal-sound-built-in");
    for want in [
        "Play a sound when it's met",
        "Voice",
        "Built in (Citrus)",
        "▶ Play",
    ] {
        assert!(texts.iter().any(|t| t == want), "{want:?} not in {texts:?}");
    }
    assert!(
        !texts.iter().any(|t| t == "Make" || t == "Says"),
        "{texts:?}"
    );

    rs::update_meta(|s| s.settings.goal_sound_voice = Some("speaker:vivian".into()));
    d.idle(&mut r, 10);
    let texts = shown_text(&d);
    d.snap("goal-sound-own-voice");
    for want in [
        "Vivian — bright young woman",
        "Says",
        super::goal_sound::CHEER_TEXT,
        "19/20",
        "Make",
        "Not made yet: the built-in sound plays until it is.",
    ] {
        assert!(texts.iter().any(|t| t == want), "{want:?} not in {texts:?}");
    }
    let set = rs::with(|s| s.settings.clone());
    assert_eq!(
        set.goal_sound_text,
        super::goal_sound::CHEER_TEXT,
        "starts with the built-in words"
    );
    rs::set_test_clock(None);
}

/// Hover text on parts of the reader that have no label of their own: the
/// goal bar along the foot of the page, the page number and the Aa button.
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn reader_hover_texts() {
    use crate::reading_stats as rs;
    let temp = tempfile::tempdir().unwrap();
    rs::set_test_clock(Some(1_790_000_000.0));
    rs::reset_for_test(rs::ReadingStats {
        goals: vec![rs::Goal {
            from_day: 0,
            minutes: 30,
            chars: 1000,
        }],
        ..Default::default()
    });
    let mut d = Driver::new();
    let mut r = stats_book(&mut d, temp.path());
    let g = r.last_geom.expect("pages drawn");
    let page = g.pages[0];
    let aa = r.probe.aa.expect("Aa drawn");
    for (name, at, want) in [
        (
            "hover-goal",
            pos2(page.center().x - 100.0, page.bottom() - 1.0),
            "Today's reading goal: 0 of 30 min, 0 of 1,000 characters. The bar follows whichever is further behind.",
        ),
        (
            "hover-folio",
            pos2(page.center().x, page.bottom() - g.inset.y * 0.5),
            "Page 1 of 3 in this chapter",
        ),
        ("hover-aa", aa.center(), "Theme, fonts, text size"),
    ] {
        d.frame(&mut r, vec![egui::Event::PointerMoved(at)]);
        d.idle(&mut r, 60);
        let texts = shown_text(&d);
        d.snap(name);
        assert!(
            texts.iter().any(|t| t.contains(want)),
            "{name}: {want:?} not in {texts:?}"
        );
    }
    rs::set_test_clock(None);
}

/// The goal-met glow and sound: they start on the turn that meets a
/// characters goal; the glow rings the window's edge in many hues and
/// leaves the page alone, and is gone after 5 s. Neither comes back for
/// paging under the goal and on again or for opening a book with the goal
/// met; a minutes goal wakes the reader for them with no input; each can be
/// turned off on its own. (The sound is counted, not played, in tests.)
#[test]
#[ignore = "opens the reader with the system fonts and dictionary settings"]
fn goal_glow() {
    use crate::reading_stats as rs;
    let temp = tempfile::tempdir().unwrap();
    let t0 = 1_790_000_000.0;
    rs::set_test_clock(Some(t0));
    rs::reset_for_test(rs::ReadingStats {
        goals: vec![rs::Goal {
            from_day: 0,
            minutes: 0,
            chars: 50,
        }],
        // No minute ticks for the line, so the goal's wake-up shows alone.
        settings: rs::TrackerSettings {
            live_line: false,
            ..Default::default()
        },
        ..Default::default()
    });
    let cheers = || super::goal_sound::PLAYED.load(std::sync::atomic::Ordering::Relaxed);
    let c0 = cheers();
    let mut d = Driver::new();
    let mut r = stats_book(&mut d, temp.path());
    assert_eq!(r.stats.glow_since, None);
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.tracker.totals.read_chars, 40);
    assert_eq!(cheers(), c0, "40 of 50");
    assert_eq!(r.stats.glow_since, None, "40 of 50");
    rs::set_test_clock(Some(t0 + 20.0));
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.tracker.totals.read_chars, 60);
    assert_eq!(r.stats.glow_since, Some(t0 + 20.0), "met on this turn");
    assert_eq!(cheers(), c0 + 1, "the sound once");

    // Two seconds in: bright, many-hued edges; the middle as without it.
    rs::set_test_clock(Some(t0 + 22.0));
    assert_eq!(d.quiet(&mut r, 3), 0.0, "frames while it shows");
    let on = d.shot("goal-glow");
    let edges = [
        pos2(1.0, 400.0),
        pos2(600.0, 1.0),
        pos2(1198.0, 400.0),
        pos2(600.0, 798.0),
    ];
    let lit: Vec<Color32> = edges.iter().map(|p| on.pixel(*p)).collect();
    let chroma = |c: &Color32| {
        let [r, g, b, _] = c.to_array();
        r.max(g).max(b) - r.min(g).min(b)
    };
    for c in &lit {
        assert!(chroma(c) > 60, "{lit:?}");
    }
    assert!(lit.iter().any(|c| *c != lit[0]), "one hue: {lit:?}");
    let middle = pos2(600.0, 400.0);
    let mid_on = on.pixel(middle);

    rs::set_test_clock(Some(t0 + 25.1));
    let wait = d.quiet(&mut r, 3);
    assert_eq!(r.stats.glow_since, None, "over after 5 s");
    assert!(wait > 1.0, "no more frames for it: {wait}");
    let off = d.shot("goal-glow-after");
    for (p, c) in edges.iter().zip(&lit) {
        assert_ne!(off.pixel(*p), *c, "edge at {p:?} still lit");
    }
    assert_eq!(off.pixel(middle), mid_on, "the page untouched");

    // Under the goal and over it again: once a day is enough.
    d.key(&mut r, Key::ArrowLeft);
    assert_eq!(r.tracker.totals.read_chars, 40);
    d.key(&mut r, Key::ArrowRight);
    assert_eq!(r.tracker.totals.read_chars, 60);
    assert_eq!(r.stats.glow_since, None, "no second glow");
    assert_eq!(cheers(), c0 + 1, "no second sound");

    // Opened again with the goal met: no glow.
    r.end_stats();
    drop(r);
    let mut r = stats_book(&mut d, temp.path());
    d.idle(&mut r, 5);
    assert_eq!(r.stats.glow_since, None, "met before the book opened");
    assert_eq!(cheers(), c0 + 1, "met before the book opened");

    // A minutes goal set now (not met yet) is met by time alone: the
    // reader asks for a frame when it will be, and glows then.
    let today = rs::local_day(rs::now(), 4);
    rs::update_meta(|s| s.set_goal(today, 1, 0));
    let wait = d.quiet(&mut r, 3);
    assert_eq!(r.stats.glow_since, None);
    assert!((1.0..60.0).contains(&wait), "woken in {wait} s");
    rs::set_test_clock(Some(rs::now() + wait - 1.0));
    d.quiet(&mut r, 1);
    assert_eq!(r.stats.glow_since, None, "a second early");
    rs::set_test_clock(Some(rs::now() + 1.0));
    d.quiet(&mut r, 1);
    assert!(r.stats.glow_since.is_some(), "met by time");
    assert_eq!(cheers(), c0 + 2, "met by time");

    // The glow turned off: a goal met later only sounds.
    r.end_stats();
    drop(r);
    rs::update_meta(|s| {
        s.settings.goal_glow = false;
        s.set_goal(today, 0, 70);
    });
    let mut r = stats_book(&mut d, temp.path());
    d.key(&mut r, Key::Home);
    d.key(&mut r, Key::ArrowRight);
    let met = rs::with(|s| s.summary(today).goal_days.contains(&today));
    assert!(met, "goal met");
    assert_eq!(r.stats.glow_since, None, "glow turned off");
    assert_eq!(cheers(), c0 + 3, "the sound still on");

    // The sound turned off too: a goal met later does nothing.
    let read = rs::with(|s| s.summary(today).days[&today].read_chars);
    rs::update_meta(|s| {
        s.settings.goal_sound = false;
        s.set_goal(today, 0, read + 1);
    });
    d.idle(&mut r, 2);
    d.key(&mut r, Key::ArrowRight);
    let met = rs::with(|s| s.summary(today).goal_days.contains(&today));
    assert!(met, "goal met again");
    assert_eq!(r.stats.glow_since, None, "glow off");
    assert_eq!(cheers(), c0 + 3, "sound off");
    rs::set_test_clock(None);
}

/// Reader places against the analysis's sections, which the reading path
/// maps them onto ([`hanstat::profile::Positions::running_word_at`]): every
/// spine file with text is a section of the same path, holding the same
/// Chinese text. (A place is taken as a share of the reader's own chapter
/// text, which counts whitespace and markup text differently from the
/// section's paragraphs.) `READER_EPUBS`: books separated by `;`, or a
/// folder of them.
#[test]
#[ignore]
fn reader_places_map_to_sections() {
    let spec = env("READER_EPUBS").expect("set READER_EPUBS");
    let mut books: Vec<std::path::PathBuf> = Vec::new();
    for p in spec.split(';').map(std::path::PathBuf::from) {
        if p.is_dir() {
            let mut v: Vec<_> = std::fs::read_dir(&p)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
                })
                .collect();
            v.sort();
            books.extend(v);
        } else {
            books.push(p);
        }
    }
    let (mut chapters, mut missing, mut off) = (0, 0, 0);
    let mut worst = 0f64;
    for path in books {
        let epub = Arc::new(Epub::open(&path).unwrap());
        let book = hanstat::extract::load(&path, hanstat::extract::EncodingChoice::Auto).unwrap();
        let mut sheets = tree::SheetCache::default();
        let (mut reader_all, mut lib_all) = (0u64, 0u64);
        for item in epub.spine.iter() {
            let ch = tree::build(&epub, item, &mut sheets);
            if ch.han_len == 0 {
                continue;
            }
            chapters += 1;
            let Some(sec) = book.sections.iter().find(|s| {
                s.href
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(item))
            }) else {
                missing += 1;
                eprintln!("{}: {item} has no section", path.display());
                continue;
            };
            // The same text: its Chinese characters (whitespace and markup
            // text are counted differently, so all characters aren't).
            let han: u64 = sec
                .paragraphs
                .iter()
                .map(|p| p.chars().filter(|&c| hanstat::chars::is_han(c)).count() as u64)
                .sum();
            let all: u64 = sec
                .paragraphs
                .iter()
                .map(|p| p.chars().count() as u64)
                .sum();
            reader_all += ch.text_len as u64;
            lib_all += all;
            let r = ch.han_len as f64 / han.max(1) as f64;
            // Short chapters (a title page) can differ a lot in relative terms.
            if han >= 1_000 {
                worst = worst.max((r - 1.0).abs());
                if (r - 1.0).abs() > 0.02 {
                    off += 1;
                    eprintln!(
                        "{}: {item}: reader {} Chinese characters, section {han} ({:+.1}%)",
                        path.display(),
                        ch.han_len,
                        (r - 1.0) * 100.0
                    );
                }
            }
        }
        // Why a place is a share of the reader's own text, not of the section's.
        eprintln!(
            "{}: reader text {reader_all} characters, sections {lib_all} ({:+.2}%)",
            path.file_name().unwrap().to_string_lossy(),
            (reader_all as f64 - lib_all as f64) * 100.0 / lib_all.max(1) as f64
        );
    }
    eprintln!(
        "{chapters} chapters; {missing} without a section; {off} with other Chinese text (worst {:.1}%)",
        worst * 100.0
    );
    assert_eq!(
        missing, 0,
        "spine files with text that the analysis has no section for"
    );
    assert_eq!(
        off, 0,
        "chapters whose Chinese characters differ from the section's by over 2%"
    );
}

/// The reader's count of Chinese characters, chapter by chapter, against
/// its layout's and against the Library's analysis. `READER_EPUBS`: books
/// separated by `;`, or a folder of them.
#[test]
#[ignore]
fn reading_han_counts() {
    let spec = env("READER_EPUBS").expect("set READER_EPUBS");
    let mut books: Vec<std::path::PathBuf> = Vec::new();
    for p in spec.split(';').map(std::path::PathBuf::from) {
        if p.is_dir() {
            let mut v: Vec<_> = std::fs::read_dir(&p)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
                })
                .collect();
            v.sort();
            books.extend(v);
        } else {
            books.push(p);
        }
    }
    let ctx = egui::Context::default();
    let fams = loop {
        if let Some(f) = fonts::families() {
            break f;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let serif = fonts::default_choice(&fams, true, false);
    let sans = fonts::default_choice(&fams, false, false);
    let (defs, installed) = fonts::definitions(&fams, &serif, &sans, 1);
    ctx.set_fonts(defs);
    let _ = ctx.run_ui(Default::default(), |_| {});
    let settings = Settings::default();
    let g = geometry(
        &settings,
        Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 860.0)),
    );
    let th = &theme::THEMES[0];
    let params = Params {
        base: settings.size,
        content_w: g.content.x,
        page_h: g.content.y,
        line_spacing: settings.line_spacing,
        fonts: installed,
        serif_default: true,
        text: th.ink,
        link: th.accent,
        rule: th.muted,
        page: th.page,
        dark: th.dark,
    };
    let mut mismatched = 0;
    for path in books {
        let epub = Arc::new(Epub::open(&path).unwrap());
        let mut sheets = tree::SheetCache::default();
        let mut images = images::Images::new(epub.clone());
        let (mut tree_han, mut laid_han) = (0u64, 0u64);
        for (i, item) in epub.spine.iter().enumerate() {
            let ch = tree::build(&epub, item, &mut sheets);
            let mut l = None;
            let _ = ctx.run_ui(Default::default(), |ui| {
                l = Some(
                    ui.ctx()
                        .fonts_mut(|f| layout::layout(&ch, &params, f, &mut images)),
                );
            });
            let l = l.unwrap();
            // Every page's count leads up to the next one's, ending at the total.
            for p in 0..l.pages.len() {
                assert!(
                    l.page_han(p) <= l.page_han(p + 1),
                    "{}: chapter {i} page {p}",
                    path.display()
                );
            }
            if l.han_total != ch.han_len {
                mismatched += 1;
                eprintln!(
                    "{} chapter {i} {item}: tree {} laid out {}",
                    path.display(),
                    ch.han_len,
                    l.han_total
                );
            }
            tree_han += ch.han_len as u64;
            laid_han += l.han_total as u64;
        }
        let book = hanstat::extract::load(&path, hanstat::extract::EncodingChoice::Auto).unwrap();
        let lib: u64 = book
            .sections
            .iter()
            .filter(|s| s.default_included())
            .flat_map(|s| s.paragraphs.iter())
            .map(|p| p.chars().filter(|&c| hanstat::chars::is_han(c)).count() as u64)
            .sum();
        eprintln!(
            "{}: reader {tree_han} (laid out {laid_han}), Library {lib}, {:+.3}%",
            path.file_name().unwrap().to_string_lossy(),
            (tree_han as f64 - lib as f64) * 100.0 / lib.max(1) as f64
        );
    }
    assert_eq!(
        mismatched, 0,
        "chapters whose layout count differs from the tree's"
    );
}
