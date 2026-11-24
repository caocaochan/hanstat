//! Measure the reading path on the books in a History (`LOCALAPPDATA` picks
//! which): the optimised order against reading the easiest-now book first,
//! greedy against beam search, the lookahead weight, and the beam against
//! every order of small random subsets; and learning while reading, with
//! where words occur against an even spread; and keeping series together
//! against letting other books come between volumes.
//!
//!     cargo run --release --example plan_eval [-- --subsets N] [--keep-series]
//!
//! `--keep-series` plans every order above with each series kept together.

use hanstat::history::History;
use hanstat::known;
use hanstat::plan::{self, Library, Objective, Params, Study};
use hanstat::profile::Profile;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let mut subsets = 6usize;
    let mut lambdas: Vec<f32> = vec![0.0, 0.1, 0.25, 0.5, 1.0];
    let mut objectives = vec![Objective::Total, Objective::Peak];
    let mut keep_series = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--subsets" => subsets = args.next().and_then(|s| s.parse().ok()).unwrap_or(6),
            "--lambdas" => {
                lambdas = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|x| x.parse().ok())
                    .collect()
            }
            "--peak-only" => objectives = vec![Objective::Peak],
            "--keep-series" => keep_series = true,
            _ => {}
        }
    }
    let (hist, _) = History::load();
    let books: Vec<(String, Profile)> = hist
        .entries
        .iter()
        .filter_map(|e| Profile::load(&hist.profile(&e.id)).map(|p| (e.title.clone(), p)))
        .collect();
    let positions: Vec<Option<hanstat::profile::Positions>> = books
        .iter()
        .map(|(_, p)| {
            hanstat::profile::Positions::load(&hist.positions(&p.id), p.digest(), p.words.len())
        })
        .collect();
    println!(
        "{} books with word data, {} with word positions",
        books.len(),
        positions.iter().filter(|p| p.is_some()).count()
    );
    let no = AtomicBool::new(false);

    // Learners at three levels.
    let levels: &[(&str, &[u8])] = &[
        ("HSK 1-3", &[1, 2, 3]),
        ("HSK 1-5", &[1, 2, 3, 4, 5]),
        ("HSK 1-6 + 7-9", &[1, 2, 3, 4, 5, 6, 7]),
    ];
    for (label, bands) in levels {
        let mut s = known::Settings::default();
        for &b in *bands {
            s.set_band(b, true);
        }
        let k = s.apply(None, true);
        let fold = Library::fold_for(k.as_ref(), true);
        let lib = Library::build(fold, false, books.clone());
        let k0 = lib.known(k.as_ref());
        let all: Vec<usize> = (0..lib.books.len()).collect();
        let width = plan::width_for(&lib, &all, plan::SEARCH_BUDGET);
        println!(
            "\n=== {label}: {} of {} library words known; beam width {width}",
            k0.len(),
            lib.vocab_len()
        );
        let now: Vec<f64> = lib
            .evaluate(
                &k0,
                &Params {
                    study: Study::Fixed(0),
                    lookahead: 0.0,
                    ..Params::default()
                },
                &all,
            )
            .steps
            .iter()
            .map(|s| s.now)
            .collect();
        let (lo, hi) = now
            .iter()
            .fold((f64::MAX, 0f64), |a, &x| (a.0.min(x), a.1.max(x)));
        println!("coverage now: {lo:.1}% to {hi:.1}%");
        println!(
            "{:<12} {:<6} {:>5} | {:>18} | {:>18} | {:>18} | {:>8} | {:>9}",
            "study", "obj", "λ", "easiest-now", "greedy", "beam", "beam s", "mean gain"
        );
        const READ: Study = Study::Read {
            percent: 90,
            min_count: 10,
        };
        for study in [
            Study::Target(95.0),
            Study::Target(98.0),
            Study::Fixed(300),
            READ,
        ] {
            for &objective in &objectives {
                for &lam in &lambdas {
                    // Nothing is studied: the weight changes nothing.
                    if matches!(study, Study::Read { .. }) && lam != lambdas[0] {
                        continue;
                    }
                    let p = Params {
                        study,
                        objective,
                        incidental: None,
                        lookahead: lam,
                        keep_series,
                    };
                    let naive =
                        lib.evaluate(&k0, &p, &lib.started_first(&p, &lib.easiest_now(&k0, &all)));
                    let greedy = lib.optimise(&k0, &p, &all, 1, &no).unwrap();
                    let t = Instant::now();
                    let beam = lib.optimise(&k0, &p, &all, width, &no).unwrap();
                    let secs = t.elapsed().as_secs_f64();
                    let show = |x: &plan::Path| match study {
                        Study::Target(_) => format!("{:>7} / {:>6}", x.total_study(), x.peak()),
                        Study::Fixed(_) => format!(
                            "low {:>5.1}% / {:>4.1}",
                            x.lowest_reading(),
                            mean_reading(x)
                        ),
                        Study::Read { .. } => format!(
                            "{:>5.1}% / {:>6}k",
                            x.lowest_start(),
                            x.total_lookups() / 1000
                        ),
                    };
                    // How much of each book's first-read cost the path saved.
                    let gain = match study {
                        Study::Target(_) => {
                            let (a, b) = beam.steps.iter().fold((0usize, 0usize), |acc, s| {
                                (acc.0 + s.study_now.unwrap_or(0), acc.1 + s.study.len())
                            });
                            format!("{:>8.0}%", 100.0 * (1.0 - b as f64 / a.max(1) as f64))
                        }
                        Study::Fixed(_) | Study::Read { .. } => String::new(),
                    };
                    println!(
                        "{:<12} {:<6} {:>5} | {} | {} | {} | {:>8.2} | {}",
                        match study {
                            Study::Target(t) => format!("target {t}%"),
                            Study::Fixed(n) => format!("learn {n}"),
                            Study::Read { .. } => "read".to_owned(),
                        },
                        format!("{objective:?}"),
                        lam,
                        show(&naive),
                        show(&greedy),
                        show(&beam),
                        secs,
                        gain
                    );
                }
            }
        }

        // Learning while reading: where words occur against an even spread,
        // and how much of what the path teaches comes from meetings across
        // books (words no single book teaches).
        {
            let lib_pos = Library::build_with(
                fold,
                false,
                books
                    .iter()
                    .cloned()
                    .zip(positions.iter().cloned())
                    .map(|((t, p), pos)| (t, p, pos))
                    .collect(),
            );
            let k0p = lib_pos.known(k.as_ref());
            let p = Params {
                study: READ,
                objective: Objective::Peak,
                incidental: None,
                lookahead: 0.0,
                keep_series,
            };
            let even = lib.optimise(&k0, &p, &all, width, &no).unwrap();
            let placed = lib_pos.optimise(&k0p, &p, &all, width, &no).unwrap();
            // The positioned order, figured without positions: how much the
            // order changed for the even-spread model's own measure.
            let placed_even = lib.evaluate(&k0, &p, &placed.order());
            let rho = spearman_orders(&even.order(), &placed.order());
            println!(
                "read, even spread: {} | with positions: {} (that order with an even spread: {}) | order agreement rho {:.2}",
                fmt_key(&even, READ),
                fmt_key(&placed, READ),
                fmt_key(&placed_even, READ),
                rho
            );
            let starts: Vec<(f64, f64)> = all
                .iter()
                .map(|&b| {
                    let a = lib.evaluate(&k0, &p, &[b]).steps[0].start;
                    let c = lib_pos.evaluate(&k0p, &p, &[b]).steps[0].start;
                    (a, c)
                })
                .collect();
            let diff: Vec<f64> = starts.iter().map(|(a, c)| c - a).collect();
            let mean = diff.iter().sum::<f64>() / diff.len().max(1) as f64;
            let (lo, hi) = diff
                .iter()
                .fold((f64::MAX, f64::MIN), |a, &x| (a.0.min(x), a.1.max(x)));
            println!(
                "start read first, positions minus even spread: mean {mean:+.2} points, from {lo:+.2} to {hi:+.2}"
            );
            let (mut taught, mut across) = (0usize, 0usize);
            for s in &placed.steps {
                let (_, prof) = &books[s.book];
                let sel: std::collections::HashSet<String> =
                    known::read_selection(&prof.words, 90, 10)
                        .into_iter()
                        .map(|(w, _)| hanstat::anki::fold(w, fold))
                        .collect();
                taught += s.learned.len();
                across += s
                    .learned
                    .iter()
                    .filter(|&&w| !sel.contains(lib_pos.word(w)))
                    .count();
            }
            println!(
                "read: the path teaches {taught} words, {across} of them ({:.1}%) from meetings across books",
                100.0 * across as f64 / taught.max(1) as f64
            );
        }

        // Keeping series together: what it costs, how many series the
        // planner splits otherwise, and the search time.
        {
            let mut series: Vec<u32> = lib
                .books
                .iter()
                .filter_map(|b| b.series.as_ref().map(|s| s.0))
                .collect();
            let in_series = series.len();
            series.sort_unstable();
            series.dedup();
            println!(
                "series: {in_series} of {} books, in {} series",
                lib.books.len(),
                series.len()
            );
            for (study, objective) in [
                (Study::Target(95.0), Objective::Peak),
                (Study::Target(95.0), Objective::Total),
                (READ, Objective::Peak),
                (READ, Objective::Total),
            ] {
                let mut p = Params {
                    study,
                    objective,
                    incidental: None,
                    lookahead: if matches!(study, Study::Read { .. }) {
                        0.0
                    } else {
                        plan::DEFAULT_LOOKAHEAD
                    },
                    keep_series: false,
                };
                let t = Instant::now();
                let free = lib.optimise(&k0, &p, &all, width, &no).unwrap();
                let t_free = t.elapsed().as_secs_f64();
                p.keep_series = true;
                let t = Instant::now();
                let kept = lib.optimise(&k0, &p, &all, width, &no).unwrap();
                let t_kept = t.elapsed().as_secs_f64();
                assert!(series_kept_together(&lib, &kept.order()));
                println!(
                    "keep series, {study:?} {objective:?}: between volumes {} ({} series split, {t_free:.2}s) | together {} ({t_kept:.2}s)",
                    fmt_key(&free, study),
                    split_series(&lib, &free.order()),
                    fmt_key(&kept, study),
                );
            }
        }

        // Beam against every order of random 7-book subsets.
        if *label == "HSK 1-5" && subsets > 0 && lib.books.len() >= 7 {
            let mut seed = 0x2545_f491_4f6c_dd1du64;
            let mut rnd = |m: usize| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % m as u64) as usize
            };
            for (study, objective) in [
                (Study::Target(95.0), Objective::Total),
                (Study::Target(95.0), Objective::Peak),
                (Study::Fixed(300), Objective::Total),
            ] {
                let p = Params {
                    study,
                    objective,
                    incidental: None,
                    lookahead: plan::DEFAULT_LOOKAHEAD,
                    keep_series,
                };
                let (mut same, mut gaps) = (0, Vec::new());
                for _ in 0..subsets {
                    let mut pick: Vec<usize> = Vec::new();
                    while pick.len() < 7 {
                        let b = rnd(lib.books.len());
                        if !pick.contains(&b) {
                            pick.push(b);
                        }
                    }
                    let t = Instant::now();
                    let beam = lib
                        .optimise(
                            &k0,
                            &p,
                            &pick,
                            plan::width_for(&lib, &pick, plan::SEARCH_BUDGET),
                            &no,
                        )
                        .unwrap();
                    let tb = t.elapsed();
                    let t = Instant::now();
                    let best = brute(&lib, &k0, &p, &pick);
                    let tx = t.elapsed();
                    if beam.key() == best.key() {
                        same += 1;
                    }
                    gaps.push(format!(
                        "{}→{} ({:.2}s vs {:.1}s)",
                        fmt_key(&beam, study),
                        fmt_key(&best, study),
                        tb.as_secs_f64(),
                        tx.as_secs_f64()
                    ));
                }
                println!(
                    "every order of 7 books, {study:?} {objective:?}: beam optimal in {same} of {subsets}: {}",
                    gaps.join("; ")
                );
            }
        }
    }
}

/// Series whose books have another book between two of them.
fn split_series(lib: &Library, order: &[usize]) -> usize {
    let mut seen: Vec<u32> = Vec::new();
    let mut split: Vec<u32> = Vec::new();
    let mut last = None;
    for &b in order {
        let s = lib.books[b].series.as_ref().map(|x| x.0);
        if let Some(s) = s
            && last != Some(s)
        {
            if seen.contains(&s) {
                if !split.contains(&s) {
                    split.push(s);
                }
            } else {
                seen.push(s);
            }
        }
        last = s;
    }
    split.len()
}

fn series_kept_together(lib: &Library, order: &[usize]) -> bool {
    split_series(lib, order) == 0
}

/// Spearman's rho between two orders of the same books.
fn spearman_orders(a: &[usize], b: &[usize]) -> f64 {
    let n = a.len() as f64;
    if n < 2.0 {
        return 1.0;
    }
    let pos = |o: &[usize], x: usize| o.iter().position(|&y| y == x).unwrap() as f64;
    let d2: f64 = a.iter().map(|&x| (pos(a, x) - pos(b, x)).powi(2)).sum();
    1.0 - 6.0 * d2 / (n * (n * n - 1.0))
}

fn mean_reading(p: &plan::Path) -> f64 {
    p.steps.iter().map(|s| s.reading).sum::<f64>() / p.steps.len().max(1) as f64
}

fn fmt_key(p: &plan::Path, study: Study) -> String {
    match study {
        Study::Target(_) => format!("{}/{}", p.peak(), p.total_study()),
        Study::Fixed(_) => format!("{:.2}%", p.lowest_reading()),
        Study::Read { .. } => format!("{:.2}%/{}", p.lowest_start(), p.total_lookups()),
    }
}

fn brute(lib: &Library, k0: &plan::Known, p: &Params, books: &[usize]) -> plan::Path {
    use rayon::prelude::*;
    fn perms(v: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
        if k == v.len() {
            out.push(v.clone());
            return;
        }
        for i in k..v.len() {
            v.swap(k, i);
            perms(v, k + 1, out);
            v.swap(k, i);
        }
    }
    let mut all = Vec::new();
    perms(&mut books.to_vec(), 0, &mut all);
    all.par_iter()
        .map(|o| lib.evaluate(k0, p, o))
        .min_by_key(|p| p.key())
        .unwrap()
}
