//! Segmentation models are not bundled (their licences differ from this
//! project's). They are downloaded on first use from their official Hugging
//! Face repositories, pinned to an exact commit and verified by SHA-256, then
//! cached in `%LOCALAPPDATA%\hanstat\models` (override: `HANSTAT_MODEL_DIR`).

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
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
        },
        ModelFile {
            name: "vocab.txt",
            url: ckip_url!("vocab.txt"),
            sha256: "45bbac6b341c319adc98a532532882e91a9cefc0329aa57bac9ae761c27b291c",
            size: 109_540,
        },
        ModelFile {
            name: "pytorch_model.bin",
            url: ckip_url!("pytorch_model.bin"),
            sha256: "29d3ccfe939242b1691e8293d0cd30b5fd6b80333bf11967f22a6a7d5a213189",
            size: 45_802_231,
        },
    ],
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
        self.files
            .iter()
            .all(|f| std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.len() == f.size))
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
            let dest = dir.join(f.name);
            if std::fs::metadata(&dest).is_ok_and(|m| m.len() == f.size) {
                done_before += f.size;
                continue;
            }
            download(f, &dest, &|d| progress(done_before + d, total)).map_err(|e| {
                format!(
                    "Downloading {} for {} failed: {e}\nURL: {}",
                    f.name, self.title, f.url
                )
            })?;
            done_before += f.size;
        }
        progress(total, total);
        Ok(dir)
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
