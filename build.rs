use std::path::{Path, PathBuf};

fn main() {
    icon();
    analysis_fingerprint();
}

// Embeds assets/icon.ico as the exe's icon resource, so Explorer, shortcuts and
// the taskbar match the window. assets/icon.png (the window icon) is the ICO's
// 256 px entry; regenerate it whenever the ICO changes.
fn icon() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to compile the Windows icon resource");
    }
}

/// Sources that cannot change what `analyze::analyze_book` counts for a given
/// text, segmenter, model and word lists: the windows, the CLI, what happens
/// before the text exists (`extract`: the text itself is in the cache key),
/// what is applied to the counts afterwards (Anki, known words, discovery,
/// History, profiles, the reading path and series), the module list in `lib.rs`, and
/// downloading (the model's identity is in the key). Anything not listed
/// here, including a file added later, is part of the fingerprint.
const NOT_ANALYSIS: &[&str] = &[
    "src/main.rs",
    "src/lib.rs",
    "src/bin",
    "src/history.rs",
    "src/profile.rs",
    "src/plan.rs",
    "src/series.rs",
    "src/extract",
    "src/anki.rs",
    "src/known.rs",
    "src/discover.rs",
    "src/models.rs",
    "src/update.rs",
];

/// Bundled data that is not read by the analysis: licences and the
/// vocabulary lists of `known.rs`.
const NOT_ANALYSIS_DATA: &[&str] = &[
    "data/HSK30-2025.tsv",
    "data/TOCFL-2024.tsv",
    "data/KO-standard-2020.tsv",
    "data/JLPT-waller.tsv",
    "data/Kaishi-1.5k.tsv",
];

/// `HANSTAT_ANALYSIS_FINGERPRINT`: a hash of the analysis sources, the
/// tables they embed and the dependency versions. The analysis cache
/// (`src/cache.rs`) puts it in every key, so results saved by a build whose
/// analysis differs are never read, while a release that only touches the
/// rest keeps the cache.
fn analysis_fingerprint() {
    for p in ["src", "data", "Cargo.toml", "Cargo.lock", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let mut files = Vec::new();
    walk(Path::new("src"), &mut files);
    walk(Path::new("data"), &mut files);
    let mut files: Vec<(String, PathBuf)> = files
        .into_iter()
        .map(|p| (p.to_string_lossy().replace('\\', "/"), p))
        .filter(|(name, _)| {
            !NOT_ANALYSIS
                .iter()
                .any(|x| name == x || name.starts_with(&format!("{x}/")))
                && !NOT_ANALYSIS_DATA.contains(&name.as_str())
                && !name.ends_with("-LICENSE.txt")
        })
        .collect();
    files.sort();

    let mut h = Fnv::new();
    for (name, path) in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        h.item(name.as_bytes());
        h.item(&bytes);
    }
    // The manifests without this package's own version, which changes with
    // every release: features and dependency versions are what matter.
    let manifest = read("Cargo.toml");
    let manifest: Vec<&str> = manifest
        .lines()
        .scan(false, |seen, l| {
            let own = !*seen && l.starts_with("version = ");
            *seen |= own;
            Some((!own).then_some(l))
        })
        .flatten()
        .collect();
    h.item(manifest.join("\n").as_bytes());
    let lock = read("Cargo.lock");
    let mut lines = Vec::new();
    let mut prev = "";
    for l in lock.lines() {
        if !(prev == "name = \"hanstat\"" && l.starts_with("version = ")) {
            lines.push(l);
        }
        prev = l;
    }
    h.item(lines.join("\n").as_bytes());
    println!("cargo:rustc-env=HANSTAT_ANALYSIS_FINGERPRINT={:016x}", h.0);
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("directory entry").path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// 64-bit FNV-1a: stable across toolchains (unlike `DefaultHasher`) and needs
/// no build dependency. It only has to tell successive versions of the
/// sources apart; the cache key itself is SHA-256.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    /// One length-prefixed item. Carriage returns are skipped, so a checkout
    /// with CRLF line endings gives the same fingerprint.
    fn item(&mut self, bytes: &[u8]) {
        let text = || bytes.iter().copied().filter(|b| *b != b'\r');
        for b in (text().count() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(text())
        {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
