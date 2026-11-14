//! Command-line front-end.
//!
//! hanstat-cli <file> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N]
//!             [--include-all] [--exclude N,M,..] [--chars-csv out.csv] [--words-csv out.csv] [--suggest N] [--auto-dict]

use hanstat::analyze::{self, Report};
use hanstat::extract::{self, EncodingChoice};
use hanstat::segment::SegmenterKind;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut file = None;
    let mut kind = SegmenterKind::Auto;
    let mut dict = None;
    let mut json = false;
    let mut top = 20usize;
    let mut include_all = false;
    let mut exclude: Vec<usize> = Vec::new();
    let mut chars_csv = None;
    let mut words_csv = None;
    let mut suggest: Option<usize> = None;
    let mut auto_dict = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--jieba" => kind = SegmenterKind::Jieba,
            "--ltp" => kind = SegmenterKind::Ltp,
            "--ckip" => kind = SegmenterKind::Ckip,
            "--dict" => dict = args.next().map(PathBuf::from),
            "--json" => json = true,
            "--top" => top = args.next().and_then(|s| s.parse().ok()).unwrap_or(20),
            "--include-all" => include_all = true,
            "--exclude" => {
                exclude = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect()
            }
            "--chars-csv" => chars_csv = args.next().map(PathBuf::from),
            "--words-csv" => words_csv = args.next().map(PathBuf::from),
            "--auto-dict" => auto_dict = true,
            "--suggest" => suggest = Some(args.next().and_then(|s| s.parse().ok()).unwrap_or(40)),
            "-h" | "--help" => {
                eprintln!(
                    "usage: hanstat-cli <file.epub|file.txt> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N] [--include-all] [--exclude i,j] [--chars-csv f] [--words-csv f] [--suggest N] [--auto-dict]"
                );
                return;
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let Some(file) = file else {
        eprintln!("no input file; see --help");
        std::process::exit(2);
    };
    let t0 = std::time::Instant::now();
    let book = extract::load(&file, EncodingChoice::Auto).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let t_load = t0.elapsed();
    let words = dict
        .map(|d| {
            hanstat::read_user_dict(&d).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
        })
        .unwrap_or_default();
    let (tr, si) = analyze::detect_variant(&book);
    if let Some(m) = hanstat::segment::model_for(kind.resolve(hanstat::script::classify(tr, si)))
        && !m.is_cached()
    {
        eprintln!(
            "Downloading {} ({:.0} MB, first use only) to {} ...",
            m.title,
            m.total_size() as f64 / 1e6,
            m.dir().display()
        );
        if let Err(e) = m.ensure(&|_, _| {}) {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    let prep = analyze::analyze_book(&book, kind, &words, &|_, _| {}).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let included: Vec<bool> = book
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| !exclude.contains(&i) && (include_all || s.default_included()))
        .collect();
    let mut words = words;
    let mut r = analyze::build_report(&book, &prep, &included);
    if auto_dict {
        let known: std::collections::HashSet<String> = words.iter().cloned().collect();
        let found = hanstat::discover::discover(
            &book,
            &included,
            &r.word_freq,
            &known,
            &hanstat::discover::Options::for_size(r.han_total),
        );
        let rec: Vec<String> = found
            .into_iter()
            .filter(|c| c.recommended)
            .map(|c| c.word)
            .collect();
        eprintln!(
            "auto-dict: forcing {} discovered words: {}",
            rec.len(),
            rec.join(" ")
        );
        words.extend(rec);
        let prep = analyze::analyze_book(&book, kind, &words, &|_, _| {}).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
        r = analyze::build_report(&book, &prep, &included);
    }
    eprintln!("load {:?}, total {:?}", t_load, t0.elapsed());

    if let Some(p) = chars_csv {
        std::fs::write(p, analyze::freq_csv(&r.char_freq, "character")).expect("write csv");
    }
    if let Some(p) = words_csv {
        std::fs::write(p, analyze::freq_csv(&r.word_freq, "word")).expect("write csv");
    }
    if let Some(n) = suggest {
        let t = std::time::Instant::now();
        let known: std::collections::HashSet<String> = words.iter().cloned().collect();
        let c = hanstat::discover::discover(
            &book,
            &included,
            &r.word_freq,
            &known,
            &hanstat::discover::Options::for_size(r.han_total),
        );
        eprintln!("discovery {:?}, {} candidates", t.elapsed(), c.len());
        println!(
            "{:3} {:10} {:>7} {:>6} {:>8} {:>5} {:>5} {:>5} dict",
            "rec", "word", "occur", "whole%", "cohesion", "dep", "H_l", "H_r"
        );
        for c in c.iter().take(n) {
            println!(
                "{:3} {:10} {:>7} {:>5.0}% {:>8.0} {:>5.2} {:>5.2} {:>5.2} {}",
                if c.recommended { "*" } else { "" },
                c.word,
                c.occurrences,
                c.kept_whole as f64 * 100.0 / c.occurrences as f64,
                c.cohesion,
                c.dependency,
                c.left_entropy,
                c.right_entropy,
                if c.in_dictionary { "yes" } else { "" }
            );
        }
        return;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    } else {
        print_text(&r, top);
    }
}

fn print_text(r: &Report, top: usize) {
    println!(
        "Title: {}  [{}]",
        r.title.as_deref().unwrap_or("-"),
        r.format
    );
    println!(
        "Script: {:?} (trad-only chars {}, simp-only chars {})",
        r.variant, r.trad_marks, r.simp_marks
    );
    println!();
    println!(
        "Chinese characters  total {:>10}   unique {:>7}   once-only {:>6}",
        r.han_total, r.han_unique, r.han_hapax
    );
    println!(
        "Chinese words       total {:>10}   unique {:>7}   once-only {:>6}   [{}]",
        r.words_total,
        r.words_unique,
        r.words_hapax,
        r.segmenter.label()
    );
    println!("All non-space chars       {:>10}", r.nonspace_total);
    println!(
        "  punctuation {}  letters {}  digits {}  symbols {}  private-use {}  U+FFFD {}  other {}",
        r.punctuation, r.letters, r.digits, r.symbols, r.private_use, r.replacement, r.other
    );
    println!(
        "Non-Chinese words   total {:>10}   unique {:>7}   numbers {}",
        r.foreign_words_total, r.foreign_words_unique, r.numbers
    );
    println!(
        "Paragraphs {}   sentences {}   avg sentence {:.1} chars   avg paragraph {:.1} chars   avg word {:.2} chars",
        r.paragraphs, r.sentences, r.avg_sentence_han, r.avg_paragraph_han, r.avg_word_len
    );
    let wl = r.word_length_tokens;
    let wt = r.words_total.max(1) as f64;
    println!(
        "Word length (tokens): 1:{:.1}%  2:{:.1}%  3:{:.1}%  4:{:.1}%  5+:{:.1}%",
        wl[0] as f64 * 100.0 / wt,
        wl[1] as f64 * 100.0 / wt,
        wl[2] as f64 * 100.0 / wt,
        wl[3] as f64 * 100.0 / wt,
        wl[4] as f64 * 100.0 / wt
    );
    println!();
    println!("Coverage   chars   words");
    for (c, w) in r.char_coverage.iter().zip(&r.word_coverage) {
        println!("{:>6}% {:>7} {:>7}", c.percent, c.items, w.items);
    }
    println!();
    let tc: Vec<String> = r
        .char_freq
        .iter()
        .take(top)
        .map(|(c, n)| format!("{c}{n}"))
        .collect();
    println!("Top chars: {}", tc.join(" "));
    let tw: Vec<String> = r
        .word_freq
        .iter()
        .take(top)
        .map(|(w, n)| format!("{w}:{n}"))
        .collect();
    println!("Top words: {}", tw.join(" "));
    println!();
    println!("Sections ({}):", r.sections.len());
    for (i, s) in r.sections.iter().enumerate().take(12) {
        println!(
            "  [{i:>3}] {} {:<30} han {:>7} uniq {:>5} new {:>5}  words {:>7}",
            if s.included { "+" } else { "-" },
            s.label.chars().take(30).collect::<String>(),
            s.han,
            s.unique_han,
            s.new_han.map(|n| n.to_string()).unwrap_or("-".into()),
            s.words
        );
    }
    if r.sections.len() > 12 {
        println!("  ... {} more", r.sections.len() - 12);
    }
    println!();
    for n in &r.notes {
        println!("note: {n}");
    }
}
