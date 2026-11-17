//! Build the bundled headword lists from Yomitan dictionaries (headwords only,
//! no definitions), all folded to Simplified:
//! * `data/chengyu.txt`: idioms of 4+ characters, from 新华成语大词典 and
//!   汉语成语源流大辞典. The segmenter makes each one token where it occurs
//!   (LTP cuts 若有所思 into 若/有所思, or glues a name to it: 白柳若/有所思).
//! * `data/extra-words.txt`: words of 现代汉语规范词典, 现代汉语词典 (7th ed.),
//!   國語辭典簡編本 and 新华成语大词典 that neither jieba's dictionary,
//!   `data/CEDICT-words.txt` nor `data/chengyu.txt` has. Rare tokens in these
//!   lists are kept whole instead of being split again (see `lexicon.rs`).
//!
//! Larger dictionaries (汉语大词典, 重編國語辭典, 现代汉语大词典, 樂詞網, Wikipedia)
//! list the fragments a segmenter leaves (幸灾, 下石, 唾手), so they are not used.
//! cargo run --release --example headwords -- <dicts folder>
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

const CHENGYU: &[&str] = &["xhcycd-yomitan.zip", "hycyyl-yomitan.zip"];
const EXTRA: &[&str] = &[
    "guifan-pinyin.zip",
    "hanyu7-pinyin.zip",
    "moe-concised-pinyin.zip",
    "xhcycd-yomitan.zip",
];

/// Han-only headwords of 2+ characters, folded to Simplified, and the dictionary's title.
fn headwords(path: &Path) -> (String, BTreeSet<String>) {
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut z = zip::ZipArchive::new(file).expect("zip");
    let mut title = String::new();
    let mut out = BTreeSet::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().expect("entry name").to_string();
        if name != "index.json" && !name.starts_with("term_bank_") {
            continue;
        }
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        if name == "index.json" {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            title = v["title"].as_str().unwrap_or("").to_owned();
            continue;
        }
        let entries: Vec<serde_json::Value> = serde_json::from_str(&s).unwrap();
        for e in entries {
            let Some(t) = e[0].as_str() else { continue };
            if t.chars().count() >= 2 && t.chars().all(hanstat::chars::is_han) {
                out.insert(hanstat::script::fold_t2s(t));
            }
        }
    }
    (title, out)
}

fn write(path: &str, header: &[String], words: &BTreeSet<String>) {
    let mut s = String::new();
    for h in header {
        s.push_str(&format!("# {h}\n"));
    }
    for w in words {
        s.push_str(w);
        s.push('\n');
    }
    std::fs::write(path, s).unwrap_or_else(|e| panic!("{path}: {e}"));
    eprintln!("{path}: {} words", words.len());
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("dicts folder"));

    let mut chengyu = BTreeSet::new();
    let mut titles = Vec::new();
    for f in CHENGYU {
        let (title, words) = headwords(&dir.join(f));
        titles.push(title);
        chengyu.extend(words.into_iter().filter(|w| w.chars().count() >= 4));
    }
    write(
        "data/chengyu.txt",
        &[
            format!(
                "Idioms of 4+ characters: headwords of {} (no definitions),",
                titles.join(" and ")
            ),
            "folded to Simplified. Built by examples/headwords.rs; see Headwords-LICENSE.txt."
                .into(),
        ],
        &chengyu,
    );

    let jieba = jieba_rs::Jieba::new();
    let cedict: BTreeSet<String> = std::fs::read_to_string("data/CEDICT-words.txt")
        .expect("data/CEDICT-words.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(hanstat::script::fold_t2s)
        .collect();
    let mut extra = BTreeSet::new();
    let mut titles = Vec::new();
    for f in EXTRA {
        let (title, words) = headwords(&dir.join(f));
        titles.push(title);
        extra.extend(
            words
                .into_iter()
                .filter(|w| !jieba.has_word(w) && !cedict.contains(w) && !chengyu.contains(w)),
        );
    }
    write(
        "data/extra-words.txt",
        &[
            format!("Headwords of {} (no definitions),", titles.join(", ")),
            "folded to Simplified, that jieba's dictionary, CEDICT-words.txt and chengyu.txt lack."
                .into(),
            "Built by examples/headwords.rs; see Headwords-LICENSE.txt.".into(),
        ],
        &extra,
    );
}
