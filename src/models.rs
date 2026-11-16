//! Segmentation models are not bundled (their licences differ from this
//! project's). They are downloaded on first use from their official Hugging
//! Face repositories or GitHub releases, pinned to an exact commit or release
//! and verified by SHA-256, then cached in `%LOCALAPPDATA%\hanstat\models`
//! (override: `HANSTAT_MODEL_DIR`). SudachiDict is the exception: it follows
//! its newest GitHub release (see [`sudachi`]), verified by the SHA-256 that
//! GitHub reports for the asset.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    /// What to keep of the download.
    pub unpack: Unpack,
}

/// One file taken out of a downloaded archive.
pub struct Member {
    /// Path inside the archive (`/`-separated).
    pub path: &'static str,
    /// File name it is saved as in the model folder.
    pub name: &'static str,
    /// Expected size; 0 when not known in advance (the archive was verified
    /// by its SHA-256, and members are written under a temporary name).
    pub size: u64,
}

pub enum Unpack {
    /// Keep the download as is, named [`ModelFile::name`].
    Keep,
    /// Extract these members of a ZIP archive, then delete the archive.
    Zip(&'static [Member]),
    /// Extract these members of a gzip-compressed tar archive, then delete the archive.
    TarGz(&'static [Member]),
}

impl ModelFile {
    /// Everything this file provides is in `dir` with the expected size.
    fn is_present(&self, dir: &Path) -> bool {
        let ok = |name: &str, size: u64| {
            std::fs::metadata(dir.join(name)).is_ok_and(|m| size == 0 || m.len() == size)
        };
        match self.unpack {
            Unpack::Keep => ok(self.name, self.size),
            Unpack::Zip(ms) | Unpack::TarGz(ms) => ms.iter().all(|m| ok(m.name, m.size)),
        }
    }
}

pub struct Model {
    pub id: &'static str,
    pub title: &'static str,
    pub licence: &'static str,
    pub homepage: &'static str,
    pub files: &'static [ModelFile],
}

pub const LTP: Model = Model {
    id: "ltp-legacy",
    title: "LTP legacy CWS model (HIT-SCIR)",
    licence: "LTP licence: free for universities, research institutes and individual researchers; commercial use requires a paid licence from HIT-SCIR.",
    homepage: "https://huggingface.co/LTP/legacy",
    files: &[ModelFile {
        name: "cws_model.bin",
        url: "https://huggingface.co/LTP/legacy/resolve/aca6c995c6ca7ab2e4cba4af34964bcc37742fe9/cws_model.bin",
        sha256: "28fc2db628b27d4f69162d697d2294671784c7a61e0b4ce414e31060cfd20592",
        size: 31_977_765,
        unpack: Unpack::Keep,
    }],
};

macro_rules! ckip_url {
    ($f:literal) => {
        concat!("https://huggingface.co/ckiplab/bert-tiny-chinese-ws/resolve/f4a11d4b00b06502c260d8134a375a4000b09d7b/", $f)
    };
}

pub const CKIP: Model = Model {
    id: "ckip-bert-tiny-chinese-ws",
    title: "CKIP bert-tiny-chinese-ws (Academia Sinica CKIP Lab)",
    licence: "GPL-3.0",
    homepage: "https://huggingface.co/ckiplab/bert-tiny-chinese-ws",
    files: &[
        ModelFile {
            name: "config.json",
            url: ckip_url!("config.json"),
            sha256: "ce3c02e59fd3456a2d1cda726c3fbf86dfaa4bc544590aceaa0c29d5ac60d463",
            size: 861,
            unpack: Unpack::Keep,
        },
        ModelFile {
            name: "vocab.txt",
            url: ckip_url!("vocab.txt"),
            sha256: "45bbac6b341c319adc98a532532882e91a9cefc0329aa57bac9ae761c27b291c",
            size: 109_540,
            unpack: Unpack::Keep,
        },
        ModelFile {
            name: "pytorch_model.bin",
            url: ckip_url!("pytorch_model.bin"),
            sha256: "29d3ccfe939242b1691e8293d0cd30b5fd6b80333bf11967f22a6a7d5a213189",
            size: 45_802_231,
            unpack: Unpack::Keep,
        },
    ],
};

macro_rules! kiwi_url {
    ($f:literal) => {
        concat!(
            "https://github.com/bab2min/Kiwi/releases/download/v0.24.1/",
            $f
        )
    };
}

macro_rules! kiwi_model_files {
    ($($f:literal = $size:literal),* $(,)?) => {
        &[$(Member { path: concat!("models/cong/base/", $f), name: $f, size: $size }),*]
    };
}

/// Kiwi's CoNg base model, the same on every platform.
const KIWI_MODEL: ModelFile = ModelFile {
    name: "kiwi_model_v0.24.1_base.tgz",
    url: kiwi_url!("kiwi_model_v0.24.1_base.tgz"),
    sha256: "50d163e1af817444ef21f7ec5ea07ef69fb347282d57a6b49260f2039d4dc346",
    size: 87_967_233,
    unpack: Unpack::TarGz(kiwi_model_files![
        "combiningRule.txt" = 3_611,
        "cong.mdl" = 75_854_705,
        "default.dict" = 3_090_954,
        "dialect.dict" = 644,
        "extract.mdl" = 17_370,
        "multi.dict" = 12_064_440,
        "nounchr.mdl" = 9_734_234,
        "sj.morph" = 8_462_892,
        "typo.dict" = 395,
    ]),
};

/// Kiwi's C library: this platform's release build and the file taken out of
/// it. Only Windows x64 and macOS arm64 are wired up; elsewhere [`KIWI_LIB`]
/// is `None` and Korean books can't be analysed.
#[cfg(all(windows, target_arch = "x86_64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("kiwi.dll");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_win_x64_v0.24.1.zip",
            url: kiwi_url!("kiwi_win_x64_v0.24.1.zip"),
            sha256: "857576c5e8492ec66f8327e35572b20f286b092a8e760fe780ace1a99afc3b30",
            size: 38_261_195,
            unpack: Unpack::Zip(&[Member {
                path: "lib/kiwi.dll",
                name: "kiwi.dll",
                size: 23_307_264,
            }]),
        },
        KIWI_MODEL,
    ];
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("libkiwi.dylib");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_mac_arm64_v0.24.1.tgz",
            url: kiwi_url!("kiwi_mac_arm64_v0.24.1.tgz"),
            sha256: "510680643980c4af94bee57151020b6373a098bac809084e7cb26080fc5bd9b9",
            size: 41_371_847,
            // libkiwi.dylib and libkiwi.0.dylib are symlinks to this file.
            unpack: Unpack::TarGz(&[Member {
                path: "lib/libkiwi.0.24.1.dylib",
                name: "libkiwi.dylib",
                size: 28_742_272,
            }]),
        },
        KIWI_MODEL,
    ];
}

#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = None;
    pub const FILES: &[ModelFile] = &[KIWI_MODEL];
}

/// File name of Kiwi's C library in [`KIWI`]'s folder; `None` on platforms
/// without a build of it.
pub const KIWI_LIB: Option<&str> = kiwi_lib::NAME;

/// Kiwi Korean morphological analyser (library + CoNg base model). The
/// library is loaded at run time (see `crate::kiwi`), so a build never needs it.
pub const KIWI: Model = Model {
    id: "kiwi-v0.24.1",
    title: "Kiwi v0.24.1 Korean morphological analyser (bab2min)",
    licence: "Apache-2.0",
    homepage: "https://github.com/bab2min/Kiwi",
    files: kiwi_lib::FILES,
};

/// SudachiDict full 20261015 from its GitHub release, the dictionary of the
/// Sudachi Japanese analyser (see `crate::japanese`). Full rather than core:
/// it adds proper nouns and set phrases, so more names and nicknames stay
/// whole (なっちゃん, 持崎湯葉). The version hanstat was tested with, and the fallback when the newest release can't be found or
/// doesn't load (see [`sudachi`]). GitHub is used rather than Works
/// Applications' S3 bucket, which serves the same file far more slowly
/// outside Japan.
pub const SUDACHI: Model = Model {
    id: "sudachidict-full-20261015",
    title: "SudachiDict full 20261015 (Works Applications)",
    licence: "Apache-2.0",
    homepage: "https://github.com/WorksApplications/SudachiDict",
    files: &[ModelFile {
        name: "sudachi-dictionary-20261015-full.zip",
        url: "https://github.com/WorksApplications/SudachiDict/releases/download/v20261015/sudachi-dictionary-20261015-full.zip",
        sha256: "b38318ca26a444cd713a5a377a353308ef38c733c64cc0be905122527516c793",
        size: 137_444_817,
        unpack: Unpack::Zip(&[
            Member {
                path: "sudachi-dictionary-20261015/system_full.dic",
                name: "system_full.dic",
                size: 330_506_339,
            },
            Member {
                path: "sudachi-dictionary-20261015/LICENSE-2.0.txt",
                name: "LICENSE-2.0.txt",
                size: 11_358,
            },
            Member {
                path: "sudachi-dictionary-20261015/LEGAL",
                name: "LEGAL",
                size: 6_037,
            },
        ]),
    }],
};

const SUDACHI_REPO: &str = "WorksApplications/SudachiDict";

/// How often to ask GitHub for a newer SudachiDict.
const SUDACHI_CHECK_SECS: u64 = 24 * 60 * 60;

/// The last answer from GitHub, kept in the model cache so that it is asked
/// at most once a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SudachiRelease {
    /// Unix time of the check.
    checked: u64,
    /// `20261015`.
    version: String,
    url: String,
    sha256: String,
    size: u64,
}

/// SudachiDict full from its newest GitHub release that has a full zip
/// (some releases have only Python wheels). GitHub is asked at most once a
/// day; without an answer, the newest dictionary already downloaded is used,
/// and failing that the tested [`SUDACHI`]. Resolved once per run.
pub fn sudachi() -> &'static Model {
    static M: OnceLock<&'static Model> = OnceLock::new();
    M.get_or_init(|| {
        let state = cache_root().join("sudachidict-latest.json");
        let saved: Option<SudachiRelease> = std::fs::read(&state)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let fresh = saved
            .as_ref()
            .filter(|s| now.saturating_sub(s.checked) < SUDACHI_CHECK_SECS)
            .cloned();
        let release = fresh.or_else(|| match latest_sudachi_release(now) {
            Ok(r) => {
                let _ = std::fs::create_dir_all(cache_root());
                let _ = std::fs::write(&state, serde_json::to_vec_pretty(&r).unwrap());
                Some(r)
            }
            Err(_) => saved,
        });
        match release {
            Some(r) if r.version == "20261015" => &SUDACHI,
            Some(r) => leak_sudachi_model(&r),
            None => sudachi_local(),
        }
    })
}

/// The newest SudachiDict already downloaded, or [`SUDACHI`]; never asks
/// the network (for display and quick checks).
pub fn sudachi_local() -> &'static Model {
    static M: OnceLock<&'static Model> = OnceLock::new();
    M.get_or_init(|| {
        let newest = std::fs::read_dir(cache_root())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let v = name.strip_prefix("sudachidict-full-")?.to_owned();
                (v.len() == 8
                    && v.bytes().all(|b| b.is_ascii_digit())
                    && e.path().join("system_full.dic").is_file())
                .then_some(v)
            })
            .max();
        match newest {
            Some(v) if v.as_str() > "20261015" => leak_sudachi_model(&SudachiRelease {
                checked: 0,
                url: sudachi_url(&v),
                version: v,
                // Already unpacked, so never downloaded again.
                sha256: String::new(),
                size: 0,
            }),
            _ => &SUDACHI,
        }
    })
}

fn sudachi_url(version: &str) -> String {
    format!(
        "https://github.com/{SUDACHI_REPO}/releases/download/v{version}/sudachi-dictionary-{version}-full.zip"
    )
}

#[derive(Deserialize)]
struct GhRelease {
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// `sha256:<hex>`, computed by GitHub.
    digest: Option<String>,
}

/// The newest release with a full zip and a SHA-256, from the list GitHub
/// returns newest first.
fn parse_sudachi_releases(json: &str, now: u64) -> Result<SudachiRelease, String> {
    let rs: Vec<GhRelease> =
        serde_json::from_str(json).map_err(|e| format!("Unexpected GitHub response: {e}"))?;
    let prefix = format!("https://github.com/{SUDACHI_REPO}/releases/download/");
    rs.iter()
        .filter(|r| !r.draft && !r.prerelease)
        .flat_map(|r| &r.assets)
        .find_map(|a| {
            let v = a
                .name
                .strip_prefix("sudachi-dictionary-")?
                .strip_suffix("-full.zip")?;
            let sha = a.digest.as_deref()?.strip_prefix("sha256:")?;
            (v.len() == 8
                && v.bytes().all(|b| b.is_ascii_digit())
                && sha.len() == 64
                && a.browser_download_url.starts_with(&prefix))
            .then(|| SudachiRelease {
                checked: now,
                version: v.to_owned(),
                url: a.browser_download_url.clone(),
                sha256: sha.to_ascii_lowercase(),
                size: a.size,
            })
        })
        .ok_or_else(|| "No SudachiDict release with a full dictionary zip".to_owned())
}

fn latest_sudachi_release(now: u64) -> Result<SudachiRelease, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into();
    let json = agent
        .get(format!(
            "https://api.github.com/repos/{SUDACHI_REPO}/releases?per_page=10"
        ))
        .header("User-Agent", concat!("hanstat/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_sudachi_releases(&json, now)
}

/// A [`Model`] for a SudachiDict release found at run time (kept for the
/// rest of the run).
fn leak_sudachi_model(r: &SudachiRelease) -> &'static Model {
    let s = |x: String| -> &'static str { Box::leak(x.into_boxed_str()) };
    let v = &r.version;
    let members: &'static [Member] = Box::leak(Box::new([
        Member {
            path: s(format!("sudachi-dictionary-{v}/system_full.dic")),
            name: "system_full.dic",
            size: 0,
        },
        Member {
            path: s(format!("sudachi-dictionary-{v}/LICENSE-2.0.txt")),
            name: "LICENSE-2.0.txt",
            size: 0,
        },
        Member {
            path: s(format!("sudachi-dictionary-{v}/LEGAL")),
            name: "LEGAL",
            size: 0,
        },
    ]));
    let files: &'static [ModelFile] = Box::leak(Box::new([ModelFile {
        name: s(format!("sudachi-dictionary-{v}-full.zip")),
        url: s(r.url.clone()),
        sha256: s(r.sha256.clone()),
        size: r.size,
        unpack: Unpack::Zip(members),
    }]));
    Box::leak(Box::new(Model {
        id: s(format!("sudachidict-full-{v}")),
        title: s(format!("SudachiDict full {v} (Works Applications)")),
        licence: "Apache-2.0",
        homepage: "https://github.com/WorksApplications/SudachiDict",
        files,
    }))
}

/// hanstat's per-user data folder (`%LOCALAPPDATA%\hanstat`, on macOS
/// `~/Library/Application Support/hanstat`): models and saved settings.
pub fn app_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("hanstat")
}

/// Root cache directory for all models.
pub fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("HANSTAT_MODEL_DIR") {
        return PathBuf::from(d);
    }
    app_dir().join("models")
}

impl Model {
    pub fn dir(&self) -> PathBuf {
        cache_root().join(self.id)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// True if every file is present with the expected size (hash checked on download).
    pub fn is_cached(&self) -> bool {
        let d = self.dir();
        self.files.iter().all(|f| f.is_present(&d))
    }

    /// Download any missing files. `progress(done_bytes, total_bytes)`.
    pub fn ensure(&self, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
        let dir = self.dir();
        if self.is_cached() {
            return Ok(dir);
        }
        // One download at a time per process; re-check after waiting, since
        // another thread may have just finished the same files.
        let _guard = DOWNLOAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_cached() {
            return Ok(dir);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let total = self.total_size();
        let mut done_before = 0u64;
        for f in self.files {
            if f.is_present(&dir) {
                done_before += f.size;
                continue;
            }
            let dest = dir.join(f.name);
            // An archive left by an interrupted unpack was verified before it was renamed.
            if !std::fs::metadata(&dest).is_ok_and(|m| m.len() == f.size) {
                download(f, &dest, &|d| progress(done_before + d, total)).map_err(|e| {
                    format!(
                        "Downloading {} for {} failed: {e}\nURL: {}",
                        f.name, self.title, f.url
                    )
                })?;
            }
            match f.unpack {
                Unpack::Keep => {}
                Unpack::Zip(ms) => {
                    unpack_zip(&dest, &dir, ms)
                        .map_err(|e| format!("Unpacking {} failed: {e}", f.name))?;
                    let _ = std::fs::remove_file(&dest);
                }
                Unpack::TarGz(ms) => {
                    unpack_tar_gz(&dest, &dir, ms)
                        .map_err(|e| format!("Unpacking {} failed: {e}", f.name))?;
                    let _ = std::fs::remove_file(&dest);
                }
            }
            done_before += f.size;
        }
        progress(total, total);
        Ok(dir)
    }
}

/// Write one archive member to `dir/m.name` (via a temporary name), checking its size.
fn save_member(mut r: impl Read, dir: &Path, m: &Member) -> Result<(), String> {
    let dest = dir.join(m.name);
    let tmp = dest.with_extension(format!("{}.part", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let n = std::io::copy(&mut r, &mut out).map_err(|e| e.to_string())?;
    drop(out);
    if m.size != 0 && n != m.size {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: expected {} bytes, got {n}", m.path, m.size));
    }
    std::fs::rename(&tmp, &dest).or_else(|e| {
        let _ = std::fs::remove_file(&tmp);
        // Another process unpacked the same file first.
        if std::fs::metadata(&dest).is_ok_and(|x| m.size == 0 || x.len() == m.size) {
            Ok(())
        } else {
            Err(e.to_string())
        }
    })
}

fn unpack_zip(archive: &Path, dir: &Path, members: &[Member]) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    for m in members {
        let entry = z.by_name(m.path).map_err(|e| format!("{}: {e}", m.path))?;
        save_member(entry, dir, m)?;
    }
    Ok(())
}

fn unpack_tar_gz(archive: &Path, dir: &Path, members: &[Member]) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(f)));
    let mut found = vec![false; members.len()];
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path = path.to_string_lossy().replace('\\', "/");
        let path = path.trim_start_matches("./");
        if let Some(i) = members.iter().position(|m| m.path == path) {
            save_member(entry, dir, &members[i])?;
            found[i] = true;
        }
    }
    match members.iter().zip(&found).find(|(_, f)| !**f) {
        Some((m, _)) => Err(format!("{} is missing from the archive", m.path)),
        None => Ok(()),
    }
}

static DOWNLOAD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn download(f: &ModelFile, dest: &std::path::Path, progress: &dyn Fn(u64)) -> Result<(), String> {
    if f.sha256.is_empty() {
        return Err("no checksum known for this file".to_owned());
    }
    let mut resp = ureq::get(f.url)
        .header("User-Agent", concat!("hanstat/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| e.to_string())?;
    // Per-process temp name so concurrent processes never share a partial file.
    let tmp = dest.with_extension(format!("{}.part", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().as_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n_total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        n_total += n as u64;
        progress(n_total);
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != f.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "checksum mismatch (expected {}, got {got}, {n_total} bytes)",
            f.sha256
        ));
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        // Another process finished the same verified file first: that's fine.
        Err(_) if std::fs::metadata(dest).is_ok_and(|m| m.len() == f.size) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_sudachi_release_with_a_full_zip() {
        let sha = "b38318ca26a444cd713a5a377a353308ef38c733c64cc0be905122527516c793";
        let url = |t: &str, n: &str| {
            format!("https://github.com/WorksApplications/SudachiDict/releases/download/{t}/{n}")
        };
        // Newest first; the newest has only wheels, a prerelease is skipped.
        let json = serde_json::json!([
            {"draft": false, "prerelease": true, "assets": [
                {"name": "sudachi-dictionary-20270101-full.zip", "size": 1,
                 "browser_download_url": url("v20270101", "sudachi-dictionary-20270101-full.zip"),
                 "digest": format!("sha256:{sha}")}]},
            {"draft": false, "prerelease": false, "assets": [
                {"name": "sudachidict_core-20261201-py3-none-any.whl", "size": 1,
                 "browser_download_url": url("v20261201", "sudachidict_core-20261201-py3-none-any.whl"),
                 "digest": format!("sha256:{sha}")}]},
            {"draft": false, "prerelease": false, "assets": [
                {"name": "sudachi-dictionary-20261015-core.zip", "size": 2,
                 "browser_download_url": url("v20261015", "sudachi-dictionary-20261015-core.zip"),
                 "digest": format!("sha256:{sha}")},
                {"name": "sudachi-dictionary-20261015-full.zip", "size": 137444817,
                 "browser_download_url": url("v20261015", "sudachi-dictionary-20261015-full.zip"),
                 "digest": format!("sha256:{}", sha.to_uppercase())}]}
        ])
        .to_string();
        let r = parse_sudachi_releases(&json, 7).unwrap();
        assert_eq!(r.version, "20261015");
        assert_eq!(r.size, 137_444_817);
        assert_eq!(r.sha256, sha);
        assert_eq!(r.checked, 7);
        assert_eq!(r.url, sudachi_url("20261015"));
        // An asset hosted elsewhere, or without a digest, is not used.
        let bad = json.replace(
            "https://github.com/WorksApplications/SudachiDict/",
            "https://example.com/",
        );
        assert!(parse_sudachi_releases(&bad, 0).is_err());
    }

    #[test]
    fn runtime_sudachi_model() {
        let m = leak_sudachi_model(&SudachiRelease {
            checked: 0,
            version: "20270101".into(),
            url: sudachi_url("20270101"),
            sha256: "ab".repeat(32),
            size: 9,
        });
        assert_eq!(m.id, "sudachidict-full-20270101");
        assert_eq!(m.files[0].name, "sudachi-dictionary-20270101-full.zip");
        let Unpack::Zip(ms) = m.files[0].unpack else {
            panic!()
        };
        assert_eq!(ms[0].path, "sudachi-dictionary-20270101/system_full.dic");
        assert_eq!(ms[0].name, "system_full.dic");
    }
}
