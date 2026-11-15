//! Effect of known words on segmentation accuracy (SIGHAN 2005 gold sets):
//! none vs. hard forcing (user dictionary) vs. the boundary-respecting merge.
//! cargo run --release --example known_eval -- <dir with *_gold.utf8> [max HSK band, default 6]
use hanstat::script::Variant;
use hanstat::segment::{Segmenter, SegmenterKind};
use rayon::prelude::*;
use std::collections::HashSet;

fn spans<S: AsRef<str>>(words: &[S]) -> HashSet<(usize, usize)> {
    let mut pos = 0;
    words
        .iter()
        .map(|w| {
            let n = w.as_ref().chars().count();
            let s = (pos, pos + n);
            pos += n;
            s
        })
        .collect()
}

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("gold dir");
    let top: u8 = a.next().map(|x| x.parse().unwrap()).unwrap_or(6);
    let known: Vec<String> = (1..=top)
        .flat_map(hanstat::known::hsk_words)
        .map(str::to_owned)
        .collect();
    println!("known words: HSK 1–{top}, {}", known.len());
    let sets = [
        (
            "PKU",
            "pku_test_gold.utf8",
            Variant::Simplified,
            SegmenterKind::Ltp,
        ),
        (
            "MSR",
            "msr_test_gold.utf8",
            Variant::Simplified,
            SegmenterKind::Ltp,
        ),
        (
            "AS",
            "as_testing_gold.utf8",
            Variant::Traditional,
            SegmenterKind::Ckip,
        ),
    ];
    println!(
        "{:5} {:6} {:8} {:>7} {:>7} {:>7}",
        "set", "model", "known", "P", "R", "F1"
    );
    for (name, file, variant, kind) in sets {
        let Ok(text) = std::fs::read_to_string(format!("{dir}/{file}")) else {
            continue;
        };
        let gold: Vec<Vec<&str>> = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .filter(|w| !w.is_empty())
            .collect();
        let raws: Vec<String> = gold.iter().map(|ws| ws.concat()).collect();
        let refs: Vec<&str> = raws.iter().map(String::as_str).collect();
        let segs = [
            ("none", Segmenter::new(kind, &[], variant).unwrap()),
            ("forced", Segmenter::new(kind, &known, variant).unwrap()),
            (
                "merged",
                Segmenter::new(kind, &[], variant)
                    .unwrap()
                    .with_known(&known)
                    .unwrap(),
            ),
        ];
        for (label, seg) in &segs {
            let preds: Vec<Vec<String>> = refs
                .par_chunks(16)
                .flat_map_iter(|c| {
                    seg.cut_many(c)
                        .into_iter()
                        .map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>())
                })
                .collect();
            let (mut tp, mut np, mut ng) = (0usize, 0usize, 0usize);
            for (p, g) in preds.iter().zip(&gold) {
                let (ps, gs) = (spans(p), spans(g));
                tp += ps.intersection(&gs).count();
                np += ps.len();
                ng += gs.len();
            }
            let (p, r) = (tp as f64 / np as f64, tp as f64 / ng as f64);
            println!(
                "{name:5} {:6} {label:8} {:>7.2} {:>7.2} {:>7.2}",
                format!("{kind:?}"),
                p * 100.0,
                r * 100.0,
                200.0 * p * r / (p + r)
            );
        }
    }
}
