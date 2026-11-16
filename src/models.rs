//! Segmentation models are not bundled (their licences differ from this
//! project's). They are downloaded on first use from their official Hugging
//! Face repositories or GitHub releases, pinned to an exact commit or release
//! and verified by SHA-256, then cached in `%LOCALAPPDATA%\hanstat\models`
//! (override: `HANSTAT_MODEL_DIR`).

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
            std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == size)
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

/// Kiwi Korean morphological analyser (library + CoNg base model). The DLL is
/// loaded at run time (see `crate::kiwi`), so a build never needs it.
pub const KIWI: Model = Model {
    id: "kiwi-v0.24.1",
    title: "Kiwi v0.24.1 Korean morphological analyser (bab2min)",
    licence: "Apache-2.0",
    homepage: "https://github.com/bab2min/Kiwi",
    files: &[
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
        ModelFile {
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
        },
    ],
};

/// SudachiDict core, the dictionary of the Sudachi Japanese analyser (see
/// `crate::japanese`), from Works Applications' official download bucket.
pub const SUDACHI: Model = Model {
    id: "sudachidict-core-20261015",
    title: "SudachiDict core 20261015 (Works Applications)",
    licence: "Apache-2.0",
    homepage: "https://github.com/WorksApplications/SudachiDict",
    files: &[ModelFile {
        name: "sudachi-dictionary-20261015-core.zip",
        url: "https://sudachi.s3.ap-northeast-1.amazonaws.com/sudachidict/v1/sudachi-dictionary-20261015-core.zip",
        sha256: "5cf5cb7f1ea943a988e2fb5d285a9a8b3538861faefa7a2604c2cd18c387f503",
        size: 76_937_904,
        unpack: Unpack::Zip(&[
            Member {
                path: "sudachi-dictionary-20261015/system_core.dic",
                name: "system_core.dic",
                size: 202_236_003,
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

/// hanstat's per-user data folder (`%LOCALAPPDATA%\hanstat`): models and saved settings.
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("hanstat")
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
    if n != m.size {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: expected {} bytes, got {n}", m.path, m.size));
    }
    std::fs::rename(&tmp, &dest).or_else(|e| {
        let _ = std::fs::remove_file(&tmp);
        // Another process unpacked the same file first.
        if std::fs::metadata(&dest).is_ok_and(|x| x.len() == m.size) {
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
