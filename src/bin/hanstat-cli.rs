//! Command-line front-end.
//!
//! hanstat-cli <file> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N]
//!             [--include-all] [--exclude N,M,..] [--chars-csv out.csv] [--words-csv out.csv] [--suggest N] [--auto-dict]

use hanstat::analyze::{self, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Knowledge, KnowledgeSummary};
use hanstat::extract::{self, EncodingChoice};
use hanstat::lang::Language;
use hanstat::segment::SegmenterKind;
use std::path::PathBuf;

/// `--known`, `--hsk`, `--known-list`: applied in the book's language.
#[derive(Default)]
struct KnownArgs {
    saved: bool,
    hsk: Option<u8>,
    lists: Vec<PathBuf>,
}

impl KnownArgs {
    /// The words marked known for `lang`, or `None` if none of the options was given.
    fn resolve(&self, lang: Language) -> Option<hanstat::known::Settings> {
        if !self.saved && self.hsk.is_none() && self.lists.is_empty() {
            return None;
        }
        let mut k = hanstat::known::Settings::default();
        if self.saved {
            let (s, warn) = hanstat::known::Settings::load_for(lang);
            if let Some(w) = warn {
                eprintln!("warning: {w}");
            }
            for b in s.hsk {
                k.set_band(b, true);
            }
            k.lists.extend(s.lists);
            for w in &s.marked {
                k.set_marked(w, true);
            }
        }
        match (self.hsk, lang) {
            (Some(n), Language::Chinese) => (1..=n).for_each(|b| k.set_band(b, true)),
            (Some(_), Language::Korean) => eprintln!("--hsk ignored: the book is Korean"),
            (None, _) => {}
        }
        for p in &self.lists {
            let l = hanstat::known::read_word_list_in(p, lang).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
            k.add_list(l);
        }
        Some(k)
    }
}

#[derive(Default)]
struct AnkiArgs {
    on: bool,
    list: bool,
    save: bool,
    collection: Option<PathBuf>,
    decks: Vec<String>,
    /// "Note type:Field"
    fields: Vec<String>,
    /// Mature at this FSRS stability in days; 0 = Anki's interval rule.
    mature_stability: Option<u32>,
}

/// Open the collection with `lang`'s saved settings plus command-line overrides.
/// `None` if only listing. Classification happens later, once the book's
/// segmenter is known (sentence fields are segmented like the book).
fn load_anki(
    a: &AnkiArgs,
    lang: Language,
) -> Result<Option<(anki::Collection, anki::Settings)>, String> {
    let (mut s, warn) = anki::Settings::load_for(lang);
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
    if let Some(d) = a.mature_stability {
        s.mature_by_stability = d > 0;
        if d > 0 {
            s.mature_stability_days = d;
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
            anki::Settings::path_for(lang).display()
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

fn print_anki_summary(k: &KnowledgeSummary, lang: Language) {
    let row = |b: &ByStatus| {
        format!(
            "mature {}  young {}  learning {}  new {}",
            b.mature, b.young, b.learning, b.new
        )
    };
    println!(
        "Anki: {} notes ({} with no {} in the chosen fields){}{}{}",
        k.notes,
        k.notes_without_chinese,
        lang.name(),
        if k.suspended_excluded > 0 {
            format!(", {} suspended cards ignored", k.suspended_excluded)
        } else {
            String::new()
        },
        match k.mature_stability_days {
            None => format!(", mature = interval {}+ days", anki::MATURE_IVL),
            Some(t) if k.no_stability > 0 => format!(
                ", mature = FSRS stability {t}+ days ({} review cards without FSRS data use interval {}+ days)",
                k.no_stability,
                anki::MATURE_IVL
            ),
            Some(t) => format!(", mature = FSRS stability {t}+ days"),
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
    if lang == Language::Chinese {
        println!("  characters  {}", row(&k.chars));
    }
}

fn print_coverage(c: &BookCoverage, top: usize, lang: Language) {
    if c.from_anki {
        print_anki_summary(&c.anki, lang);
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
    // Known = on a young or mature card, or in a known word list. Korean has words only.
    let chinese = lang == Language::Chinese;
    let unique: &[(&str, &anki::KnownCount)] = if chinese {
        &[
            ("Unique characters", &c.unique_chars),
            ("Unique words", &c.unique_words),
        ]
    } else {
        &[("Unique words", &c.unique_words)]
    };
    for (label, k) in unique {
        println!(
            "{label:<18} known {:>7} of {:>7} ({:.1}%)   not known {:>7}",
            k.known,
            k.total,
            k.percent(),
            k.not_known()
        );
    }
    let running: &[(&str, &anki::KnownCount)] = if chinese {
        &[
            ("Running chars", &c.running_chars),
            ("Running words", &c.running_words),
        ]
    } else {
        &[("Running words", &c.running_words)]
    };
    for (label, k) in running {
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
    // Known words, resolved once the book's language is known.
    let mut ka = KnownArgs::default();
    // Language without a file (for --anki alone); a book's language is detected.
    let mut lang_arg: Option<Language> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--known" => ka.saved = true,
            "--hsk" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    Some(n @ 1..=9) => n.min(7),
                    _ => {
                        eprintln!("--hsk expects a level from 1 to 9 (7, 8 and 9 share one list)");
                        std::process::exit(2)
                    }
                };
                ka.hsk = Some(n);
            }
            "--known-list" => ka
                .lists
                .push(PathBuf::from(args.next().unwrap_or_default())),
            "--lang" => {
                lang_arg = match args.next().as_deref() {
                    Some("zh") => Some(Language::Chinese),
                    Some("ko") => Some(Language::Korean),
                    _ => {
                        eprintln!("--lang expects zh or ko");
                        std::process::exit(2)
                    }
                }
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
            "--anki-mature-stability" => {
                ak.on = true;
                let v = args.next().unwrap_or_default();
                ak.mature_stability = Some(v.parse().unwrap_or_else(|_| {
                    eprintln!("--anki-mature-stability expects a number of days (0 = interval rule), got {v:?}");
                    std::process::exit(2)
                }));
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
                     \x20      [--anki] [--anki-list] [--anki-collection collection.anki2] [--anki-deck NAME]... [--anki-field \"NOTE TYPE:FIELD\"]... [--anki-mature-stability DAYS] [--anki-save]\n\
                     \x20      [--known] [--hsk N] [--known-list words.txt]... [--lang zh|ko]\n\
                     \n\
                     The book's language is detected: Korean books are analysed with Kiwi (downloaded on first use) and have word statistics only.\n\
                     --known adds the HSK levels and word lists marked known in the hanstat window (saved in {} for Chinese, {} for Korean).\n\
                     --hsk N marks HSK 3.0 (2025 syllabus) levels 1 to N known (7, 8, 9: all; Chinese only); --known-list adds a word list (one word per line).\n\
                     These count as known together with young/mature Anki cards, or on their own without --anki.\n\
                     --anki uses the decks/note types/fields chosen in the hanstat window for the book's language (saved in {} / {}).\n\
                     --anki-deck/--anki-field replace that choice for this run (repeatable; a deck includes its subdecks); --anki-save keeps it.\n\
                     --anki-mature-stability DAYS counts a review card as mature once its FSRS stability reaches DAYS instead of at an interval of 21+ days (0: interval rule).\n\
                     Without a file, --anki prints the collection summary only, for --lang (default zh).",
                    hanstat::known::Settings::path_for(Language::Chinese).display(),
                    hanstat::known::Settings::path_for(Language::Korean).display(),
                    hanstat::anki::Settings::path_for(Language::Chinese).display(),
                    hanstat::anki::Settings::path_for(Language::Korean).display()
                );
                return;
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let open_anki = |lang: Language| {
        if ak.on || ak.list {
            load_anki(&ak, lang).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
        } else {
            None
        }
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
        let lang = lang_arg.unwrap_or_default();
        if let Some(loaded) = &open_anki(lang) {
            let k = anki_knowledge(
                loaded,
                &anki::SegConfig::for_language(lang, kind, None, &words),
            );
            print_anki_summary(&k.summary, lang);
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
    let lang = analyze::detect_language(&book);
    let anki_col = open_anki(lang);
    let known_lists = ka.resolve(lang);
    // Word lists and marked words also join words the segmenter split.
    let seg_known: Vec<String> = known_lists
        .as_ref()
        .map(|k| k.segmenter_words())
        .unwrap_or_default();
    let (tr, si) = analyze::detect_variant(&book);
    if let Some(m) =
        hanstat::segment::model_for(kind.resolve_in(lang, hanstat::script::classify(tr, si)))
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
    let prep =
        analyze::analyze_book(&book, kind, &words, &seg_known, &|_, _| {}).unwrap_or_else(|e| {
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
        let prep = analyze::analyze_book(&book, kind, &words, &seg_known, &|_, _| {})
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
        r = analyze::build_report(&book, &prep, &included);
    }
    let anki_k = anki_col.as_ref().map(|loaded| {
        anki_knowledge(
            loaded,
            &anki::SegConfig::for_language(lang, kind, Some(book_segmenter), &words),
        )
    });
    let fold = match &anki_col {
        Some((_, s)) => s.match_across_scripts,
        None => anki::Settings::load_for(lang).0.match_across_scripts,
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
        if lang == Language::Korean {
            eprintln!("--chars-csv: a Korean book has word statistics only; not written");
        } else {
            std::fs::write(p, analyze::freq_csv(&r.char_freq, "character")).expect("write csv");
        }
    }
    if let Some(p) = words_csv {
        std::fs::write(p, analyze::freq_csv(&r.word_freq, "word")).expect("write csv");
    }
    if let Some(n) = suggest {
        if lang == Language::Korean {
            eprintln!("--suggest: word discovery is for Chinese books only");
            return;
        }
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
    if r.language == Language::Korean {
        print_text_korean(r, top);
        return;
    }
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
        print_coverage(c, top, Language::Chinese);
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
    print_notes(r);
}

/// Sections overflow line, then the notes.
fn print_notes(r: &Report) {
    if r.sections.len() > 12 {
        println!("  ... {} more", r.sections.len() - 12);
    }
    println!();
    for n in &r.notes {
        println!("note: {n}");
    }
}

/// A Korean book: word statistics only.
fn print_text_korean(r: &Report, top: usize) {
    println!("Language: Korean (words in dictionary form)");
    println!();
    println!(
        "Korean words        total {:>10}   unique {:>7}   once-only {:>6}   [{}]",
        r.words_total,
        r.words_unique,
        r.words_hapax,
        r.segmenter.label()
    );
    println!(
        "Non-Korean words    total {:>10}   unique {:>7}   numbers {}",
        r.foreign_words_total, r.foreign_words_unique, r.numbers
    );
    println!(
        "Paragraphs {}   sentences {}   avg sentence {:.1} words   avg paragraph {:.1} words",
        r.paragraphs,
        r.sentences,
        r.avg_sentence_words.unwrap_or(0.0),
        r.avg_paragraph_words.unwrap_or(0.0)
    );
    println!();
    println!("Coverage   words");
    for w in &r.word_coverage {
        println!("{:>6}% {:>7}", w.percent, w.items);
    }
    println!();
    let tw: Vec<String> = r
        .word_freq
        .iter()
        .take(top)
        .map(|(w, n)| format!("{w}:{n}"))
        .collect();
    println!("Top words: {}", tw.join(" "));
    println!();
    if let Some(c) = &r.anki {
        print_coverage(c, top, Language::Korean);
        println!();
    }
    println!("Sections ({}):", r.sections.len());
    for (i, s) in r.sections.iter().enumerate().take(12) {
        println!(
            "  [{i:>3}] {} {:<30} words {:>7} uniq {:>5} new {:>5}",
            if s.included { "+" } else { "-" },
            s.label.chars().take(30).collect::<String>(),
            s.words,
            s.unique_words,
            s.new_words.map(|n| n.to_string()).unwrap_or("-".into()),
        );
    }
    print_notes(r);
}
