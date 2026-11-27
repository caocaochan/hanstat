//! Command-line front-end.
//!
//! hanstat-cli <file> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N]
//!             [--include-all] [--exclude N,M,..] [--chars-csv out.csv] [--words-csv out.csv] [--suggest N] [--auto-dict] [--library]
//! hanstat-cli --plan [--read [--read-percent P] [--min-count M] | --target P | --learn N] [--incidental M] [--lookahead X] [--objective peak|total] [--keep-series] [--width W] [--study-lists DIR] [--json]

use hanstat::analyze::{self, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Knowledge, KnowledgeSummary};
use hanstat::extract::{self, EncodingChoice};
use hanstat::history::{self, History};
use hanstat::plan;
use hanstat::segment::SegmenterKind;
use std::path::PathBuf;

/// `--known`, `--level`/`--hsk`, `--hsk2`, `--tocfl`, `--known-list`.
#[derive(Default)]
struct KnownArgs {
    saved: bool,
    /// `--level N` or `--hsk N`: HSK bands 1 to N.
    level: Option<u8>,
    /// `--hsk2 N`: HSK 2.0 levels 1 to N (2012 lists).
    hsk2: Option<u8>,
    /// `--tocfl N`: TOCFL levels 1 to N (Novice 1 to Level 5).
    tocfl: Option<u8>,
    lists: Vec<PathBuf>,
}

impl KnownArgs {
    /// The words marked known, or `None` if none of the options was given.
    fn resolve(&self) -> Option<hanstat::known::Settings> {
        if !self.saved
            && self.level.is_none()
            && self.hsk2.is_none()
            && self.tocfl.is_none()
            && self.lists.is_empty()
        {
            return None;
        }
        let mut k = hanstat::known::Settings::default();
        if self.saved {
            let (s, warn) = hanstat::known::Settings::load();
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
            k.hsk2 = s.hsk2;
            k.tocfl = s.tocfl;
        }
        if let Some(n) = self.hsk2 {
            (1..=n).for_each(|b| k.set_hsk2(b, true));
        }
        if let Some(n) = self.tocfl {
            (1..=n).for_each(|b| k.set_tocfl(b, true));
        }
        if let Some(n) = self.level {
            // Band 7 is the shared 7–9 list.
            let bands = &hanstat::known::BANDS;
            for &b in bands.iter().filter(|&&b| b <= n.min(7)) {
                k.set_band(b, true);
            }
        }
        for p in &self.lists {
            let l = hanstat::known::read_word_list(p).unwrap_or_else(|e| {
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
        "Anki: {} notes ({} with no Chinese in the chosen fields){}{}{}",
        k.notes,
        k.notes_without_chinese,
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
    let unique: &[(&str, &anki::KnownCount)] = &[
        ("Unique characters", &c.unique_chars),
        ("Unique words", &c.unique_words),
    ];
    for (label, k) in unique {
        println!(
            "{label:<18} known {:>7} of {:>7} ({:.1}%)   not known {:>7}",
            k.known,
            k.total,
            k.percent(),
            k.not_known()
        );
    }
    let running: &[(&str, &anki::KnownCount)] = &[
        ("Running chars", &c.running_chars),
        ("Running words", &c.running_words),
    ];
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
    /// Learn while reading ([`plan::Study::Read`]), with this rule (default:
    /// the window's, in `plan.json`).
    read: bool,
    read_percent: Option<u8>,
    min_count: Option<u32>,
    /// Read each series' books one after another ([`plan::Params::keep_series`]).
    keep_series: bool,
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
            read: false,
            read_percent: None,
            min_count: None,
            keep_series: false,
        }
    }
}

/// Order the books in the Library into a reading path, against `k`.
fn run_plan(a: &PlanArgs, k: Option<&Knowledge>, fold: bool, json: bool) {
    let (hist, warn) = History::load();
    if let Some(w) = warn {
        eprintln!("warning: {w}");
    }
    let mut books = Vec::new();
    let mut missing = Vec::new();
    for e in &hist.entries {
        match hanstat::profile::Profile::load(&hist.profile(&e.id)) {
            Some(p) => books.push((e.title.clone(), p)),
            None => missing.push(e),
        }
    }
    for e in &missing {
        eprintln!(
            "no word data for {} (analysed by an older version); left out: run hanstat-cli \"{}\" --library",
            e.title,
            e.path.display()
        );
    }
    if books.is_empty() {
        eprintln!(
            "No books with word data in the library ({}). Add some with hanstat-cli <file> --library.",
            History::path().display()
        );
        std::process::exit(1);
    }
    let window = plan::Settings::load().0;
    let params = if a.read {
        plan::Params {
            study: plan::Study::Read {
                percent: a.read_percent.unwrap_or(window.read_percent),
                min_count: a.min_count.unwrap_or(window.read_min_count),
            },
            objective: a.objective,
            incidental: None,
            lookahead: 0.0,
            keep_series: a.keep_series,
        }
    } else {
        plan::Params {
            study: match (a.target, a.learn) {
                (_, Some(n)) => plan::Study::Fixed(n),
                (Some(t), None) => plan::Study::Target(t),
                (None, None) => plan::Study::Target(95.0),
            },
            objective: a.objective,
            incidental: a.incidental,
            lookahead: a.lookahead,
            keep_series: a.keep_series,
        }
    };
    let t = std::time::Instant::now();
    // With where their words occur, when that was saved with them.
    let hist_path = History::path();
    let books: Vec<(
        String,
        hanstat::profile::Profile,
        Option<hanstat::profile::Positions>,
    )> = books
        .into_iter()
        .map(|(t, p)| {
            let pos = hanstat::profile::Positions::load(
                &hanstat::profile::positions_path_in(&hist_path, &p.id),
                p.digest(),
                p.words.len(),
            );
            (t, p, pos)
        })
        .collect();
    let lib =
        plan::Library::build_with(fold, hanstat::settings::Settings::load().skip_hapax, books);
    let mut k0 = lib.known(k);
    // Learning while reading, a word's meetings in the books marked read
    // count (their word counts as kept when marked, else the Library's).
    if a.read {
        let marks = hanstat::known::Settings::load().0.read;
        let counts: Vec<Vec<(String, u32)>> = marks
            .iter()
            .filter_map(|m| {
                hanstat::profile::Profile::load(&hanstat::profile::read_copy_path_in(
                    &hist_path, &m.id,
                ))
                .or_else(|| hanstat::profile::Profile::load(&hist.profile(&m.id)))
                .map(|p| p.words)
            })
            .collect();
        lib.add_read_meetings(&mut k0, counts.iter().map(|c| &c[..]));
    }
    let all: Vec<usize> = (0..lib.books.len()).collect();
    let width = a
        .width
        .unwrap_or_else(|| plan::width_for(&lib, &all, plan::SEARCH_BUDGET));
    let (path, naive) = lib
        .optimise_with_baseline(
            &k0,
            &params,
            &all,
            width,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
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
                if a.read {
                    return serde_json::json!({
                        "id": b.id,
                        "title": b.title,
                        "segmenter": b.segmenter,
                        "positions": b.has_positions(),
                        "from": s.from,
                        "known_now": s.now,
                        "known_when_reached": s.reached,
                        "known_at_start": s.start,
                        "known_over_book": s.reading,
                        "unknown_met": s.lookups,
                        "teaches": lib.study_list(s.book, &s.learned),
                        "words_from_earlier_books": s.gained,
                    });
                }
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
            "mode": if a.read { "read" } else { "study" },
            "lowest_start": a.read.then(|| path.lowest_start()),
            "total_unknown_met": a.read.then(|| path.total_lookups()),
            "objective": match params.objective {
                plan::Objective::Peak => "peak",
                plan::Objective::Total => "total",
            },
            "study": match params.study {
                plan::Study::Target(t) => serde_json::json!({"target": t}),
                plan::Study::Fixed(n) => serde_json::json!({"learn": n}),
                plan::Study::Read { percent, min_count } => {
                    serde_json::json!({"read_percent": percent, "min_count": min_count})
                }
            },
            "lookahead": params.lookahead,
            "incidental": params.incidental,
            "keep_series": params.keep_series,
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
        "Reading path: {} books, {}{}{}{}",
        lib.books.len(),
        match params.study {
            plan::Study::Target(t) => format!("studying until {t}% of running words are known"),
            plan::Study::Fixed(n) => format!("studying {n} words before each"),
            plan::Study::Read { percent, min_count } => format!(
                "learning while reading ({percent}% of the words a book uses {min_count}+ times, and words met {min_count}+ times across books)"
            ),
        },
        match params.incidental {
            Some(m) => format!(", words read {m}+ times learned"),
            None => String::new(),
        },
        if params.lookahead > 0.0 {
            format!(", lookahead {}", params.lookahead)
        } else {
            String::new()
        },
        if params.keep_series {
            ", each series' books together"
        } else {
            ""
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
    if a.read {
        println!(
            "{:>3}  {:>6}  {:>7}  {:>6}  {:>6}  {:>7}  {:>8}  {:>6}  title",
            "#", "now", "reached", "start", "book", "teaches", "unknown", "gained"
        );
        for (i, s) in path.steps.iter().enumerate() {
            let b = &lib.books[s.book];
            println!(
                "{:>3}  {:>5.1}%  {:>6.1}%  {:>5.1}%  {:>5.1}%  {:>7}  {:>8}  {:>6}  {}{}{}",
                i + 1,
                s.now,
                s.reached,
                s.start,
                s.reading,
                s.learned.len(),
                s.lookups,
                s.gained,
                s.from
                    .map(|f| format!("[reading, {:.0}%] ", f * 100.0))
                    .unwrap_or_default(),
                b.title,
                if b.has_positions() {
                    ""
                } else {
                    " (positions estimated)"
                },
            );
        }
        println!();
        let summary = |p: &plan::Path| {
            format!(
                "hardest beginning {:.1}% known, {} unknown words met, {} learned",
                p.lowest_start(),
                p.total_lookups(),
                p.total_learned()
            )
        };
        println!("This order:           {}", summary(&path));
        println!("Easiest-now order:    {}", summary(&naive));
        return;
    }
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

/// `hanstat-cli dict …`: the reader's Yomitan dictionaries.
///
/// ```text
/// hanstat-cli dict import <zip>... [--dir D]   import (replacing the same dictionary)
/// hanstat-cli dict list [--dir D]
/// hanstat-cli dict lookup <text> [--dir D]
/// hanstat-cli dict bench <text file> [--dir D]  2,000 lookups through a book
/// hanstat-cli dict check [--dir D]              announced revisions
/// ```
fn dict_cli(args: Vec<String>) -> Result<(), String> {
    use hanstat::dict::{format, import, lookup::Engine, manage};
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;
    let mut dir = hanstat::dict::root();
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--dir" {
            dir = PathBuf::from(it.next().ok_or("--dir expects a folder")?);
        } else {
            rest.push(a);
        }
    }
    let cmd = rest.first().cloned().unwrap_or_default();
    match cmd.as_str() {
        "import" => {
            manage::recover(&dir);
            let t_all = Instant::now();
            for zip in &rest[1..] {
                let zip = PathBuf::from(zip);
                let index = import::read_index(&zip)?;
                let id = manage::existing(&dir, &index).unwrap_or_else(format::new_id);
                let t = Instant::now();
                let m = import::import(
                    &zip,
                    &manage::tmp_dir(&dir, &id),
                    &import::Progress::default(),
                    &AtomicBool::new(false),
                )
                .inspect_err(|_| {
                    let _ = std::fs::remove_dir_all(manage::tmp_dir(&dir, &id));
                })?;
                manage::swap(&dir, &id)?;
                manage::add_to_manifest(&dir, &id)?;
                println!(
                    "{:>7.2}s  {:>9} terms {:>9} glossaries {:>9} freq {:>5} media {:>7.1} MB  {}",
                    t.elapsed().as_secs_f64(),
                    m.terms,
                    m.glossaries,
                    m.freqs,
                    m.media,
                    m.bytes as f64 / 1e6,
                    m.title
                );
            }
            println!("{:.2}s in all", t_all.elapsed().as_secs_f64());
        }
        "list" => {
            for (e, m) in manage::list(&dir) {
                match m {
                    Some(m) => println!(
                        "{} {} {:>9} terms {:>9} freq  rev {}  {}{}",
                        e.id,
                        if e.enabled { "on " } else { "off" },
                        m.terms,
                        m.freqs,
                        m.revision,
                        m.title,
                        if m.update.is_some() {
                            "  (updatable)"
                        } else {
                            ""
                        }
                    ),
                    None => println!("{} unreadable", e.id),
                }
            }
        }
        "lookup" => {
            let text = rest.get(1).ok_or("dict lookup expects text")?;
            let t = Instant::now();
            let (mut e, errors) = Engine::load(&dir, None);
            for err in errors {
                eprintln!("{err}");
            }
            let t_open = t.elapsed();
            let t = Instant::now();
            let r = e.lookup(text);
            let t_look = t.elapsed();
            for g in &r.groups {
                let freqs: Vec<String> = g
                    .freqs
                    .iter()
                    .map(|b| format!("{} {}", e.dicts[b.dict].title, b.values[0].display))
                    .collect();
                println!(
                    "{} [{}]{} {}",
                    g.headword,
                    g.reading,
                    if g.folded { " (简)" } else { "" },
                    freqs.join(", ")
                );
                for p in &g.parts {
                    let first = p.glossaries[0].json.as_deref().unwrap_or("(unreadable)");
                    let first: String = first.chars().take(100).collect();
                    println!(
                        "    {} ×{}: {first}",
                        e.dicts[p.dict].title,
                        p.glossaries.len()
                    );
                }
            }
            eprintln!("open {t_open:?}, lookup {t_look:?}");
        }
        "bench" => {
            let path = rest.get(1).ok_or("dict bench expects a text file")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
            let t = Instant::now();
            let (mut e, _) = Engine::load(&dir, None);
            let t_open = t.elapsed();
            let positions: Vec<usize> = (0..chars.len())
                .step_by((chars.len() / 2000).max(1))
                .take(2000)
                .collect();
            let mut times = Vec::new();
            let (mut groups, mut bytes) = (0usize, 0usize);
            let cold = Instant::now();
            let first: String = chars[positions[0]..].iter().take(16).collect();
            let _ = e.lookup(&first);
            let t_cold = cold.elapsed();
            for &p in &positions[1..] {
                let s: String = chars[p..].iter().take(16).collect();
                let t = Instant::now();
                let r = e.lookup(&s);
                times.push(t.elapsed().as_secs_f64() * 1e3);
                groups += r.groups.len();
                bytes += r
                    .groups
                    .iter()
                    .flat_map(|g| &g.parts)
                    .flat_map(|p| &p.glossaries)
                    .map(|g| g.json.as_ref().map_or(0, |j| j.len()))
                    .sum::<usize>();
            }
            times.sort_by(|a, b| a.total_cmp(b));
            println!(
                "{} dictionaries, open {:?}, first lookup {:?}; {} lookups: median {:.3} ms, p99 {:.3} ms, max {:.3} ms; {} groups, {:.1} MB of glossary JSON",
                e.dicts.len(),
                t_open,
                t_cold,
                times.len(),
                times[times.len() / 2],
                times[times.len() * 99 / 100],
                times.last().unwrap(),
                groups,
                bytes as f64 / 1e6
            );
        }
        "check" => {
            let urls: Vec<(String, String)> = manage::list(&dir)
                .into_iter()
                .filter_map(|(e, m)| Some((e.id, m?.update?.index_url)))
                .collect();
            let t = Instant::now();
            let results = hanstat::dict::update::check_all(urls, true);
            let wall = t.elapsed();
            for (id, r) in results {
                let m = manage::read_meta(&dir.join(&id));
                let (title, rev) = m.map(|m| (m.title, m.revision)).unwrap_or_default();
                match r {
                    Ok(remote) => println!(
                        "{}  {title}: {rev} → {}",
                        if hanstat::dict::revision::is_newer(&rev, &remote.revision) {
                            "UPDATE"
                        } else {
                            "same  "
                        },
                        remote.revision
                    ),
                    Err(e) => println!("FAILED  {title}: {e}"),
                }
            }
            println!("checked in {wall:?}");
        }
        _ => return Err("dict expects import, list, lookup, bench or check".into()),
    }
    Ok(())
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("dict") {
        if let Err(e) = dict_cli(std::env::args().skip(2).collect()) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
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
    let mut ka = KnownArgs::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--known" => ka.saved = true,
            "--hsk" | "--level" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    // 7, 8 and 9 share one list (band 7).
                    Some(n @ 1..=9) => n.min(7),
                    _ => {
                        eprintln!(
                            "{a} expects an HSK level from 1 to 9 (7, 8 and 9 share one list)"
                        );
                        std::process::exit(2)
                    }
                };
                ka.level = Some(n);
            }
            "--hsk2" => {
                let n: u8 = match args.next().and_then(|s| s.parse().ok()) {
                    Some(n @ 1..=6) => n,
                    _ => {
                        eprintln!("--hsk2 expects an HSK 2.0 level from 1 to 6");
                        std::process::exit(2)
                    }
                };
                ka.hsk2 = Some(n);
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
            "--known-list" => ka
                .lists
                .push(PathBuf::from(args.next().unwrap_or_default())),
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
            // `--history`: its name before the History became the Library.
            "--library" | "--history" => add_history = true,
            "--plan" => pa.on = true,
            "--target" => {
                pa.on = true;
                pa.target = Some(num(args.next(), "--target expects a percentage, e.g. 95"));
                if !(0.0..=100.0).contains(&pa.target.unwrap()) {
                    eprintln!("--target expects a percentage from 0 to 100");
                    std::process::exit(2)
                }
            }
            "--read" => {
                pa.on = true;
                pa.read = true;
            }
            "--read-percent" => {
                pa.on = true;
                pa.read = true;
                pa.read_percent = Some(num(
                    args.next(),
                    "--read-percent expects a percentage, e.g. 90",
                ));
            }
            "--min-count" => {
                pa.on = true;
                pa.read = true;
                pa.min_count = Some(num(
                    args.next(),
                    "--min-count expects a number of times, e.g. 10",
                ));
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
            "--keep-series" => {
                pa.on = true;
                pa.keep_series = true;
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
                    "usage: hanstat-cli <file.epub|file.txt> [--ckip|--ltp|--jieba] [--dict words.txt] [--json] [--top N] [--include-all] [--exclude i,j] [--chars-csv f] [--words-csv f] [--suggest N] [--auto-dict] [--no-cache] [--library]\n\
                     \x20      hanstat-cli --plan [--read [--read-percent P] [--min-count M] | --target P | --learn N] [--incidental M] [--lookahead X] [--objective peak|total] [--keep-series] [--width W] [--study-lists DIR] [--json]\n\
                     \x20      [--anki] [--anki-list] [--anki-collection collection.anki2] [--anki-deck NAME]... [--anki-field \"NOTE TYPE:FIELD\"]... [--anki-mature-stability DAYS] [--anki-save]\n\
                     \x20      [--known] [--level N] [--hsk2 N] [--tocfl N] [--known-list words.txt]...\n\
                     \n\
                     The word counts of a book are saved in {} and read back when the same text is analysed again with the same\n\
                     \x20 segmenter, model, --dict and known words by the same analysis code; --no-cache (or HANSTAT_NO_CACHE) analyses afresh and saves nothing.\n\
                     --known adds the levels and word lists marked known in the hanstat window (saved in {}).\n\
                     --level N (or --hsk N) marks HSK 3.0 levels 1 to N known (2025 syllabus; 7, 8, 9: all).\n\
                     --hsk2 N marks HSK 2.0 levels 1 to N known (the 2012 lists, 5,000 words).\n\
                     --tocfl N marks TOCFL levels 1 to N known (SC-TOP's 華語八千詞表, 2024: 1–2 are Novice 1–2, 3–7 are Level 1–5).\n\
                     --known-list adds a word list (one word per line).\n\
                     These count as known together with young/mature Anki cards, or on their own without --anki.\n\
                     --anki uses the decks/note types/fields chosen in the hanstat window (saved in {}).\n\
                     --anki-deck/--anki-field replace that choice for this run (repeatable; a deck includes its subdecks); --anki-save keeps it.\n\
                     --anki-mature-stability DAYS counts a review card as mature once its FSRS stability reaches DAYS instead of at an interval of 21+ days (0: interval rule).\n\
                     Without a file, --anki prints the collection summary only.\n\
                     \n\
                     --library records the book in the Library (and its word data for the reading path), as opening it in the window does.\n\
                     --plan orders the books in the Library into a reading path against the known words (--anki, --known, --level, ...):\n\
                     \x20 before each book you study its unknown words until --target P% of its running words are known (default 95), or --learn N of them;\n\
                     \x20 they stay known for the books after it. --incidental M also counts words read M+ times in a book as learned.\n\
                     \x20 --lookahead X weighs the books still to come when choosing study words (0: the book's most frequent; default {}).\n\
                     \x20 --read instead studies nothing: a book teaches the --read-percent P% most frequent of the words it uses --min-count M+ times,\n\
                     \x20 and words met M+ times across books (as ✔ Mark read; defaults from the window), and books are ordered by how easy they start\n\
                     \x20 (their first 20,000 running words). --objective peak (default) or total (fewest words studied / unknown words met in all).\n\
                     \x20 --keep-series reads each series' books one after another, in volume order, with no other book between them.\n\
                     \x20 --width W sets the search's beam width (1: greedy). --study-lists DIR writes each book's study list (word<TAB>count).",
                    hanstat::cache::Cache::default_dir().display(),
                    hanstat::known::Settings::path().display(),
                    hanstat::anki::Settings::path().display(),
                    plan::DEFAULT_LOOKAHEAD
                );
                return;
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let open_anki = || {
        if ak.on || ak.list {
            load_anki(&ak).unwrap_or_else(|e| {
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
            eprintln!("--plan works on the Library: give no file");
            std::process::exit(2);
        }
        let anki_col = open_anki();
        let anki_k = anki_col
            .as_ref()
            .map(|loaded| anki_knowledge(loaded, &anki::SegConfig::new(kind, None, &words)));
        let across = match &anki_col {
            Some((_, s)) => s.match_across_scripts,
            None => anki::Settings::load().0.match_across_scripts,
        };
        let k = match ka.resolve() {
            Some(l) => l.apply(anki_k.as_ref(), across),
            None => anki_k,
        };
        let fold = plan::Library::fold_for(k.as_ref(), across);
        run_plan(&pa, k.as_ref(), fold, json);
        return;
    }
    let Some(file) = file else {
        if let Some(loaded) = &open_anki() {
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
    let cache = hanstat::cache::Cache::open().filter(|_| !no_cache);
    let book = hanstat::book_cache::load(&file, EncodingChoice::Auto, cache.as_ref())
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
    let t_load = t0.elapsed();
    let anki_col = open_anki();
    let known_lists = ka.resolve();
    // Imported word lists also join words the segmenter split.
    let seg_known: Vec<String> = known_lists
        .as_ref()
        .map(|k| k.segmenter_words())
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
    let analyse = |words: &[String]| {
        analyze::analyze_book_cached(&book, kind, words, &seg_known, cache.as_ref(), &|_, _| {})
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
    };
    let t_analysis = std::time::Instant::now();
    let mut prep = analyse(&words);
    let mut analysis = (t_analysis.elapsed(), prep.cached);
    let included: Vec<bool> = book
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| !exclude.contains(&i) && (include_all || s.default_included()))
        .collect();
    let mut words = words;
    let book_segmenter = prep.segmenter_kind;
    let fold = match &anki_col {
        Some((_, s)) => s.match_across_scripts,
        None => anki::Settings::load().0.match_across_scripts,
    };
    let mut r = analyze::build_report(&book, &prep, &included, fold);
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
        prep = analyse(&words);
        analysis = (analysis.0 + t_analysis.elapsed(), analysis.1 && prep.cached);
        r = analyze::build_report(&book, &prep, &included, fold);
    }
    let anki_k = anki_col.as_ref().map(|loaded| {
        anki_knowledge(
            loaded,
            &anki::SegConfig::new(kind, Some(book_segmenter), &words),
        )
    });
    let known_k = match &known_lists {
        Some(l) => l.apply(anki_k.as_ref(), fold),
        None => anki_k,
    };
    let discovered: Option<(Vec<hanstat::discover::Candidate>, std::time::Duration)> =
        (known_k.is_some() || add_history || suggest.is_some()).then(|| {
            let t = std::time::Instant::now();
            let known: std::collections::HashSet<String> = words.iter().cloned().collect();
            let found = hanstat::discover::discover(
                &book,
                &included,
                &r.word_freq,
                &known,
                &hanstat::discover::Options::for_size(r.han_total),
            );
            (found, t.elapsed())
        });
    // Names are left out of the difficulty: the user dictionary plus the
    // recommended discovered words, as the GUI does.
    let names: Option<std::collections::HashSet<String>> =
        (known_k.is_some() || add_history).then(|| {
            let mut names: std::collections::HashSet<String> = words.iter().cloned().collect();
            if let Some((found, _)) = &discovered {
                names.extend(
                    found
                        .iter()
                        .filter(|c| c.recommended)
                        .map(|c| c.word.clone()),
                );
            }
            names
        });
    if let (Some(k), Some(names)) = (known_k, &names) {
        r.anki = Some(BookCoverage::compute(
            &k,
            &r.char_freq,
            &r.word_freq,
            names,
            hanstat::settings::Settings::load().skip_hapax,
        ));
    }
    if add_history {
        let positions =
            hanstat::profile::Positions::from_analysis(&book, &prep, &included, &r.word_freq);
        record_history(&file, &book, &r, names.as_ref().unwrap(), &positions);
    }
    eprintln!(
        "load {:?}, analysis {:?}{}, total {:?}",
        t_load,
        analysis.0,
        if analysis.1 { " (from cache)" } else { "" },
        t0.elapsed()
    );

    if let Some(p) = chars_csv {
        std::fs::write(p, analyze::freq_csv(&r.char_freq, "character")).expect("write csv");
    }
    if let Some(p) = words_csv {
        std::fs::write(p, analyze::freq_csv(&r.word_freq, "word")).expect("write csv");
    }
    if let Some(n) = suggest {
        let (c, elapsed) = discovered.unwrap();
        eprintln!("discovery {elapsed:?}, {} candidates", c.len());
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

/// `--library`: the book's Library entry, cover and word data, as the window saves them.
fn record_history(
    file: &std::path::Path,
    book: &extract::Book,
    r: &Report,
    names: &std::collections::HashSet<String>,
    positions: &hanstat::profile::Positions,
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
        sections: Some(r.sections.iter().map(|s| s.included).collect()),
        // Unknown: the window brings the row up to date with its own inputs.
        inputs: String::new(),
    };
    if let Err(e) = hist.record(entry, true) {
        fail(e);
    }
    let p = hanstat::profile::Profile::from_report(&id, r, names);
    p.save(&hist.profile(&id)).unwrap_or_else(|e| fail(e));
    positions
        .save(&hist.positions(&id), p.digest())
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
