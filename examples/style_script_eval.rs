//! Does the script change the writing-style rating? Rates each book's
//! samples as they are and with the script folded (Traditional → Simplified
//! with `script::fold_t2s`, Simplified → Traditional with
//! `script::fold_s2t`), same model, prompt and windows. Per-question answers
//! go to `style.log` in the app folder, so point `LOCALAPPDATA` at a scratch
//! folder and `HANSTAT_MODEL_DIR` at the real models.
//!
//!     cargo run --release --example style_script_eval -- book.epub ...

use hanstat::extract::{self, EncodingChoice};
use hanstat::script::{self, Variant};
use hanstat::style::{self, Log, Mode, Server};
use std::path::Path;

fn main() {
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    let log = Log::create();
    let server = Server::start(Mode::Gpu, &log).expect("llama.cpp");
    // `--rate-book`: rate as the app does (`Server::rate_book`) instead.
    if paths.first().is_some_and(|a| a == "--rate-book") {
        paths.remove(0);
        for p in &paths {
            let book = extract::load(Path::new(p), EncodingChoice::Auto).expect("book");
            let r = server.rate_book(&book).expect("rate").expect("long enough");
            println!("{p}\t{:.3}\t{:?}", r.score, r.samples);
        }
        return;
    }
    println!("title\tvariant\tsample\tas_is\tfolded");
    for p in &paths {
        let book = match extract::load(Path::new(p), EncodingChoice::Auto) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{p}: {e}");
                continue;
            }
        };
        let Some(samples) = style::samples(&book) else {
            eprintln!("{p}: too short");
            continue;
        };
        let text: String = samples.concat();
        let (mut t, mut s) = (0u64, 0u64);
        let tables = script::fold_t2s(&text);
        for (a, b) in text.chars().zip(tables.chars()) {
            if a != b {
                t += 1;
            }
        }
        let back = script::fold_s2t(&text);
        for (a, b) in text.chars().zip(back.chars()) {
            if a != b {
                s += 1;
            }
        }
        // More characters change under S→T than T→S: the text is Simplified.
        let variant = if t > s {
            Variant::Traditional
        } else {
            Variant::Simplified
        };
        let title = Path::new(p).file_stem().unwrap().to_string_lossy();
        for (k, smp) in samples.iter().enumerate() {
            let folded = match variant {
                Variant::Traditional => script::fold_t2s(smp),
                _ => script::fold_s2t(smp),
            };
            log.line(format!("BOOK {title} sample {k} as_is"));
            let a = server.rate(smp).expect("rate");
            log.line(format!("BOOK {title} sample {k} folded"));
            let b = server.rate(&folded).expect("rate");
            println!("{title}\t{variant:?}\t{k}\t{a:.3}\t{b:.3}");
        }
    }
}
