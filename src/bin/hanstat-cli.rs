//! Command-line front-end.
//!
//! hanstat-cli <file> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N]
//!             [--include-all] [--exclude N,M,..] [--chars-csv out.csv] [--words-csv out.csv] [--suggest N] [--auto-dict]

use hanstat::analyze::{self, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Knowledge, KnowledgeSummary};
use hanstat::extract::{self, EncodingChoice};
use hanstat::segment::SegmenterKind;
use std::path::PathBuf;

#[derive(Default)]
struct AnkiArgs {
    on: bool,
    list: bool,
    save: bool,
    collection: Option<PathBuf>,
    decks: Vec<String>,
    /// "Note type:Field"
    fields: Vec<String>,
}

/// Open the collection with the saved settings plus command-line overrides.
/// `None` if only listing. Classification happens later, once the book's
/// segmenter is known (sentence fields are segmented like the book).
fn load_anki(a: &AnkiArgs) -> Result<Option<(anki::Collection, anki::Settings)>, String> {
    let (mut s, warn) = anki::Settings::load();
    if let Some(w) = warn {
        eprintln!("warning: {w}");
    }
    if let Some(c) = &a.collection {
        s.collection = Some(c.clone());
    }
    if s.collection.is_none() {
        let p = anki::find_profiles();
        match p.len() {
            0 => {
                return Err(
                    "No Anki collection found; pass --anki-collection path\\to\\collection.anki2"
                        .into(),
                );
            }
            1 => s.collection = Some(p[0].collection.clone()),
            _ => {
                return Err(format!(
                    "Several Anki profiles found; pick one with --anki-collection:\n{}",
                    p.iter()
                        .map(|p| format!("  {}", p.collection.display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
    }
    let path = s.collection.clone().unwrap();
    let col = anki::Collection::open(&path)?;
    let info = col.info.clone();
    s.rebind(&info);
    if !a.decks.is_empty() {
        s.decks.clear();
        for name in &a.decks {
            let d = info
                .decks
                .iter()
                .find(|d| &d.name == name)
                .ok_or_else(|| format!("No deck named {name:?} with cards; see --anki-list"))?;
            s.set_deck(d, true);
        }
    }
    if !a.fields.is_empty() {
        s.note_types.clear();
        for spec in &a.fields {
            let (nt_name, field) = spec
                .rsplit_once(':')
                .ok_or_else(|| format!("--anki-field expects \"Note type:Field\", got {spec:?}"))?;
            let nt = info
                .note_types
                .iter()
                .find(|n| n.name == nt_name)
                .ok_or_else(|| format!("No note type named {nt_name:?}; see --anki-list"))?;
            if !nt.fields.iter().any(|f| f == field) {
                return Err(format!(
                    "Note type {nt_name:?} has no field {field:?}; fields: {}",
                    nt.fields.join(", ")
                ));
            }
            s.set_field(nt, field, true);
        }
    }
    if a.list {
        println!("Collection: {}", path.display());
        println!("Decks (cards incl. subdecks; * = selected):");
        for d in &info.decks {
            println!(
                "  {} {}{}  ({} cards)",
                if s.deck_selected(d) { "*" } else { " " },
                "  ".repeat(d.depth()),
                d.name,
                d.cards
            );
        }
        println!("Note types (notes; * = selected field):");
        for nt in info.note_types.iter().filter(|n| n.notes > 0) {
            let f: Vec<String> = nt
                .fields
                .iter()
                .map(|f| {
                    if s.field_selected(nt, f) {
                        format!("*{f}")
                    } else {
                        f.clone()
                    }
                })
                .collect();
            println!("  {} ({}): {}", nt.name, nt.notes, f.join(", "));
        }
        if !a.on {
            return Ok(None);
        }
    }
    if !s.has_selection() {
        return Err(
            "No Anki decks/fields selected. Choose them in the hanstat window (Anki button), or pass --anki-deck and --anki-field (see --anki-list)."
                .into(),
        );
    }
    s.enabled = true;
    if a.save {
        s.save()?;
        eprintln!(
            "Saved Anki selection to {}",
            anki::Settings::path().display()
        );
    }
    Ok(Some((col, s)))
}

fn anki_knowledge(loaded: &(anki::Collection, anki::Settings), seg: &anki::SegConfig) -> Knowledge {
    loaded.0.knowledge(&loaded.1, seg).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    })
}

fn print_anki_summary(k: &KnowledgeSummary) {
    let row = |b: &ByStatus| {
        format!(
            "mature {}  young {}  learning {}  new {}",
            b.mature, b.young, b.learning, b.new
        )
    };
    println!(
        "Anki: {} notes ({} with no Chinese in the chosen fields){}{}",
        k.notes,
        k.notes_without_chinese,
        if k.suspended_excluded > 0 {
            format!(", {} suspended cards ignored", k.suspended_excluded)
        } else {
            String::new()
        },
        match k.segmenter {
            Some(s) if k.sentences > 0 => {
                format!(
                    ", {} sentences split into words ({})",
                    k.sentences,
                    s.label()
                )
            }
            _ => String::new(),
        }
    );
    println!("  cards       {}", row(&k.cards));
    println!("  words       {}", row(&k.words));
    println!("  characters  {}", row(&k.chars));
}

fn print_coverage(c: &BookCoverage, top: usize) {
    if c.from_anki {
        print_anki_summary(&c.anki);
    }
    if !c.listed.source.is_empty() {
        println!(
            "Known from {}: {} words{}",
            c.listed.source,
            c.listed.words,
            if c.from_anki {
                format!(" ({} not young/mature in Anki)", c.listed.new_words)
            } else {
                String::new()
            }
        );
    }
    // Known = on a young or mature card, or in a known word list.
    for (label, k) in [
        ("Unique characters", &c.unique_chars),
        ("Unique words", &c.unique_words),
    ] {
        println!(
            "{label:<18} known {:>7} of {:>7} ({:.1}%)   not known {:>7}",
            k.known,
            k.total,
            k.percent(),
            k.not_known()
        );
    }
    for (label, k) in [
        ("Running chars", &c.running_chars),
        ("Running words", &c.running_words),
    ] {
        println!(
            "{label:<18} known {:>7} of {:>7} ({:.1}%)   not known {:>7}",
            k.known,
            k.total,
            k.percent(),
            k.not_known()
        );
    }
    println!(
        "Difficulty: {} ({}); running words exclude {} occurrences of names",
        c.difficulty.label(),
        c.difficulty.description(),
        c.name_tokens
    );
    let tw: Vec<String> = c
        .top_unknown_words
        .iter()
        .take(top)
        .map(|(w, n)| format!("{w}:{n}"))
        .collect();
    println!("Top words not known: {}", tw.join(" "));
}

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
    let mut ak = AnkiArgs::default();
    // HSK levels and word lists marked known (None: not used).
    let mut known_lists: Option<hanstat::known::Settings> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--known" => {
                let (s, warn) = hanstat::known::Settings::load();
                if let Some(w) = warn {
                    eprintln!("warning: {w}");
                }
                let k = known_lists.get_or_insert_with(Default::default);
                for b in s.hsk {
                    k.set_band(b, true);
                }
                k.lists.extend(s.lists);
                for w in &s.marked {
                    k.set_marked(w, true);
                }
            }
            "--hsk" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    Some(n @ 1..=9) => n.min(7),
                    _ => {
                        eprintln!("--hsk expects a level from 1 to 9 (7, 8 and 9 share one list)");
                        std::process::exit(2)
                    }
                };
                let k = known_lists.get_or_insert_with(Default::default);
                for b in 1..=n {
                    k.set_band(b, true);
                }
            }
            "--known-list" => {
                let p = PathBuf::from(args.next().unwrap_or_default());
                let l = hanstat::known::read_word_list(&p).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1)
                });
                known_lists.get_or_insert_with(Default::default).add_list(l);
            }
            "--anki" => ak.on = true,
            "--anki-list" => ak.list = true,
            "--anki-save" => (ak.on, ak.save) = (true, true),
            "--anki-collection" => {
                ak.on = true;
                ak.collection = args.next().map(PathBuf::from);
            }
            "--anki-deck" => {
                ak.on = true;
                ak.decks.extend(args.next());
            }
            "--anki-field" => {
                ak.on = true;
                ak.fields.extend(args.next());
            }
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
                    "usage: hanstat-cli <file.epub|file.txt> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N] [--include-all] [--exclude i,j] [--chars-csv f] [--words-csv f] [--suggest N] [--auto-dict]\n\
                     \x20      [--anki] [--anki-list] [--anki-collection collection.anki2] [--anki-deck NAME]... [--anki-field \"NOTE TYPE:FIELD\"]... [--anki-save]\n\
                     \x20      [--known] [--hsk N] [--known-list words.txt]...\n\
                     \n\
                     --known adds the HSK levels and word lists marked known in the hanstat window (saved in {}).\n\
                     --hsk N marks HSK 3.0 (2025 syllabus) levels 1 to N known (7, 8, 9: all); --known-list adds a word list (one word per line).\n\
                     These count as known together with young/mature Anki cards, or on their own without --anki.\n\
                     --anki uses the decks/note types/fields chosen in the hanstat window (saved in {}).\n\
                     --anki-deck/--anki-field replace that choice for this run (repeatable; a deck includes its subdecks); --anki-save keeps it.\n\
                     Without a file, --anki prints the collection summary only.",
                    hanstat::known::Settings::path().display(),
                    hanstat::anki::Settings::path().display()
                );
                return;
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let anki_col = if ak.on || ak.list {
        load_anki(&ak).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        })
    } else {
        None
    };
    let words = dict
        .map(|d| {
            hanstat::read_user_dict(&d).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
        })
        .unwrap_or_default();
    let Some(file) = file else {
        if let Some(loaded) = &anki_col {
            let k = anki_knowledge(loaded, &anki::SegConfig::new(kind, None, &words));
            print_anki_summary(&k.summary);
            return;
        }
        if ak.list {
            return;
        }
        eprintln!("no input file; see --help");
        std::process::exit(2);
    };
    let t0 = std::time::Instant::now();
    let book = extract::load(&file, EncodingChoice::Auto).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let t_load = t0.elapsed();
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
    let book_segmenter = prep.segmenter_kind;
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
    let anki_k = anki_col.as_ref().map(|loaded| {
        anki_knowledge(
            loaded,
            &anki::SegConfig::new(kind, Some(book_segmenter), &words),
        )
    });
    let fold = match &anki_col {
        Some((_, s)) => s.match_across_scripts,
        None => anki::Settings::load().0.match_across_scripts,
    };
    let known_k = match &known_lists {
        Some(l) => l.apply(anki_k.as_ref(), fold),
        None => anki_k,
    };
    if let Some(k) = known_k {
        // Names are left out of the difficulty: the user dictionary plus the
        // recommended discovered words, as the GUI does.
        let known: std::collections::HashSet<String> = words.iter().cloned().collect();
        let mut names = known.clone();
        names.extend(
            hanstat::discover::discover(
                &book,
                &included,
                &r.word_freq,
                &known,
                &hanstat::discover::Options::for_size(r.han_total),
            )
            .into_iter()
            .filter(|c| c.recommended)
            .map(|c| c.word),
        );
        r.anki = Some(BookCoverage::compute(
            &k,
            &r.char_freq,
            &r.word_freq,
            &names,
        ));
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
    if let Some(c) = &r.anki {
        print_coverage(c, top);
        println!();
    }
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
