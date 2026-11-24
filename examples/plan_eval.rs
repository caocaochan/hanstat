//! Measure the reading path on the books in a History (`LOCALAPPDATA` picks
//! which): the optimised order against reading the easiest-now book first,
//! greedy against beam search, the lookahead weight, and the beam against
//! every order of small random subsets.
//!
//!     cargo run --release --example plan_eval [-- --subsets N]

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
            _ => {}
        }
    }
    let (hist, _) = History::load();
    let books: Vec<(String, Profile)> = hist
        .entries
        .iter()
        .filter_map(|e| Profile::load(&hist.profile(&e.id)).map(|p| (e.title.clone(), p)))
        .collect();
    println!("{} books with word data", books.len());
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
        for study in [Study::Target(95.0), Study::Target(98.0), Study::Fixed(300)] {
            for &objective in &objectives {
                for &lam in &lambdas {
                    let p = Params {
                        study,
                        objective,
                        incidental: None,
                        lookahead: lam,
                    };
                    let naive = lib.evaluate(&k0, &p, &lib.easiest_now(&k0, &all));
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
                    };
                    // How much of each book's first-read cost the path saved.
                    let gain = match study {
                        Study::Target(_) => {
                            let (a, b) = beam.steps.iter().fold((0usize, 0usize), |acc, s| {
                                (acc.0 + s.study_now.unwrap_or(0), acc.1 + s.study.len())
                            });
                            format!("{:>8.0}%", 100.0 * (1.0 - b as f64 / a.max(1) as f64))
                        }
                        Study::Fixed(_) => String::new(),
                    };
                    println!(
                        "{:<12} {:<6} {:>5} | {} | {} | {} | {:>8.2} | {}",
                        match study {
                            Study::Target(t) => format!("target {t}%"),
                            Study::Fixed(n) => format!("learn {n}"),
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

fn mean_reading(p: &plan::Path) -> f64 {
    p.steps.iter().map(|s| s.reading).sum::<f64>() / p.steps.len().max(1) as f64
}

fn fmt_key(p: &plan::Path, study: Study) -> String {
    match study {
        Study::Target(_) => format!("{}/{}", p.peak(), p.total_study()),
        Study::Fixed(_) => format!("{:.2}%", p.lowest_reading()),
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
