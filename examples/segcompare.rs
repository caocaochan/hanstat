//! Side-by-side comparison of jieba-rs and LTP legacy CWS.
//! Usage: cargo run --release --example segcompare -- <file.txt>
use std::time::Instant;

fn main() -> anyhow_free::Result {
    let path = std::env::args().nth(1).expect("path");
    let text = std::fs::read_to_string(&path).expect("utf-8 file");

    let t = Instant::now();
    let ltp = hanstat::segment::ltp_model().unwrap();
    eprintln!("ltp load: {:?}", t.elapsed());
    let t = Instant::now();
    let jieba = jieba_rs::Jieba::new();
    eprintln!("jieba load: {:?}", t.elapsed());

    let fixed = [
        "他叫汤姆去拿外衣。",
        "结婚的和尚未结婚的",
        "南京市长江大桥",
        "研究生命的起源",
        "他说的确实在理",
        "这个门把手坏了请把手拿开",
        "工信处女干事每月经过下属科室都要亲口交代24口交换机等技术性器件的安装工作",
    ];
    // Pull a few Han-heavy lines from the novel.
    let samples: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.chars().count() > 25)
        .step_by(4000)
        .take(8)
        .collect();

    for s in fixed.iter().copied().chain(samples) {
        let j: Vec<&str> = jieba.cut(s, false).into_iter().map(|t| t.word).collect();
        let l: Vec<&str> = ltp.predict(s).unwrap();
        println!("jieba: {}", j.join(" / "));
        println!("ltp  : {}", l.join(" / "));
        println!();
    }

    // Throughput on the whole file, line by line.
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let t = Instant::now();
    let mut nj = 0usize;
    for l in &lines {
        nj += jieba.cut(l, false).len();
    }
    eprintln!("jieba full file: {} tokens in {:?}", nj, t.elapsed());
    let t = Instant::now();
    let mut nl = 0usize;
    for l in &lines {
        nl += ltp.predict(l).unwrap().len();
    }
    eprintln!("ltp   full file: {} tokens in {:?}", nl, t.elapsed());

    // Book-level word stats: Han-only tokens.
    let is_han = |w: &str| {
        !w.is_empty()
            && w.chars()
                .all(|c| unicode_script::UnicodeScript::script(&c) == unicode_script::Script::Han)
    };
    let report = |name: &str, toks: Vec<&str>| {
        let mut m: std::collections::HashMap<&str, u32> = Default::default();
        for w in toks.into_iter().filter(|w| is_han(w)) {
            *m.entry(w).or_default() += 1;
        }
        let total: u32 = m.values().sum();
        let single = m.keys().filter(|w| w.chars().count() == 1).count();
        let name_whole = m.get("温简言").copied().unwrap_or(0);
        let name_split = m.get("简言").copied().unwrap_or(0);
        println!(
            "{name:12} total={total:8} unique={:6} unique_1char={single:5} 温简言={name_whole} 简言={name_split}",
            m.len()
        );
    };
    report(
        "jieba",
        lines
            .iter()
            .flat_map(|l| jieba.cut(l, false))
            .map(|t| t.word)
            .collect(),
    );
    report(
        "jieba+hmm",
        lines
            .iter()
            .flat_map(|l| jieba.cut(l, true))
            .map(|t| t.word)
            .collect(),
    );
    report(
        "ltp",
        lines.iter().flat_map(|l| ltp.predict(l).unwrap()).collect(),
    );
    Ok(())
}

mod anyhow_free {
    pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
}
