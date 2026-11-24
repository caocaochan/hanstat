//! End-to-end checks on hand-built inputs with hand-counted expectations.

use hanstat::analyze::{analyze_book, build_report};
use hanstat::extract::{self, EncodingChoice, SectionKind};
use hanstat::segment::SegmenterKind;
use std::io::{Cursor, Write};

fn build_epub(files: &[(&str, &str)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

const OPF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>测试书</dc:title></metadata>
<manifest>
  <item id="nav" href="Text/nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
  <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  <item id="c1" href="Text/ch%201.xhtml" media-type="application/xhtml+xml"/>
  <item id="n1" href="Text/notes.xhtml" media-type="application/xhtml+xml"/>
  <item id="orphan" href="Text/orphan.xhtml" media-type="application/xhtml+xml"/>
</manifest>
<spine toc="ncx">
  <itemref idref="nav"/>
  <itemref idref="c1"/>
  <itemref idref="c1"/>
  <itemref idref="n1" linear="no"/>
</spine></package>"#;

const NCX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><navMap>
<navPoint id="p1"><navLabel><text>注释页</text></navLabel><content src="Text/notes.xhtml#n1"/></navPoint>
</navMap></ncx>"#;

const NAV: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>目录</title></head>
<body><nav epub:type="toc"><ol><li><a href="ch%201.xhtml">第一章</a></li></ol></nav></body></html>"#;

fn ch1() -> String {
    // Han: 第一章(3) 我们学习汉字(6) 中国𠮷豈(4) 葛城人(3) = 16, all distinct.
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>不该计入</title><style>p {{ color: red }}</style></head>
<body>
<h1>第一章</h1>
<p>我们<span class="a">学</span>习<ruby>汉<rt>hàn</rt></ruby><ruby>字<rp>(</rp><rt>zì</rt><rp>)</rp></ruby>。</p>
<p>中{zwsp}国&#x20BB7;{compat}</p>
<p>葛{ivs}城
   人</p>
<p hidden="hidden">隐藏文字</p>
<div style="display:none">也隐藏</div>
<img src="x.png" alt="图片说明"/>
<p>{pua}ABC abc 123</p>
<script>var s = "脚本";</script>
</body></html>"#,
        zwsp = '\u{200B}',
        compat = '\u{F900}',
        ivs = '\u{E0100}',
        pua = '\u{E000}'
    )
}

const NOTES: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>n</title></head><body><p id="n1">注释：我们。</p></body></html>"#;
const ORPHAN: &str =
    r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>孤儿文件不计</p></body></html>"#;

#[test]
fn epub_adversarial() {
    let ch1 = ch1();
    let bytes = build_epub(&[
        ("mimetype", "application/epub+zip"),
        ("META-INF/container.xml", CONTAINER),
        ("OEBPS/content.opf", OPF),
        ("OEBPS/toc.ncx", NCX),
        ("OEBPS/Text/nav.xhtml", NAV),
        ("OEBPS/Text/ch 1.xhtml", &ch1),
        ("OEBPS/Text/notes.xhtml", NOTES),
        ("OEBPS/Text/orphan.xhtml", ORPHAN),
    ]);
    let book = extract::epub::load(bytes).unwrap();
    assert_eq!(book.title.as_deref(), Some("测试书"));
    let kinds: Vec<_> = book
        .sections
        .iter()
        .map(|s| (s.label.as_str(), s.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("目录", SectionKind::Nav),
            ("第一章", SectionKind::Body),
            ("注释页", SectionKind::NonLinear)
        ]
    );
    assert_eq!(
        book.sections[1].paragraphs,
        [
            "第一章",
            "我们学习汉字。",
            "中\u{200B}国𠮷\u{F900}",
            "葛\u{E0100}城人",
            "\u{E000}ABC abc 123"
        ]
    );

    let prep = analyze_book(&book, SegmenterKind::Jieba, &[], &[], &|_, _| {}).unwrap();
    let inc: Vec<bool> = book.sections.iter().map(|s| s.default_included()).collect();
    let r = build_report(&book, &prep, &inc, false);
    assert_eq!(r.han_total, 20);
    assert_eq!(r.han_unique, 18);
    assert_eq!(r.private_use, 1);
    assert_eq!(r.foreign_words_total, 2);
    assert_eq!(r.foreign_words_unique, 1);
    assert_eq!(r.numbers, 1);
    assert_eq!(r.norm.invisible_removed, 1);
    assert_eq!(r.norm.compat_ideographs, 1);
    assert_eq!(r.norm.variation_selectors, 1);
    assert_eq!(r.paragraphs, 6);
    assert_eq!(r.sentences, 6);
    assert!(r.char_freq.iter().any(|(c, _)| *c == '豈'));
    assert!(r.char_freq.iter().any(|(c, _)| *c == '𠮷'));
    assert!(
        r.word_freq.iter().any(|(w, _)| w == "学习"),
        "span must not split 学习: {:?}",
        r.word_freq
    );
    assert!(r.notes.iter().any(|n| n.contains("duplicate")));
    assert!(r.notes.iter().any(|n| n.contains("not in the spine")));

    // Including the nav doc adds its 3 characters but no new distinct ones.
    let r2 = build_report(&book, &prep, &[true, true, true], false);
    assert_eq!(r2.han_total, 23);
    assert_eq!(r2.han_unique, 18);
}

/// Folded, a book that mixes scripts counts 說 and 说 as one character,
/// shown as the book more often writes it; unfolded, as two.
#[test]
fn chars_fold_across_scripts() {
    let text = "第一章\n他說話。他說了。我说。\n";
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Auto);
    let prep = analyze_book(&book, SegmenterKind::Jieba, &[], &[], &|_, _| {}).unwrap();
    let inc = vec![true; book.sections.len()];
    let apart = build_report(&book, &prep, &inc, false);
    let folded = build_report(&book, &prep, &inc, true);
    assert!(folded.chars_folded && !apart.chars_folded);
    assert_eq!(apart.han_total, folded.han_total);
    assert_eq!(apart.han_unique, folded.han_unique + 1); // 說 and 说
    let count =
        |r: &hanstat::analyze::Report, c: char| r.char_freq.iter().find(|x| x.0 == c).map(|x| x.1);
    assert_eq!(count(&folded, '說'), Some(3));
    assert_eq!(count(&folded, '说'), None);
    let rows = |r: &hanstat::analyze::Report| -> usize {
        r.sections.iter().filter_map(|s| s.new_han).sum()
    };
    assert_eq!(rows(&folded), folded.han_unique);
}

#[test]
fn traditional_segmentation_keeps_original_chars() {
    let para = "我們學習漢字，這個問題很難。他們說話的時候總是很認真。\n";
    let text = format!("第一章 測試\n{}", para.repeat(10));
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Auto);
    for kind in SegmenterKind::ALL {
        let prep = analyze_book(&book, kind, &[], &[], &|_, _| {}).unwrap();
        let r = build_report(&book, &prep, &vec![true; book.sections.len()], false);
        assert_eq!(r.variant, hanstat::script::Variant::Traditional);
        let words: Vec<&str> = r.word_freq.iter().map(|(w, _)| w.as_str()).collect();
        for w in ["我們", "學習", "漢字", "問題"] {
            assert!(words.contains(&w), "{kind:?} missing {w}: {words:?}");
        }
    }
}

#[test]
fn discovers_invented_name() {
    // An invented name in many different contexts, mixed with ordinary text.
    let before = [
        "我看见",
        "他说",
        "你和",
        "只有",
        "突然",
        "原来是",
        "因为",
        "门外的",
        "可是",
        "他们找到",
    ];
    let after = [
        "笑了",
        "走过来",
        "没有回答",
        "点点头",
        "皱起眉",
        "转身离开",
        "看着窗外",
        "站在原地",
        "叹了口气",
        "沉默了",
    ];
    let mut text = String::from("第一章 开始\n");
    for (i, b) in before.iter().enumerate() {
        for a in &after[i % 3..] {
            text.push_str(&format!("{b}墨尘渊{a}。今天的天气很好，大家都很高兴。\n"));
        }
    }
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Auto);
    let prep = analyze_book(&book, SegmenterKind::Ltp, &[], &[], &|_, _| {}).unwrap();
    let inc = vec![true; book.sections.len()];
    let r = build_report(&book, &prep, &inc, false);
    let found = hanstat::discover::discover(
        &book,
        &inc,
        &r.word_freq,
        &Default::default(),
        &hanstat::discover::Options { min_occurrences: 5 },
    );
    let c = found
        .iter()
        .find(|c| c.word == "墨尘渊")
        .unwrap_or_else(|| panic!("not found: {found:?}"));
    assert!(c.recommended, "{c:?}");
    assert!(
        !found.iter().any(|c| c.word == "尘渊" && c.recommended),
        "fragment recommended"
    );
    assert!(
        !found
            .iter()
            .any(|c| c.recommended && c.word.contains("天气")),
        "ordinary words recommended"
    );
}

#[test]
fn user_dictionary_merges_names() {
    let text = "温简言看向不远处的黑方小队。温简言笑了。\n";
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Auto);
    for kind in SegmenterKind::ALL {
        let prep = analyze_book(&book, kind, &["温简言".into()], &[], &|_, _| {}).unwrap();
        let r = build_report(&book, &prep, &[true], false);
        let n = r.word_freq.iter().find(|(w, _)| w == "温简言").map(|x| x.1);
        assert_eq!(n, Some(2), "{kind:?}: {:?}", r.word_freq);
    }
}

#[test]
fn analysis_cache_gives_the_same_counts() {
    use hanstat::analyze::analyze_book_cached;
    use hanstat::cache::Cache;

    let dir = std::env::temp_dir().join(format!("hanstat-fixtures-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = Cache::at(&dir);
    let text = "第一章 开始\n我们今天下午一起去图书馆看书。然后研究生命的起源问题！\nABC abc 123\n\n第二章 结束\n她只看了一瞬间就走了。\n";
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Utf8);
    let run = |book: &extract::Book, words: &[String], known: &[String], cache: Option<&Cache>| {
        analyze_book_cached(book, SegmenterKind::Jieba, words, known, cache, &|_, _| {}).unwrap()
    };
    let inc: Vec<bool> = book.sections.iter().map(|_| true).collect();
    let json = |p| serde_json::to_string(&build_report(&book, p, &inc, false)).unwrap();

    let plain = run(&book, &[], &[], None);
    let first = run(&book, &[], &[], Some(&cache));
    let second = run(&book, &[], &[], Some(&cache));
    assert!(!plain.cached && !first.cached);
    assert!(second.cached);
    assert_eq!(plain.stats, first.stats);
    assert_eq!(plain.stats, second.stats);
    assert_eq!(json(&plain), json(&second));
    assert_eq!(cache.usage().0, 1);

    // Other words are another analysis; the first one is still there.
    let forced = ["研究生命".to_owned()];
    let a = run(&book, &forced, &[], Some(&cache));
    assert!(!a.cached);
    assert_ne!(a.stats, plain.stats);
    let b = run(&book, &forced, &[], Some(&cache));
    assert!(b.cached);
    assert_eq!((a.stats, a.user_words), (b.stats, b.user_words));
    let known = ["一瞬间".to_owned(), "图书馆".to_owned()];
    let a = run(&book, &[], &known, Some(&cache));
    assert!(!a.cached);
    let b = run(&book, &[], &known, Some(&cache));
    assert!(b.cached);
    assert_eq!((a.stats, a.known_words), (b.stats, b.known_words));
    assert!(run(&book, &[], &[], Some(&cache)).cached);
    assert_eq!(cache.usage().0, 3);

    // A changed character is another book.
    let other = extract::txt::load(
        text.replace("图书馆", "图书店").as_bytes(),
        EncodingChoice::Utf8,
    );
    assert!(!run(&other, &[], &[], Some(&cache)).cached);

    std::fs::remove_dir_all(&dir).unwrap();
}

/// What the reading path's "Analyse" does for a book without word data:
/// analyse the file, save its profile, and plan with it.
#[test]
fn book_file_to_profile_to_reading_path() {
    use hanstat::plan::{Library, Params, Study};
    use hanstat::profile::{Profile, analyse_file, path_in};
    let dir = std::env::temp_dir().join(format!("hanstat-path-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut books = Vec::new();
    for (name, text) in [
        (
            "a.txt",
            "温简言看向不远处的黑方小队。温简言笑了。我们今天去学校。\n",
        ),
        ("b.txt", "我们今天去学校。温简言看向黑方小队。\n"),
    ] {
        let file = dir.join(name);
        std::fs::write(&file, text).unwrap();
        let a = analyse_file(
            &file,
            SegmenterKind::Jieba,
            None,
            &["温简言".into()],
            &[],
            None,
            true,
        )
        .unwrap();
        let id = a.file_id.unwrap();
        assert!(a.names.iter().any(|n| n == "温简言"));
        let p = Profile::from_report(&id, &a.report, &a.names);
        let path = path_in(&dir.join("history.json"), &id);
        p.save(&path).unwrap();
        books.push((name.to_owned(), Profile::load(&path).unwrap()));
    }
    let lib = Library::build(true, false, books);
    let k0 = lib.known(None);
    let p = lib.evaluate(
        &k0,
        &Params {
            study: Study::Target(100.0),
            ..Params::default()
        },
        &[0, 1],
    );
    // The name is never studied; b's words all come from a.
    assert!(p.steps[0].study.iter().all(|&w| lib.word(w) != "温简言"));
    assert!(
        p.steps[1].study.is_empty(),
        "{:?}",
        lib.study_list(1, &p.steps[1].study)
    );
    assert_eq!(p.steps[1].reached, 100.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Library row whose counts are current is updated from its saved word
/// data alone when only the known words change: that must give the row the
/// full report gives.
#[test]
fn profile_alone_gives_the_rows_coverage() {
    use hanstat::anki::{BookCoverage, Knowledge};
    use hanstat::history::Known;
    use hanstat::profile::Profile;
    use std::collections::HashSet;
    let text = "第一章\n他說話。他說了。我们学习汉字，这个问题很难。林原说话很认真。林原来了。\n";
    let book = extract::txt::load(text.as_bytes(), EncodingChoice::Auto);
    let prep = analyze_book(&book, SegmenterKind::Jieba, &[], &[], &|_, _| {}).unwrap();
    let inc = vec![true; book.sections.len()];
    let names: HashSet<String> = ["林原".to_owned()].into();
    for fold in [false, true] {
        let r = build_report(&book, &prep, &inc, fold);
        let mut k = Knowledge::empty(fold);
        k.add_listed(
            r.word_freq.iter().step_by(2).map(|(w, _)| w.as_str()),
            "test".into(),
        );
        for skip_hapax in [false, true] {
            let from_report =
                BookCoverage::compute(&k, &r.char_freq, &r.word_freq, &names, skip_hapax);
            let dir =
                std::env::temp_dir().join(format!("hanstat-cov-{}-{fold}", std::process::id()));
            let path = dir.join("p.json.gz");
            Profile::from_report("id", &r, &names).save(&path).unwrap();
            let p = Profile::load(&path).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            let p_names: HashSet<String> = p.names.iter().cloned().collect();
            let from_profile = BookCoverage::compute(
                &k,
                p.chars.as_ref().unwrap(),
                &p.words,
                &p_names,
                skip_hapax,
            );
            assert_eq!(
                Known::from_coverage(&from_report),
                Known::from_coverage(&from_profile)
            );
        }
    }
}
