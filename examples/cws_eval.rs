//! Word-segmentation accuracy on SIGHAN 2005 (icwb2) gold test sets.
//! cargo run --release --example cws_eval -- <dir with *_gold.utf8> [--base models/ckip/bert-base-chinese-ws]
use hanstat::ckip::Ckip;
use hanstat::script::Variant;
use hanstat::segment::{Segmenter, SegmenterKind};
use rayon::prelude::*;
use std::collections::HashSet;
use std::time::Instant;

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
    let mut base_dir = None;
    while let Some(x) = a.next() {
        if x == "--base" {
            base_dir = a.next();
        }
    }
    let base = base_dir.map(|d| Ckip::from_pth_dir(std::path::Path::new(&d)).expect("load base"));

    let sets = [
        ("PKU (simp)", "pku_test_gold.utf8", Variant::Simplified),
        ("MSR (simp)", "msr_test_gold.utf8", Variant::Simplified),
        ("AS (trad)", "as_testing_gold.utf8", Variant::Traditional),
        ("CityU (trad)", "cityu_test_gold.utf8", Variant::Traditional),
    ];
    println!(
        "{:14} {:24} {:>7} {:>7} {:>7} {:>9}",
        "test set", "segmenter", "P", "R", "F1", "chars/s"
    );
    for (name, file, variant) in sets {
        let text = std::fs::read_to_string(format!("{dir}/{file}")).expect("gold");
        let gold: Vec<Vec<&str>> = text
            .lines()
            .map(|l| {
                l.split(|c: char| c.is_whitespace())
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|w| !w.is_empty())
            .collect();
        let raws: Vec<String> = gold.iter().map(|ws| ws.concat()).collect();
        let nchars: usize = raws.iter().map(|r| r.chars().count()).sum();

        type Run<'a> = Box<dyn Fn(&[&str]) -> Vec<Vec<String>> + Sync + 'a>;
        let mut runs: Vec<(String, Run)> = Vec::new();
        for kind in [
            SegmenterKind::Jieba,
            SegmenterKind::Ltp,
            SegmenterKind::Ckip,
        ] {
            let seg = Segmenter::new(kind, &[], variant).unwrap();
            runs.push((
                kind.label().to_owned(),
                Box::new(move |c: &[&str]| {
                    seg.cut_many(c)
                        .into_iter()
                        .map(|v| v.into_iter().map(str::to_owned).collect())
                        .collect()
                }),
            ));
        }
        if let Some(b) = &base {
            let fold = variant != Variant::Traditional;
            runs.push((
                "CKIP BERT-base".to_owned(),
                Box::new(move |c: &[&str]| {
                    b.segment_many(c, fold)
                        .into_iter()
                        .map(|v| v.into_iter().map(str::to_owned).collect())
                        .collect()
                }),
            ));
        }
        for (label, run) in &runs {
            let t = Instant::now();
            let refs: Vec<&str> = raws.iter().map(String::as_str).collect();
            let preds: Vec<Vec<String>> = refs.par_chunks(16).flat_map_iter(run).collect();
            let dt = t.elapsed().as_secs_f64();
            let (mut tp, mut np, mut ng) = (0usize, 0usize, 0usize);
            for (p, g) in preds.iter().zip(&gold) {
                let (ps, gs) = (spans(p), spans(g));
                tp += ps.intersection(&gs).count();
                np += ps.len();
                ng += gs.len();
            }
            let (p, r) = (tp as f64 / np as f64, tp as f64 / ng as f64);
            println!(
                "{name:14} {label:24} {:>7.2} {:>7.2} {:>7.2} {:>9.0}",
                p * 100.0,
                r * 100.0,
                200.0 * p * r / (p + r),
                nchars as f64 / dt
            );
        }
    }
}
