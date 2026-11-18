//! A dictionary zip written by the test, imported, and looked up.

use super::format::{Entry, Manifest};
use super::import::tests::{make_zip, temp_dir};
use super::import::{Progress, import};
use super::lookup::Engine;
use super::manage;
use std::sync::atomic::AtomicBool;

fn term(expr: &str, reading: &str, gloss: &str) -> String {
    format!(
        r#"["{expr}","{reading}","n","",{},{gloss},1,""]"#,
        expr.chars().count()
    )
}

/// Two term banks (the second repeats a glossary of the first), a frequency
/// bank in two value shapes, tags, styles and an image.
fn sample(dir: &std::path::Path) -> std::path::PathBuf {
    let shared = r#"[{"type":"structured-content","content":{"tag":"span","data":{"x":"y"},"content":"to speak"}}]"#;
    let bank1 = format!(
        "[{},{},{},{}]",
        term("说话", "shuō huà", shared),
        term("說話", "shuō huà", shared),
        term("说", "shuō", r#"["to say"]"#),
        term("话", "huà", r#"["word"]"#),
    );
    let bank2 = format!(
        "[{},{},{}]",
        term("说", "shuì", r#"["to persuade"]"#),
        term("说话间", "shuō huà jiān", r#"["meanwhile"]"#),
        term("长", "cháng", shared),
    );
    let meta = r#"[["说话","freq",{"value":194,"displayValue":"194"}],["说","freq",15],["话","freq",{"reading":"huà","frequency":120}],["话","freq",{"reading":"x","frequency":1}],["说","pitch",{}]]"#;
    let z = dir.join("sample.zip");
    make_zip(
        &z,
        &[
            (
                "index.json",
                br#"{"title":"Sample [1]","revision":"2026.1.2","format":3,"isUpdatable":true,
                    "indexUrl":"https://example.org/index.json","downloadUrl":"https://example.org/s.zip"}"#,
            ),
            ("styles.css", b"[data-sc-x=\"y\"] { color: red }"),
            ("term_bank_1.json", bank1.as_bytes()),
            ("term_bank_2.json", bank2.as_bytes()),
            ("term_meta_bank_1.json", meta.as_bytes()),
            ("tag_bank_1.json", br#"[["n","partOfSpeech",0,"noun",0]]"#),
            ("img/a.png", b"\x89PNG fake"),
        ],
    );
    z
}

/// The real dictionaries of `HANSTAT_DICTS` (the 17 zips of `dicts/`),
/// imported into `HANSTAT_DICT_OUT` (deleted after), checked against the
/// counts and lookups measured on 2026-11-18 (the same as hoshidicts'):
///
/// ```text
/// HANSTAT_DICTS=dicts HANSTAT_DICT_OUT=<scratch> cargo test --release --lib dict::tests::real -- --ignored
/// ```
#[test]
#[ignore]
fn real_dictionaries() {
    let (Some(src), Some(out)) = (
        std::env::var_os("HANSTAT_DICTS"),
        std::env::var_os("HANSTAT_DICT_OUT"),
    ) else {
        panic!("set HANSTAT_DICTS and HANSTAT_DICT_OUT");
    };
    let root = std::path::PathBuf::from(out).join("real");
    let _ = std::fs::remove_dir_all(&root);
    // (zip, terms, distinct glossaries, frequencies)
    let want: &[(&str, u64, u64, u64)] = &[
        ("bilibili_slang", 43_926, 26_488, 0),
        ("Danmei", 0, 0, 264_348),
        ("genshin_zh", 37_488, 20_953, 0),
        ("CC-CEDICT", 203_458, 125_422, 0),
        ("guifan-pinyin", 121_485, 75_579, 0),
        ("xhcycd-yomitan", 49_675, 26_558, 0),
        ("liangancidian-pinyin", 165_429, 104_220, 0),
        ("moegirl-yomitan", 159_738, 146_082, 0),
        ("xhdcd-yomitan-text", 183_476, 111_965, 0),
        ("Bilibili", 0, 0, 1_958_154),
        ("hanyu7-pinyin", 114_442, 71_536, 0),
        ("moe-concised-pinyin", 72_847, 46_146, 0),
        ("hycyyl-yomitan", 101_939, 12_270, 0),
        ("moe-revised-pinyin", 269_298, 168_310, 0),
        ("hydcd-qiding-yomitan-light", 936_906, 430_075, 0),
        ("naer_terms", 2_451_838, 1_286_172, 0),
        ("wikipedia_zh", 3_952_585, 2_112_712, 0),
    ];
    let mut titles = std::collections::HashMap::new();
    let mut m = Manifest::default();
    for &(name, terms, glossaries, freqs) in want {
        let zip = std::path::Path::new(&src).join(format!("{name}.zip"));
        let id = super::format::new_id();
        let meta = import(
            &zip,
            &manage::tmp_dir(&root, &id),
            &Progress::default(),
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        manage::swap(&root, &id).unwrap();
        assert_eq!(
            (meta.terms, meta.glossaries, meta.freqs),
            (terms, glossaries, freqs),
            "{name}"
        );
        titles.insert(meta.title.clone(), name);
        m.dicts.push(Entry {
            id,
            enabled: true,
            check: None,
            collapsed: false,
        });
    }
    m.save(&root).unwrap();
    let (mut e, errors) = Engine::load(&root, None);
    assert!(errors.is_empty(), "{errors:?}");
    // Which dictionaries have the word as a headword, and how many glossaries.
    let mut has = |word: &str| -> std::collections::BTreeMap<&str, usize> {
        let r = e.lookup(word);
        let mut out = std::collections::BTreeMap::new();
        for g in r.groups.iter().filter(|g| g.headword == word) {
            for p in &g.parts {
                *out.entry(titles[&e.dicts[p.dict].title]).or_default() += p.glossaries.len();
            }
        }
        out
    };
    let ten = [
        "CC-CEDICT",
        "guifan-pinyin",
        "hanyu7-pinyin",
        "hydcd-qiding-yomitan-light",
        "liangancidian-pinyin",
        "moe-concised-pinyin",
        "moe-revised-pinyin",
        "naer_terms",
        "wikipedia_zh",
        "xhdcd-yomitan-text",
    ];
    for w in ["说话", "說話"] {
        let got: Vec<&str> = has(w).into_keys().collect();
        let mut want = ten.to_vec();
        want.sort();
        assert_eq!(got, want, "{w}");
    }
    // Glossary counts that agree with hoshidicts' (its results grouped by reading).
    assert_eq!(has("不")["hydcd-qiding-yomitan-light"], 10);
    assert_eq!(has("樂")["hydcd-qiding-yomitan-light"], 10);
    assert_eq!(has("了")["CC-CEDICT"], 4);
    assert_eq!(has("是")["hanyu7-pinyin"], 3);
    assert_eq!(has("行")["xhdcd-yomitan-text"], 9);
    // Frequencies (also hoshidicts' values).
    let r = e.lookup("说话");
    let ranks: Vec<(String, i32)> = r.groups[0]
        .freqs
        .iter()
        .map(|b| (e.dicts[b.dict].title.clone(), b.values[0].value))
        .collect();
    assert!(
        ranks.contains(&("Danmei".into(), 194)) && ranks.contains(&("Bilibili".into(), 563)),
        "{ranks:?}"
    );
    // 應該: Danmei has only the Simplified form.
    let r = e.lookup("應該");
    let danmei = r.groups[0]
        .freqs
        .iter()
        .find(|b| e.dicts[b.dict].title == "Danmei")
        .unwrap();
    assert!(danmei.folded);
    // 時: Danmei and Bilibili have the rare Traditional spelling; 时's rank is shown.
    let r = e.lookup("時");
    for b in &r.groups[0].freqs {
        assert!(b.folded, "{}", e.dicts[b.dict].title);
        assert!(
            b.values[0].value < 5000,
            "{} {}",
            e.dicts[b.dict].title,
            b.values[0].value
        );
    }
    // One group for readings that differ only in spacing.
    let r = e.lookup("画蛇添足");
    let readings: Vec<&String> = r
        .groups
        .iter()
        .filter(|g| g.headword == "画蛇添足")
        .map(|g| &g.reading)
        .collect();
    assert_eq!(readings.len(), 1, "{readings:?}");
    drop(e);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn import_and_look_up() {
    let root = temp_dir("roundtrip");
    let z = sample(&root);
    let id = "0000000000000001";
    let m = import(
        &z,
        &manage::tmp_dir(&root, id),
        &Progress::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    manage::swap(&root, id).unwrap();
    assert_eq!(m.title, "Sample [1]");
    assert_eq!((m.terms, m.freqs, m.media), (7, 4, 1));
    assert_eq!(m.glossaries, 5, "the shared glossary (3 terms) once");
    assert_eq!(m.max_len, 3);
    assert_eq!(m.skipped.get("pitch"), Some(&1));
    assert_eq!(m.tags[0].notes, "noun");
    assert!(m.update.is_some() && m.styles.contains("data-sc-x"));
    assert_eq!(
        super::store::read_media(&root.join(id), "img/a.png").as_deref(),
        Some(&b"\x89PNG fake"[..])
    );
    Manifest {
        dicts: vec![Entry {
            id: id.into(),
            enabled: true,
            check: None,
            collapsed: false,
        }],
        last_auto_check: 0,
        default_installed: false,
    }
    .save(&root)
    .unwrap();

    let (mut e, errors) = Engine::load(&root, Some(id));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(e.scan_len(), 3);
    // Longest match first; then shorter ones; 说 has two readings.
    let r = e.lookup("说话间很好");
    let heads: Vec<(&str, &str, usize)> = r
        .groups
        .iter()
        .map(|g| (g.headword.as_str(), g.reading.as_str(), g.matched))
        .collect();
    assert_eq!(
        heads,
        [
            ("说话间", "shuō huà jiān", 3),
            ("说话", "shuō huà", 2),
            ("说", "shuō", 1),
            ("说", "shuì", 1)
        ]
    );
    let g = &r.groups[1];
    assert!(
        g.parts[0].glossaries[0]
            .json
            .as_ref()
            .unwrap()
            .contains("to speak")
    );
    assert_eq!(g.freqs[0].values[0].value, 194);
    // 话: the reading-limited frequency for huà only.
    let r = e.lookup("话");
    assert_eq!(r.groups[0].freqs[0].values.len(), 1);
    assert_eq!(r.groups[0].freqs[0].values[0].value, 120);
    // Traditional text: 說話 is a headword; the frequency comes from 说话.
    let r = e.lookup("說話了");
    assert_eq!(r.groups[0].headword, "說話");
    assert!(!r.groups[0].folded);
    assert!(r.groups[0].freqs[0].folded);
    // 長 is no headword: found folded to 长.
    let r = e.lookup("長");
    assert_eq!(r.groups[0].headword, "长");
    assert!(r.groups[0].folded);
    assert_eq!(r.matched(), 1);
    // Nothing there.
    assert_eq!(e.lookup("。").groups.len(), 0);
    // The cache gives the same result back.
    let a = e.lookup("说话间很好");
    let b = e.lookup("说话间很好");
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    // Unloaded: its files can be deleted.
    e.unload(&[id.to_string()]);
    assert!(e.is_empty());
    manage::delete(&root, id).unwrap();
    assert!(!root.join(id).exists());
    assert!(Manifest::load(&root).dicts.is_empty());
    std::fs::remove_dir_all(&root).ok();
}
