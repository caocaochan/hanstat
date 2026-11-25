use super::*;
use crate::reader::tts::engine::{Availability, Cmd, Engine, State};
use crate::reader::tts::{clip_path, opus};

fn png_cover(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([16, 32, 64]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn cover_art_fits_2048_pixels_without_upscaling() {
    let big = Cover::from_bytes(&png_cover(1024, 4096)).unwrap();
    let small = Cover::from_bytes(&png_cover(120, 80)).unwrap();
    assert_eq!((big.width, big.height), (512, 2048));
    assert_eq!((small.width, small.height), (120, 80));
    assert!(Cover::from_bytes(b"not an image").is_none());
}

fn chapter(spine: usize, title: &str, text: &str) -> Chapter {
    Chapter {
        spine,
        title: title.to_owned(),
        sentences: chapter_sentences(&text.chars().collect::<Vec<_>>()),
    }
}

fn progress(total: usize) -> Progress {
    Progress {
        stage: Mutex::new(Stage::Check),
        batch: Arc::new(Batch::default()),
        enc_done: AtomicUsize::new(0),
        total,
    }
}

#[test]
fn dialog_expands_when_chapters_load_and_the_save_button_is_visible() {
    let (send, loading) = std::sync::mpsc::channel();
    let mut dialog = Audiobook {
        source: Arc::new(SourceBook {
            path: PathBuf::from("Book.epub"),
            id: "book".into(),
            traditional: false,
        }),
        loading,
        form: None,
        job: None,
        confirm: None,
        complete: None,
        error: None,
    };
    let ctx = egui::Context::default();
    let mut frame = |height| {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, height),
                )),
                ..egui::RawInput::default()
            },
            |ui| assert!(matches!(dialog.show(ui.ctx()), Action::Keep)),
        );
        // Nothing draws the textures: egui asserts they are not dropped unseen.
        out.textures_delta.clear();
        out
    };
    frame(750.0);
    frame(750.0);
    let id = egui::Id::unique("export-audiobook");
    let loading_height = egui::AreaState::load(&ctx, id).unwrap().rect().height();
    assert!(send.send(Ok(chapter_list_form())).is_ok());
    for height in [750.0, 460.0, 750.0] {
        frame(height);
        let output = frame(height);
        let loaded_height = egui::AreaState::load(&ctx, id).unwrap().rect().height();
        assert!(
            loaded_height > loading_height + 180.0,
            "{loading_height} -> {loaded_height}"
        );
        let visible = |text: &str| {
            output.shapes.iter().any(|clipped| {
                if let egui::Shape::Text(shape) = &clipped.shape {
                    shape.galley.text() == text
                        && clipped
                            .clip_rect
                            .contains(shape.visual_bounding_rect().center())
                } else {
                    false
                }
            })
        };
        assert!(visible("M4B (AAC)"));
        assert!(visible("Export sentence subtitles (.srt)"));
        assert!(visible("Save audiobook…"));
    }
}

fn chapter_list_form() -> Form {
    let settings = TtsSettings::default();
    let speech = settings.speech(Source::Azure, false);
    Form {
        book: Arc::new(Book {
            title: "Book".into(),
            cover: None,
            dir: PathBuf::new(),
            chapters: (0..20)
                .map(|i| chapter(i, &format!("Chapter {}", i + 1), "甲。"))
                .collect(),
            known_profiles: Vec::new(),
            reader_profile: speech.key(""),
        }),
        selected: vec![true; 20],
        coverage: vec![Coverage {
            speech,
            sentences: vec![1; 20],
        }],
        settings,
        format: Format::default(),
        subtitles: true,
        azure_voices: Vec::new(),
        typed_key: String::new(),
        fetching: None,
        local_voices: Vec::new(),
        local_ready: [false; 2],
        download: None,
        provider_error: None,
        reset_scroll: true,
        encoder: Setup::new(Ok(Some(Ffmpeg::on_path()))),
    }
}

#[test]
fn chapter_list_reopens_at_the_top_and_keeps_scroll_during_selection() {
    let ctx = egui::Context::default();
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 500.0),
        )),
        ..egui::RawInput::default()
    };
    let frame = |form: &mut Form| {
        let mut result = 0.0;
        ctx.run_ui(input(), |ui| {
            let id = ui.make_persistent_id(egui::IdSalt::new(CHAPTER_SCROLL));
            form.show(ui, false);
            result = egui::scroll_area::State::load(&ctx, id)
                .unwrap()
                .clamped_offset()
                .y;
        })
        .textures_delta
        .clear();
        result
    };
    let mut form = chapter_list_form();
    assert_eq!(frame(&mut form), 0.0);
    ctx.run_ui(input(), |ui| {
        let scroll = egui::ScrollArea::vertical()
            .id_salt(CHAPTER_SCROLL)
            .max_height(150.0)
            .animated(false)
            .show(ui, |ui| {
                ui.set_min_height(400.0);
                ui.scroll_with_delta(egui::vec2(0.0, -100.0));
            });
        assert_eq!(scroll.state.clamped_offset().y, 100.0);
    })
    .textures_delta
    .clear();
    assert_eq!(frame(&mut form), 100.0);
    form.selected[0] = false;
    assert_eq!(frame(&mut form), 100.0);
    let mut reopened = chapter_list_form();
    assert_eq!(frame(&mut reopened), 0.0);
}

#[test]
fn prefers_complete_chapters_then_cached_sentences_then_the_reader_voice() {
    let temp = tempfile::tempdir().unwrap();
    let settings = TtsSettings::default();
    let azure = settings.speech(Source::Azure, false);
    let local = settings.speech(Source::Local, false);
    let book = Book {
        title: "測試".into(),
        cover: None,
        dir: temp.path().to_owned(),
        chapters: vec![
            chapter(0, "第一章", "甲。乙。丙。丁。"),
            chapter(1, "第二章", "戊。"),
        ],
        known_profiles: vec![azure.clone()],
        reader_profile: azure.key(""),
    };
    local.record(&book.dir).unwrap();
    let save = |speech: &Speech, text| {
        fs::write(
            clip_path(&book.dir, speech.key(text), speech.ext()),
            b"cached",
        )
        .unwrap();
    };
    save(&local, "戊。");
    for text in ["甲。", "乙。", "丙。"] {
        save(&azure, text);
    }
    let covering = book.coverage(None);
    assert_eq!(book.preferred(&[true, true], &covering).unwrap().0, &local);
    assert_eq!(book.preferred(&[true, false], &covering).unwrap().0, &azure);
    save(&azure, "戊。");
    let covering = book.coverage(None);
    assert_eq!(book.preferred(&[true, true], &covering).unwrap().0, &azure);
    assert_eq!(book.preferred(&[false, true], &covering).unwrap().0, &azure);
    assert!(book.preferred(&[false, false], &covering).is_none());
}

#[test]
fn synthesis_failure_leaves_saved_work_available_to_a_retry() {
    let temp = tempfile::tempdir().unwrap();
    let speech = TtsSettings::default().speech(Source::Azure, false);
    let texts = ["已生成。", "待生成。"];
    let path = |text: &str| clip_path(temp.path(), speech.key(text), MP3);
    fs::write(path(texts[0]), b"existing clip").unwrap();
    let chapter = chapter(0, "章", &texts.join(""));
    let batch = Arc::new(Batch::default());
    let error = Pool::generate(
        [&chapter.sentences],
        temp.path(),
        &speech,
        None,
        &batch,
        Arc::new(|| {}),
    )
    .unwrap_err();
    assert!(error.contains("Azure key"));
    assert_eq!(fs::read(path(texts[0])).unwrap(), b"existing clip");
    assert!(!path(texts[1]).exists());
    fs::write(path(texts[1]), b"new clip").unwrap();
    let retry = Arc::new(Batch::default());
    Pool::generate(
        [&chapter.sentences],
        temp.path(),
        &speech,
        None,
        &retry,
        Arc::new(|| {}),
    )
    .unwrap();
    assert_eq!(retry.cached.load(Relaxed), 2);
}

#[test]
#[ignore = "requires FFmpeg with AAC and libopus and ffprobe on PATH"]
fn exports_nonconsecutive_cached_chapters_in_both_formats() {
    let temp = tempfile::tempdir().unwrap();
    let speech = TtsSettings::default().speech(Source::Local, false);
    let chapter1 = "第一章 #=;\\";
    let chapter3 = "第三章";
    for (text, count) in [
        ("第一句。", 24_013),
        ("第二句。", 23_641),
        ("第三句。", 24_000),
    ] {
        let clip = opus::encode(&vec![1000; count]).unwrap();
        fs::write(clip_path(temp.path(), speech.key(text), OPUS), clip).unwrap();
    }
    let book = Arc::new(Book {
        title: "測試".into(),
        cover: Cover::from_bytes(&png_cover(80, 120)),
        dir: temp.path().to_owned(),
        chapters: vec![
            chapter(0, chapter1, "第一句。第二句。"),
            chapter(1, "第二章", "略過的句子。"),
            chapter(2, chapter3, "第三句。"),
        ],
        known_profiles: Vec::new(),
        reader_profile: speech.key(""),
    });
    for format in [Format::M4b, Format::Opus] {
        let audio = temp.path().join(format!("聽書.{}", format.ext()));
        let subtitles = audio.with_extension("srt");
        let request = Request {
            book: book.clone(),
            chosen: vec![0, 2],
            speech: speech.clone(),
            format,
            files: Files {
                audio: Destination {
                    path: audio.clone(),
                    replace: false,
                },
                subtitles: Some(Destination {
                    path: subtitles.clone(),
                    replace: true,
                }),
            },
            azure_region: String::new(),
            azure_key: String::new(),
            ffmpeg: Ffmpeg::on_path(),
        };
        let p = progress(3);
        p.batch.cancel.store(true, Relaxed);
        assert!(matches!(request.run(&p, Arc::new(|| {})), Err(e) if e == output::CANCELLED));
        assert!(!audio.exists());
        fs::write(&subtitles, b"old subtitles").unwrap();
        let completed = request.run(&progress(3), Arc::new(|| {})).unwrap();
        assert_eq!(completed.first, 0);
        assert_eq!(completed.speech, speech);
        assert_eq!(
            fs::read_to_string(subtitles).unwrap(),
            "1\n00:00:00,000 --> 00:00:01,001\n第一句。\n\n\
             2\n00:00:01,001 --> 00:00:01,986\n第二句。\n\n\
             3\n00:00:01,986 --> 00:00:02,986\n第三句。\n\n"
        );
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_chapters",
                "-of",
                "json",
            ])
            .arg(audio)
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(
            metadata["streams"][0]["codec_name"],
            if format == Format::M4b { "aac" } else { "opus" }
        );
        let cover = metadata["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["disposition"]["attached_pic"] == 1)
            .expect("embedded cover");
        assert_eq!(cover["codec_name"], "mjpeg");
        assert_eq!(cover["width"], 80);
        assert_eq!(cover["height"], 120);
        let chapters = metadata["chapters"].as_array().unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(
            chapters[0]["tags"]["title"].as_str().unwrap().trim_end(),
            chapter1
        );
        assert_eq!(chapters[1]["tags"]["title"], chapter3);
        let second: f64 = chapters[1]["start_time"].as_str().unwrap().parse().unwrap();
        assert!((second - 47_654.0 / 24_000.0).abs() <= 0.001);
    }
}

#[test]
#[ignore = "requires FFmpeg with libopus and ffprobe on PATH"]
fn multi_hour_opus_keeps_chapters_and_subtitles_on_one_timeline() {
    let temp = tempfile::tempdir().unwrap();
    let speech = TtsSettings::default().speech(Source::Local, false);
    let sentence = "長音訊測試。";
    let clip = opus::encode(&vec![0; 60 * opus::RATE as usize]).unwrap();
    fs::write(clip_path(temp.path(), speech.key(sentence), OPUS), clip).unwrap();
    let book = Arc::new(Book {
        title: "長音訊".into(),
        cover: None,
        dir: temp.path().to_owned(),
        chapters: vec![
            chapter(0, "第一章", &sentence.repeat(61)),
            chapter(1, "第二章", &sentence.repeat(60)),
        ],
        known_profiles: Vec::new(),
        reader_profile: speech.key(""),
    });
    let audio = temp.path().join("long.opus");
    let subtitles = temp.path().join("long.srt");
    let request = Request {
        book,
        chosen: vec![0, 1],
        speech,
        format: Format::Opus,
        files: Files {
            audio: Destination {
                path: audio.clone(),
                replace: false,
            },
            subtitles: Some(Destination {
                path: subtitles.clone(),
                replace: false,
            }),
        },
        azure_region: String::new(),
        azure_key: String::new(),
        ffmpeg: Ffmpeg::on_path(),
    };
    request.run(&progress(121), Arc::new(|| {})).unwrap();
    let srt = fs::read_to_string(subtitles).unwrap();
    assert!(srt.contains("62\n01:01:00,000 --> 01:02:00,000"));
    assert!(srt.contains("121\n02:00:00,000 --> 02:01:00,000"));
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "chapter=start_time:format=duration",
            "-of",
            "json",
        ])
        .arg(audio)
        .output()
        .unwrap();
    assert!(probe.status.success());
    let values: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(values["chapters"][1]["start_time"], "3660.000000");
    let duration: f64 = values["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((duration - 7260.0).abs() < 0.02);
}

#[test]
#[ignore = "requires a sound device"]
fn reader_plays_the_cached_sentence_before_requesting_setup() {
    use crate::reader::tts::audio::MUTE;
    let temp = tempfile::tempdir().unwrap();
    let speech = TtsSettings::default().speech(Source::Local, false);
    let clip = opus::encode(&vec![0; opus::RATE as usize]).unwrap();
    fs::write(clip_path(temp.path(), speech.key("已生成。"), OPUS), clip).unwrap();
    let document = scraper::Html::parse_document("<p>已生成。待生成。</p>");
    let chapter = tree::build_doc(
        &document,
        &tree::ua_rules(),
        &tree::rules(""),
        tree::Source::Epub {
            base: String::new(),
            path: String::new(),
        },
    );
    let sentences = crate::reader::tts::sentences(&chapter.chars);
    let chapters = Arc::new(Mutex::new(vec![Some(Arc::new(chapter))]));
    MUTE.store(true, Relaxed);
    let engine = Engine::start(chapters, temp.path().to_owned(), speech, Arc::new(|| {}));
    engine.send(Cmd::Availability(Availability::Setup(
        "Set up the voice.".into(),
    )));
    engine.send(Cmd::Play { spine: 0, src: 0 });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut highlighted = false;
    loop {
        let now = engine.now();
        if now.state == State::Playing {
            highlighted |= now.at == Some((0, sentences[0].clone()));
        }
        if now.needs_setup {
            assert!(highlighted);
            assert_eq!(now.error.as_deref(), Some("Set up the voice."));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{now:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a configured local model and its Vulkan runtime"]
fn local_synthesis_fills_the_same_cache_the_exporter_reads() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        local::downloaded(),
        "set HANSTAT_MODEL_DIR to the installed models"
    );
    let mut settings = TtsSettings::default();
    settings.local.parallel = 1;
    let speech = settings.speech(Source::Local, false);
    let text = "今天的天气很不错。";
    let chapter = chapter(0, "試聽", text);
    let batch = Arc::new(Batch::default());
    let result = Pool::generate(
        [&chapter.sentences],
        temp.path(),
        &speech,
        None,
        &batch,
        Arc::new(|| {}),
    );
    local::unload();
    result.unwrap();
    let file = saved_clip(temp.path(), speech.key(text), OPUS).unwrap();
    let (samples, _) = speech.decode(fs::read(file).unwrap()).unwrap();
    assert!(samples.len() > 1000);
    assert_eq!(batch.done.load(Relaxed), 1);
    assert_eq!(
        fs::read_dir(temp.path().join("profiles")).unwrap().count(),
        1
    );
}
