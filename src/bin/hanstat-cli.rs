//! Command-line front-end.
//!
//! hanstat-cli <file> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N]
//!             [--include-all] [--exclude N,M,..] [--chars-csv out.csv] [--words-csv out.csv] [--suggest N] [--auto-dict] [--history]
//! hanstat-cli --plan [--lang zh|ko|ja] [--target P | --learn N] [--incidental M] [--lookahead X] [--width W] [--study-lists DIR] [--json]

use hanstat::analyze::{self, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Knowledge, KnowledgeSummary};
use hanstat::extract::{self, EncodingChoice};
use hanstat::history::{self, History};
use hanstat::lang::Language;
use hanstat::plan;
use hanstat::segment::SegmenterKind;
use std::path::PathBuf;

/// `--known`, `--level`/`--hsk`, `--known-list`, `--kaishi`: applied in the book's language.
#[derive(Default)]
struct KnownArgs {
    saved: bool,
    /// `--level N`: levels 1 to N in the book's language (Japanese: N5 to N`N`).
    level: Option<u8>,
    /// `--hsk N`: Chinese only.
    hsk: Option<u8>,
    /// `--tocfl N`: TOCFL levels 1 to N (Novice 1 to C1), Chinese only.
    tocfl: Option<u8>,
    lists: Vec<PathBuf>,
    /// `--kaishi`: Japanese only.
    kaishi: bool,
}

impl KnownArgs {
    /// The words marked known for `lang`, or `None` if none of the options was given.
    fn resolve(&self, lang: Language) -> Option<hanstat::known::Settings> {
        if !self.saved
            && self.level.is_none()
            && self.hsk.is_none()
            && self.tocfl.is_none()
            && self.lists.is_empty()
            && !self.kaishi
        {
            return None;
        }
        let mut k = hanstat::known::Settings {
            language: lang,
            ..Default::default()
        };
        if self.saved {
            let (s, warn) = hanstat::known::Settings::load_for(lang);
            if let Some(w) = warn {
                eprintln!("warning: {w}");
            }
            for b in s.levels {
                k.set_band(b, true);
            }
            k.lists.extend(s.lists);
            for w in &s.marked {
                k.set_marked(w, true);
            }
            k.kaishi = s.kaishi;
            k.tocfl = s.tocfl;
        }
        if self.kaishi {
            if lang == Language::Japanese {
                k.kaishi = true;
            } else {
                eprintln!("--kaishi ignored: the book is {}", lang.name());
            }
        }
        match (self.hsk, lang) {
            (Some(n), Language::Chinese) => (1..=n).for_each(|b| k.set_band(b, true)),
            (Some(_), lang) => {
                eprintln!("--hsk ignored: the book is {} (use --level)", lang.name())
            }
            (None, _) => {}
        }
        match (self.tocfl, lang) {
            (Some(n), Language::Chinese) => (1..=n).for_each(|b| k.set_tocfl(b, true)),
            (Some(_), lang) => eprintln!("--tocfl ignored: the book is {}", lang.name()),
            (None, _) => {}
        }
        if let Some(n) = self.level {
            let bands = hanstat::known::bands(lang);
            if lang == Language::Japanese {
                // JLPT counts down: --level 3 is N5, N4 and N3.
                if n > 5 {
                    eprintln!("--level {n}: JLPT levels are N5 to N1; marking N5 only");
                }
                let n = n.min(5);
                for &b in bands.iter().filter(|&&b| b >= n) {
                    k.set_band(b, true);
                }
            } else {
                let top = *bands.last().unwrap();
                if lang == Language::Korean && n > top {
                    eprintln!("--level {n}: Korean levels go up to {top}급; marking 1–{top}급");
                }
                for &b in bands.iter().filter(|&&b| b <= n.min(top)) {
                    k.set_band(b, true);
                }
            }
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
    match lang {
        Language::Chinese => println!("  characters  {}", row(&k.chars)),
        Language::Japanese => println!("  kanji       {}", row(&k.chars)),
        Language::Korean => {}
    }
}

fn print_coverage(c: &BookCoverage, top: usize, r: &Report) {
    let lang = r.language;
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
    // Known = on a young or mature card, or in a known word list. Korean has
    // words only; Japanese characters are kanji.
    let unique: &[(&str, &anki::KnownCount)] = match lang {
        Language::Chinese => &[
            ("Unique characters", &c.unique_chars),
            ("Unique words", &c.unique_words),
        ],
        Language::Japanese => &[
            ("Unique kanji", &c.unique_chars),
            ("Unique words", &c.unique_words),
        ],
        Language::Korean => &[("Unique words", &c.unique_words)],
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
    let running: &[(&str, &anki::KnownCount)] = match lang {
        Language::Chinese => &[
            ("Running chars", &c.running_chars),
            ("Running words", &c.running_words),
        ],
        Language::Japanese => &[
            ("Running kanji", &c.running_chars),
            ("Running words", &c.running_words),
        ],
        Language::Korean => &[("Running words", &c.running_words)],
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
        .map(|(w, n)| format!("{}:{n}", r.display(w)))
        .collect();
    println!("Top words not known: {}", tw.join(" "));
}

/// `--plan` and its options.
struct PlanArgs {
    on: bool,
    target: Option<f64>,
    learn: Option<usize>,
    incidental: Option<u32>,
    lookahead: f32,
    objective: plan::Objective,
    width: Option<usize>,
    study_lists: Option<PathBuf>,
}

impl Default for PlanArgs {
    fn default() -> Self {
        PlanArgs {
            on: false,
            target: None,
            learn: None,
            incidental: None,
            lookahead: plan::DEFAULT_LOOKAHEAD,
            objective: plan::Objective::default(),
            width: None,
            study_lists: None,
        }
    }
}

/// Order the books of `lang` in the History into a reading path, against `k`.
fn run_plan(a: &PlanArgs, lang: Language, k: Option<&Knowledge>, fold: bool, json: bool) {
    let (hist, warn) = History::load();
    if let Some(w) = warn {
        eprintln!("warning: {w}");
    }
    let mut books = Vec::new();
    let mut missing = Vec::new();
    for e in hist.entries.iter().filter(|e| e.stats.language == lang) {
        match hanstat::profile::Profile::load(&hist.profile(&e.id)) {
            Some(p) => books.push((e.title.clone(), p)),
            None => missing.push(e),
        }
    }
    for e in &missing {
        eprintln!(
            "no word data for {} (analysed by an older version); left out: run hanstat-cli \"{}\" --history",
            e.title,
            e.path.display()
        );
    }
    if books.is_empty() {
        eprintln!(
            "No {} books with word data in the history ({}). Add some with hanstat-cli <file> --history.",
            lang.name(),
            History::path().display()
        );
        std::process::exit(1);
    }
    let params = plan::Params {
        study: match (a.target, a.learn) {
            (_, Some(n)) => plan::Study::Fixed(n),
            (Some(t), None) => plan::Study::Target(t),
            (None, None) => plan::Study::Target(95.0),
        },
        objective: a.objective,
        incidental: a.incidental,
        lookahead: a.lookahead,
    };
    let t = std::time::Instant::now();
    let lib = plan::Library::build(lang, fold, books);
    let k0 = lib.known(k);
    let all: Vec<usize> = (0..lib.books.len()).collect();
    let width = a
        .width
        .unwrap_or_else(|| plan::width_for(&lib, &all, plan::SEARCH_BUDGET));
    let path = lib
        .optimise(
            &k0,
            &params,
            &all,
            width,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    let naive = lib.evaluate(&k0, &params, &lib.easiest_now(&k0, &all));
    eprintln!(
        "plan of {} books, beam width {width}: {:?}",
        all.len(),
        t.elapsed()
    );

    if let Some(dir) = &a.study_lists {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| {
            eprintln!("Cannot create {}: {e}", dir.display());
            std::process::exit(1)
        });
        for (i, s) in path.steps.iter().enumerate() {
            let title: String = lib.books[s.book]
                .title
                .chars()
                .map(|c| {
                    if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            let f = dir.join(format!("{:02} {}.txt", i + 1, title.trim()));
            let mut text = format!(
                "# Words to study before {} (step {} of the reading path), with their counts in the book\n",
                lib.books[s.book].title,
                i + 1
            );
            for (w, n) in lib.study_list(s.book, &s.study) {
                text.push_str(&format!("{w}\t{n}\n"));
            }
            std::fs::write(&f, text).unwrap_or_else(|e| {
                eprintln!("Cannot write {}: {e}", f.display());
                std::process::exit(1)
            });
        }
        eprintln!("study lists written to {}", dir.display());
    }

    if json {
        let steps: Vec<serde_json::Value> = path
            .steps
            .iter()
            .map(|s| {
                let b = &lib.books[s.book];
                serde_json::json!({
                    "id": b.id,
                    "title": b.title,
                    "segmenter": b.segmenter,
                    "known_now": s.now,
                    "known_when_reached": s.reached,
                    "known_while_reading": s.reading,
                    "study": lib.study_list(s.book, &s.study),
                    "study_if_read_now": s.study_now,
                    "words_from_earlier_books": s.gained,
                    "learned_by_reading": s.by_reading,
                })
            })
            .collect();
        let out = serde_json::json!({
            "language": lang,
            "study": match params.study {
                plan::Study::Target(t) => serde_json::json!({"target": t}),
                plan::Study::Fixed(n) => serde_json::json!({"learn": n}),
            },
            "lookahead": params.lookahead,
            "incidental": params.incidental,
            "known_words": k0.len(),
            "library_words": lib.vocab_len(),
            "total_study": path.total_study(),
            "largest_step": path.peak(),
            "lowest_reading": path.lowest_reading(),
            "easiest_now_order": {
                "total_study": naive.total_study(),
                "largest_step": naive.peak(),
                "lowest_reading": naive.lowest_reading(),
            },
            "steps": steps,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }
    println!(
        "Reading path: {} {} books, {}{}{}",
        lib.books.len(),
        lang.name(),
        match params.study {
            plan::Study::Target(t) => format!("studying until {t}% of running words are known"),
            plan::Study::Fixed(n) => format!("studying {n} words before each"),
        },
        match params.incidental {
            Some(m) => format!(", words read {m}+ times learned"),
            None => String::new(),
        },
        if params.lookahead > 0.0 {
            format!(", lookahead {}", params.lookahead)
        } else {
            String::new()
        }
    );
    println!(
        "Known now: {} of the books' {} distinct words{}",
        k0.len(),
        lib.vocab_len(),
        k.map(|k| match (k.from_anki, k.listed.source.is_empty()) {
            (true, true) => " (Anki)".to_owned(),
            (true, false) => format!(" (Anki + {})", k.listed.source),
            (false, _) => format!(" ({})", k.listed.source),
        })
        .unwrap_or_default()
    );
    let segs = lib.segmenters(&all);
    if segs.len() > 1 {
        println!(
            "note: the books were segmented with {}; words shared across them are undercounted",
            segs.join(", ")
        );
    }
    println!();
    let fixed = matches!(params.study, plan::Study::Fixed(_));
    println!(
        "{:>3}  {:>6}  {:>7}  {:>6}  {:>8}  {:>6}  {:>7}  title",
        "#",
        "now",
        "reached",
        "study",
        if fixed { "" } else { "if first" },
        "gained",
        "reading"
    );
    for (i, s) in path.steps.iter().enumerate() {
        println!(
            "{:>3}  {:>5.1}%  {:>6.1}%  {:>6}  {:>8}  {:>6}  {:>6.1}%  {}",
            i + 1,
            s.now,
            s.reached,
            s.study.len(),
            s.study_now.map(|n| n.to_string()).unwrap_or_default(),
            s.gained,
            s.reading,
            lib.books[s.book].title
        );
    }
    println!();
    let summary = |p: &plan::Path| {
        if fixed {
            format!(
                "lowest coverage while reading {:.1}%, {} words studied",
                p.lowest_reading(),
                p.total_study()
            )
        } else {
            format!(
                "{} words to study in all, largest step {}",
                p.total_study(),
                p.peak()
            )
        }
    };
    println!("This order:           {}", summary(&path));
    println!("Easiest-now order:    {}", summary(&naive));
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
    let mut no_cache = false;
    let mut add_history = false;
    let mut pa = PlanArgs::default();
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
            "--level" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    // Chinese 7, 8, 9 share one list (band 7).
                    Some(n @ 1..=9) => n.min(7),
                    _ => {
                        eprintln!(
                            "--level expects a level from 1 to 9 (HSK), 1 to 6 (Korean 급) or 1 to 5 (JLPT N5–N1: 3 is N5–N3)"
                        );
                        std::process::exit(2)
                    }
                };
                ka.level = Some(n);
            }
            "--tocfl" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    Some(n @ 1..=7) => n,
                    _ => {
                        eprintln!(
                            "--tocfl expects 1 to 7: 1–2 are Novice 1–2, 3–7 are Level 1–5 (--tocfl 4 is Novice 1 to Level 2; Level 6 has no list)"
                        );
                        std::process::exit(2)
                    }
                };
                ka.tocfl = Some(n);
            }
            "--kaishi" => ka.kaishi = true,
            "--known-list" => ka
                .lists
                .push(PathBuf::from(args.next().unwrap_or_default())),
            "--lang" => {
                lang_arg = match args.next().as_deref() {
                    Some("zh") => Some(Language::Chinese),
                    Some("ko") => Some(Language::Korean),
                    Some("ja") => Some(Language::Japanese),
                    _ => {
                        eprintln!("--lang expects zh, ko or ja");
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
            "--no-cache" => no_cache = true,
            "--suggest" => suggest = Some(args.next().and_then(|s| s.parse().ok()).unwrap_or(40)),
            "--history" => add_history = true,
            "--plan" => pa.on = true,
            "--target" => {
                pa.on = true;
                pa.target = Some(num(args.next(), "--target expects a percentage, e.g. 95"));
                if !(0.0..=100.0).contains(&pa.target.unwrap()) {
                    eprintln!("--target expects a percentage from 0 to 100");
                    std::process::exit(2)
                }
            }
            "--learn" => {
                pa.on = true;
                pa.learn = Some(num(args.next(), "--learn expects a number of words"));
            }
            "--incidental" => {
                pa.on = true;
                pa.incidental = Some(num(args.next(), "--incidental expects a number of times"));
            }
            "--lookahead" => {
                pa.on = true;
                pa.lookahead = num(args.next(), "--lookahead expects a weight, e.g. 0.25");
            }
            "--objective" => {
                pa.on = true;
                pa.objective = match args.next().as_deref() {
                    Some("total") => plan::Objective::Total,
                    Some("peak") => plan::Objective::Peak,
                    _ => {
                        eprintln!("--objective expects total or peak");
                        std::process::exit(2)
                    }
                };
            }
            "--width" => {
                pa.on = true;
                pa.width = Some(num(args.next(), "--width expects a beam width"));
            }
            "--study-lists" => {
                pa.on = true;
                pa.study_lists = args.next().map(PathBuf::from);
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: hanstat-cli <file.epub|file.txt> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N] [--include-all] [--exclude i,j] [--chars-csv f] [--words-csv f] [--suggest N] [--auto-dict] [--no-cache] [--history]\n\
                     \x20      hanstat-cli --plan [--lang zh|ko|ja] [--target P | --learn N] [--incidental M] [--lookahead X] [--width W] [--study-lists DIR] [--json]\n\
                     \x20      [--anki] [--anki-list] [--anki-collection collection.anki2] [--anki-deck NAME]... [--anki-field \"NOTE TYPE:FIELD\"]... [--anki-mature-stability DAYS] [--anki-save]\n\
                     \x20      [--known] [--level N] [--hsk N] [--tocfl N] [--kaishi] [--known-list words.txt]... [--lang zh|ko|ja]\n\
                     \n\
                     The book's language is detected: Korean books are analysed with Kiwi and have word statistics only;\n\
                     \x20 Japanese books are analysed with Sudachi and have kanji and word statistics (both downloaded on first use).\n\
                     The word counts of a book are saved in {} and read back when the same text is analysed again with the same\n\
                     \x20 segmenter, model, --dict and known words by the same analysis code; --no-cache (or HANSTAT_NO_CACHE) analyses afresh and saves nothing.\n\
                     --known adds the levels and word lists marked known in the hanstat window (saved in {} for Chinese, {} for Korean, {} for Japanese).\n\
                     --level N marks levels 1 to N known in the book's language: HSK 3.0 (2025 syllabus; 7, 8, 9: all) for Chinese,\n\
                     \x20 1–6급 of 국립국어원's 국제 통용 한국어 표준 교육과정 vocabulary (the list TOPIK levels follow) for Korean,\n\
                     \x20 JLPT N5 down to N`N` for Japanese (Jonathan Waller's lists; --level 3 is N5–N3).\n\
                     --tocfl N marks TOCFL levels 1 to N known in Chinese books (SC-TOP's 華語八千詞表, 2024: 1–2 are Novice 1–2, 3–7 are Level 1–5).\n\
                     --hsk N is --level N for Chinese books only; --kaishi marks the words of the Kaishi 1.5k deck known (Japanese books only);\n\
                     \x20 --known-list adds a word list (one word per line).\n\
                     These count as known together with young/mature Anki cards, or on their own without --anki.\n\
                     --anki uses the decks/note types/fields chosen in the hanstat window for the book's language (saved in {} / {}).\n\
                     --anki-deck/--anki-field replace that choice for this run (repeatable; a deck includes its subdecks); --anki-save keeps it.\n\
                     --anki-mature-stability DAYS counts a review card as mature once its FSRS stability reaches DAYS instead of at an interval of 21+ days (0: interval rule).\n\
                     Without a file, --anki prints the collection summary only, for --lang (default zh).\n\
                     \n\
                     --history records the book in the History (and its word data for the reading path), as opening it in the window does.\n\
                     --plan orders the --lang books in the History into a reading path against the known words (--anki, --known, --level, ...):\n\
                     \x20 before each book you study its unknown words until --target P% of its running words are known (default 95), or --learn N of them;\n\
                     \x20 they stay known for the books after it. --incidental M also counts words read M+ times in a book as learned.\n\
                     \x20 --lookahead X weighs the books still to come when choosing study words (0: the book's most frequent; default {}).\n\
                     \x20 --width W sets the search's beam width (1: greedy). --study-lists DIR writes each book's study list (word<TAB>count).",
                    hanstat::cache::Cache::default_dir().display(),
                    hanstat::known::Settings::path_for(Language::Chinese).display(),
                    hanstat::known::Settings::path_for(Language::Korean).display(),
                    hanstat::known::Settings::path_for(Language::Japanese).display(),
                    hanstat::anki::Settings::path_for(Language::Chinese).display(),
                    hanstat::anki::Settings::path_for(Language::Korean).display(),
                    plan::DEFAULT_LOOKAHEAD
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
    if pa.on {
        if file.is_some() {
            eprintln!("--plan works on the History: give no file");
            std::process::exit(2);
        }
        let lang = lang_arg.unwrap_or_default();
        let anki_col = open_anki(lang);
        let anki_k = anki_col.as_ref().map(|loaded| {
            anki_knowledge(
                loaded,
                &anki::SegConfig::for_language(lang, kind, None, &words),
            )
        });
        let across = match &anki_col {
            Some((_, s)) => s.match_across_scripts,
            None => anki::Settings::load_for(lang).0.match_across_scripts,
        };
        let k = match ka.resolve(lang) {
            Some(l) => l.apply(anki_k.as_ref(), across),
            None => anki_k,
        };
        let fold = plan::Library::fold_for(k.as_ref(), lang, across);
        run_plan(&pa, lang, k.as_ref(), fold, json);
        return;
    }
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
    let cache = hanstat::cache::Cache::open().filter(|_| !no_cache);
    let analyse = |words: &[String]| {
        analyze::analyze_book_cached(&book, kind, words, &seg_known, cache.as_ref(), &|_, _| {})
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
    };
    let t_analysis = std::time::Instant::now();
    let prep = analyse(&words);
    let mut analysis = (t_analysis.elapsed(), prep.cached);
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
        let t_analysis = std::time::Instant::now();
        let prep = analyse(&words);
        analysis = (analysis.0 + t_analysis.elapsed(), analysis.1 && prep.cached);
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
    // Names are left out of the difficulty: the user dictionary plus the
    // recommended discovered words, as the GUI does.
    let names: Option<std::collections::HashSet<String>> =
        (known_k.is_some() || add_history).then(|| {
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
            names
        });
    if let (Some(k), Some(names)) = (known_k, &names) {
        r.anki = Some(BookCoverage::compute(
            &k,
            &r.char_freq,
            &r.word_freq,
            names,
            &r.word_parts,
        ));
    }
    if add_history {
        record_history(&file, &book, &r, names.as_ref().unwrap());
    }
    eprintln!(
        "load {:?}, analysis {:?}{}, total {:?}",
        t_load,
        analysis.0,
        if analysis.1 { " (from cache)" } else { "" },
        t0.elapsed()
    );

    if let Some(p) = chars_csv {
        if lang == Language::Korean {
            eprintln!("--chars-csv: a Korean book has word statistics only; not written");
        } else {
            std::fs::write(p, analyze::freq_csv(&r.char_freq, "character")).expect("write csv");
        }
    }
    if let Some(p) = words_csv {
        // Japanese word keys are written as the book spells them.
        let shown: Vec<(&str, u32)> = r
            .word_freq
            .iter()
            .map(|(w, n)| (r.display(w), *n))
            .collect();
        std::fs::write(p, analyze::freq_csv(&shown, "word")).expect("write csv");
    }
    if let Some(n) = suggest {
        if lang != Language::Chinese {
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

/// `--history`: the book's History entry, cover and word data, as the window saves them.
fn record_history(
    file: &std::path::Path,
    book: &extract::Book,
    r: &Report,
    names: &std::collections::HashSet<String>,
) {
    fn fail(e: String) -> ! {
        eprintln!("{e}");
        std::process::exit(1)
    }
    let id = history::file_id(file)
        .unwrap_or_else(|e| fail(format!("Cannot read {}: {e}", file.display())));
    let (mut hist, warn) = History::load();
    if let Some(w) = warn {
        fail(w);
    }
    if let Some(cover) = &book.cover {
        let dest = hist.cover(&id);
        if !dest.is_file() {
            let _ = history::save_cover(cover, &dest);
        }
    }
    let entry = history::Entry {
        id: id.clone(),
        path: std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf()),
        title: r
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                file.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
        analysed: history::now(),
        stats: history::Stats::from_report(r),
    };
    if let Err(e) = hist.record(entry, true) {
        fail(e);
    }
    hanstat::profile::Profile::from_report(&id, r, names)
        .save(&hist.profile(&id))
        .unwrap_or_else(|e| fail(e));
    eprintln!("recorded in {}", History::path().display());
}

/// A number argument, or exit with `msg`.
fn num<T: std::str::FromStr>(v: Option<String>, msg: &str) -> T {
    v.and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        eprintln!("{msg}");
        std::process::exit(2)
    })
}

fn print_text(r: &Report, top: usize) {
    println!(
        "Title: {}  [{}]",
        r.title.as_deref().unwrap_or("-"),
        r.format
    );
    if r.language != Language::Chinese {
        print_text_words(r, top);
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
        print_coverage(c, top, r);
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

/// A Korean book (word statistics only) or a Japanese one (words, plus kanji).
fn print_text_words(r: &Report, top: usize) {
    let japanese = r.language == Language::Japanese;
    let name = r.language.name();
    if japanese {
        println!(
            "Language: Japanese (words in dictionary form, spellings merged; shown as the book spells them)"
        );
    } else {
        println!("Language: Korean (words in dictionary form)");
    }
    println!();
    if japanese {
        println!(
            "Kanji               total {:>10}   unique {:>7}   once-only {:>6}",
            r.han_total, r.han_unique, r.han_hapax
        );
    }
    println!(
        "{:<20}total {:>10}   unique {:>7}   once-only {:>6}   [{}]",
        format!("{name} words"),
        r.words_total,
        r.words_unique,
        r.words_hapax,
        r.segmenter.label()
    );
    println!(
        "{:<20}total {:>10}   unique {:>7}   numbers {}",
        format!("Non-{name} words"),
        r.foreign_words_total,
        r.foreign_words_unique,
        r.numbers
    );
    println!(
        "Paragraphs {}   sentences {}   avg sentence {:.1} words   avg paragraph {:.1} words",
        r.paragraphs,
        r.sentences,
        r.avg_sentence_words.unwrap_or(0.0),
        r.avg_paragraph_words.unwrap_or(0.0)
    );
    println!();
    if japanese {
        println!("Coverage   words   kanji");
        for (i, w) in r.word_coverage.iter().enumerate() {
            let k = r
                .char_coverage
                .get(i)
                .map_or("-".into(), |c| c.items.to_string());
            println!("{:>6}% {:>7} {:>7}", w.percent, w.items, k);
        }
    } else {
        println!("Coverage   words");
        for w in &r.word_coverage {
            println!("{:>6}% {:>7}", w.percent, w.items);
        }
    }
    println!();
    if japanese {
        let tc: Vec<String> = r
            .char_freq
            .iter()
            .take(top)
            .map(|(c, n)| format!("{c}{n}"))
            .collect();
        println!("Top kanji: {}", tc.join(" "));
    }
    let tw: Vec<String> = r
        .word_freq
        .iter()
        .take(top)
        .map(|(w, n)| format!("{}:{n}", r.display(w)))
        .collect();
    println!("Top words: {}", tw.join(" "));
    println!();
    if let Some(c) = &r.anki {
        print_coverage(c, top, r);
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
