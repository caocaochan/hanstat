//! Build `data/CEDICT-words.txt`, the general-language words word discovery
//! checks besides jieba's dictionary: CC-CEDICT headwords of 2+ Chinese
//! characters with at least one common-noun entry (proper nouns have
//! capitalised pinyin: 北京 Běijīng), minus those jieba already knows.
//! A Traditional headword is kept only when its Simplified form isn't.
//! cargo run --release --example cedict_words -- <CC-CEDICT Yomitan zip> > data/CEDICT-words.txt
//! (the Yomitan conversion by MarvNC/cc-cedict-yomitan of the MDBG release.)
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

fn main() {
    let path = std::env::args().nth(1).expect("CC-CEDICT Yomitan zip");
    let mut z = zip::ZipArchive::new(std::fs::File::open(&path).expect("open zip")).expect("zip");
    let mut revision = String::new();
    // headword -> has a common (lowercase-pinyin) entry
    let mut common: BTreeMap<String, bool> = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().expect("entry name").to_string();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        if name == "index.json" {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            revision = v["revision"].as_str().unwrap_or("").to_owned();
            continue;
        }
        if !name.starts_with("term_bank_") {
            continue;
        }
        let entries: Vec<serde_json::Value> = serde_json::from_str(&s).unwrap();
        for e in entries {
            let (Some(t), Some(r)) = (e[0].as_str(), e[1].as_str()) else {
                continue;
            };
            if t.chars().count() < 2 || !t.chars().all(hanstat::chars::is_han) {
                continue;
            }
            let lower = r.chars().next().is_some_and(|c| !c.is_uppercase());
            *common.entry(t.to_owned()).or_default() |= lower;
        }
    }
    let jieba = jieba_rs::Jieba::new();
    let kept: BTreeSet<&str> = common
        .iter()
        .filter(|(_, c)| **c)
        .map(|(w, _)| w.as_str())
        .filter(|w| !jieba.has_word(w) && !jieba.has_word(&hanstat::script::fold_t2s(w)))
        .collect();
    let out: Vec<&str> = kept
        .iter()
        .copied()
        .filter(|w| {
            let s = hanstat::script::fold_t2s(w);
            s == *w || !kept.contains(s.as_str())
        })
        .collect();
    println!(
        "# CC-CEDICT {revision} (https://cc-cedict.org, CC BY-SA 4.0; see CEDICT-LICENSE.txt):"
    );
    println!(
        "# common-noun headwords not in jieba's dictionary, built by examples/cedict_words.rs."
    );
    for w in &out {
        println!("{w}");
    }
    eprintln!(
        "{} headwords with a common entry, {} written",
        common.values().filter(|c| **c).count(),
        out.len()
    );
}
