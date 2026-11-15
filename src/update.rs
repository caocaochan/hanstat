//! Self-update from GitHub releases.
//!
//! The latest release is looked up through the GitHub API. Its zip is
//! downloaded into memory and checked against the release's `.sha256` file.
//! Then each file in it replaces the one next to the running exe. Windows
//! lets a running exe be renamed but not overwritten, so the old file is
//! renamed to `<name>.<pid>.hanstat-old` first and deleted at the next start.
//! Every rename is rolled back if any step fails, so a failed update leaves the
//! old version in place.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const REPO: &str = "caocaochan/hanstat";
pub const RELEASES_PAGE: &str = "https://github.com/caocaochan/hanstat/releases/latest";
const OLD_SUFFIX: &str = "hanstat-old";
const NEW_SUFFIX: &str = "hanstat-new";
const USER_AGENT: &str = concat!("hanstat/", env!("CARGO_PKG_VERSION"));

pub type Version = (u64, u64, u64);

/// `1.2.3` or `v1.2.3`; anything else (pre-release suffixes included) is `None`.
pub fn parse_version(s: &str) -> Option<Version> {
    let mut it = s.strip_prefix('v').unwrap_or(s).split('.');
    let v = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    it.next().is_none().then_some(v)
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("Cargo.toml version is x.y.z")
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    /// `v1.2.3`
    pub tag: String,
    /// Release notes (the version's CHANGELOG.md section), as Markdown.
    pub notes: String,
    pub zip_url: String,
    pub sha256_url: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// The release zip's asset name. It has no version in it, so
/// `.../releases/latest/download/<this>` is a permanent link.
const ZIP_NAME: &str = "hanstat-x86_64-pc-windows-msvc.zip";

/// Parse the `releases/latest` API response. Asset URLs must point at this
/// repository's own release downloads.
fn parse_release(json: &str) -> Result<Release, String> {
    let r: ApiRelease =
        serde_json::from_str(json).map_err(|e| format!("Unexpected GitHub response: {e}"))?;
    if r.draft || r.prerelease {
        return Err(format!("{} is not a final release", r.tag_name));
    }
    let version = parse_version(&r.tag_name)
        .ok_or_else(|| format!("Unrecognised release tag {:?}", r.tag_name))?;
    let zip_name = ZIP_NAME;
    let sha_name = format!("{zip_name}.sha256");
    let prefix = format!(
        "https://github.com/{REPO}/releases/download/{}/",
        r.tag_name
    );
    let find = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name == name && a.browser_download_url == format!("{prefix}{name}"))
            .ok_or_else(|| format!("Release {} has no {name}", r.tag_name))
    };
    let zip = find(zip_name)?;
    let sha = find(&sha_name)?;
    Ok(Release {
        version,
        tag: r.tag_name.clone(),
        notes: r.body.unwrap_or_default().trim().to_owned(),
        zip_url: zip.browser_download_url.clone(),
        sha256_url: sha.browser_download_url.clone(),
        size: zip.size,
    })
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("{url}: {e}"))
}

/// The latest release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let json = get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?
    .body_mut()
    .read_to_string()
    .map_err(|e| e.to_string())?;
    let rel = parse_release(&json)?;
    Ok((rel.version > current_version()).then_some(rel))
}

/// The hash from a `sha256sum`-style line: `<hex>  <file name>`.
fn parse_sha256_file(text: &str, file: &str) -> Result<String, String> {
    let text = text.trim_start_matches('\u{feff}');
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(name)) = (parts.next(), parts.next())
            && name.trim_start_matches('*') == file
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(hash.to_ascii_lowercase());
        }
    }
    Err(format!(
        "No SHA-256 for {file} in the release's checksum file"
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download the release zip and verify it against the release's `.sha256`.
/// `progress(done_bytes, total_bytes)`.
pub fn download(rel: &Release, progress: &dyn Fn(u64, u64)) -> Result<Vec<u8>, String> {
    let zip_name = ZIP_NAME;
    let sha_text = get(&rel.sha256_url)?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let want = parse_sha256_file(&sha_text, zip_name)?;

    let mut resp = get(&rel.zip_url)?;
    let mut reader = resp.body_mut().as_reader();
    let mut data = Vec::with_capacity(rel.size as usize);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress(data.len() as u64, rel.size.max(data.len() as u64));
    }
    let got = hex(&Sha256::digest(&data));
    if got != want {
        return Err(format!(
            "Checksum mismatch for {zip_name} (expected {want}, got {got}, {} bytes)",
            data.len()
        ));
    }
    Ok(data)
}

/// Path of the running exe, captured once: after an update renames it, the
/// OS may report either name, and a restart must launch the original path.
pub fn exe_path() -> Result<&'static Path, String> {
    static EXE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    EXE.get_or_init(|| {
        std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .map_err(|e| format!("Cannot locate the running exe: {e}"))
    })
    .as_ref()
    .map(PathBuf::as_path)
    .map_err(Clone::clone)
}

/// Why this copy shouldn't update itself, if it shouldn't: a debug build, a
/// build run from Cargo's `target` folder, or `HANSTAT_NO_UPDATE` set.
pub fn disabled_reason() -> Option<String> {
    if cfg!(debug_assertions) {
        return Some("Debug build: updates are off.".into());
    }
    if std::env::var_os("HANSTAT_NO_UPDATE").is_some() {
        return Some("HANSTAT_NO_UPDATE is set: updates are off.".into());
    }
    match exe_path() {
        Err(e) => Some(e),
        Ok(p) => {
            let in_target = p
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n.eq_ignore_ascii_case("target"));
            in_target.then(|| "Running from a Cargo build folder: updates are off.".into())
        }
    }
}

/// A plain file name that can safely be joined onto the install folder.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'])
}

/// The release's files: `(file name, contents)`. The zip holds one top-level
/// folder (`hanstat/`; `hanstat-<tag>-<target>/` up to 0.3.1) with the files
/// directly inside it. The folder's name doesn't matter here.
fn release_files(zip: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut ar =
        zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("Bad release zip: {e}"))?;
    let mut out = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar
            .by_index(i)
            .map_err(|e| format!("Bad release zip: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let name = f
            .name()
            .map_err(|e| format!("Bad release zip: {e}"))?
            .into_owned();
        // Compress-Archive has written both `/` and `\` separators over time.
        let parts: Vec<&str> = name.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        let base = match parts.as_slice() {
            [_, base] | [base] => *base,
            _ => continue, // deeper folders: none today; ignore rather than guess
        };
        if !safe_file_name(base) {
            return Err(format!("Release zip has an unsafe file name: {name:?}"));
        }
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)
            .map_err(|e| format!("Bad release zip ({name}): {e}"))?;
        out.push((base.to_owned(), data));
    }
    if !out
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("hanstat.exe"))
    {
        return Err("Release zip has no hanstat.exe".into());
    }
    Ok(out)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(suffix);
    PathBuf::from(s)
}

/// Install the release zip's files into `dir`. The zip's `hanstat.exe` is
/// written as `gui_name` (the running exe's file name, in case it was renamed).
pub fn install(zip: &[u8], dir: &Path, gui_name: &str) -> Result<(), String> {
    let mut files = release_files(zip)?;
    for (name, _) in &mut files {
        if name.eq_ignore_ascii_case("hanstat.exe") {
            *name = gui_name.to_owned();
        }
    }
    let pid = std::process::id();
    let err = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());

    // Stage every file first so a full disk or a permissions problem stops the
    // update before anything is replaced.
    let mut staged = Vec::new();
    let stage = |staged: &mut Vec<PathBuf>| -> Result<(), String> {
        for (name, data) in &files {
            let new = with_suffix(&dir.join(name), NEW_SUFFIX);
            std::fs::write(&new, data).map_err(|e| err("Cannot write", &new, e))?;
            staged.push(new);
        }
        Ok(())
    };
    if let Err(e) = stage(&mut staged) {
        for p in &staged {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }

    // Swap them in. `done` records each finished swap for the rollback:
    // (installed path, where the previous file went, if there was one).
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut result = Ok(());
    for (name, _) in &files {
        let dest = dir.join(name);
        let new = with_suffix(&dest, NEW_SUFFIX);
        let old = with_suffix(&dest, &format!("{pid}.{OLD_SUFFIX}"));
        let moved = if dest.exists() {
            if let Err(e) = std::fs::rename(&dest, &old) {
                result = Err(err("Cannot move aside", &dest, e));
                break;
            }
            Some(old)
        } else {
            None
        };
        if let Err(e) = std::fs::rename(&new, &dest) {
            if let Some(old) = &moved {
                let _ = std::fs::rename(old, &dest);
            }
            result = Err(err("Cannot replace", &dest, e));
            break;
        }
        done.push((dest, moved));
    }
    if result.is_err() {
        for (dest, old) in done.iter().rev() {
            match old {
                Some(old) => {
                    let _ = std::fs::rename(old, dest);
                }
                None => {
                    let _ = std::fs::remove_file(dest);
                }
            }
        }
        for p in &staged {
            let _ = std::fs::remove_file(p);
        }
    }
    result
}

/// Delete files an earlier update left in `dir`. The previous exe is still
/// running for a moment after the restart, so keep trying for a few seconds.
pub fn clean_up(dir: &Path) {
    for _ in 0..40 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut left = false;
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(&format!(".{OLD_SUFFIX}"))
                || name.ends_with(&format!(".{NEW_SUFFIX}"))
            {
                left |= std::fs::remove_file(e.path()).is_err();
            }
        }
        if !left {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Saved in `%LOCALAPPDATA%\hanstat\update.json`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Check at startup and install a newer release in the background.
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_update: true }
    }
}

impl Settings {
    fn path() -> PathBuf {
        crate::models::app_dir().join("update.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_vec_pretty(self).unwrap())
            .map_err(|e| format!("Cannot save {}: {e}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.3.1"), Some((0, 3, 1)));
        assert_eq!(parse_version("10.20.30"), Some((10, 20, 30)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("v1.2.3-rc1"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
        assert_eq!(
            current_version(),
            parse_version(env!("CARGO_PKG_VERSION")).unwrap()
        );
    }

    fn api_json(tag: &str, repo: &str) -> String {
        let zip = ZIP_NAME;
        serde_json::json!({
            "tag_name": tag,
            "body": "### Fixed\n- Things.\n",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": zip, "size": 123,
                 "browser_download_url": format!("https://github.com/{repo}/releases/download/{tag}/{zip}")},
                {"name": format!("{zip}.sha256"), "size": 99,
                 "browser_download_url": format!("https://github.com/{repo}/releases/download/{tag}/{zip}.sha256")},
            ]
        })
        .to_string()
    }

    #[test]
    fn release_parsing() {
        let r = parse_release(&api_json("v9.8.7", REPO)).unwrap();
        assert_eq!(r.version, (9, 8, 7));
        assert_eq!(r.size, 123);
        assert_eq!(r.notes, "### Fixed\n- Things.");
        assert!(
            r.zip_url
                .ends_with("/releases/download/v9.8.7/hanstat-x86_64-pc-windows-msvc.zip")
        );
        // Assets hosted anywhere else are refused.
        assert!(parse_release(&api_json("v9.8.7", "someone/else")).is_err());
        assert!(parse_release(&api_json("nightly", REPO)).is_err());
    }

    #[test]
    fn sha256_file() {
        let h = "AB".repeat(32);
        let text = format!("\u{feff}{h}  hanstat-v1.0.0-x.zip\r\n");
        assert_eq!(
            parse_sha256_file(&text, "hanstat-v1.0.0-x.zip").unwrap(),
            h.to_lowercase()
        );
        assert!(parse_sha256_file(&text, "other.zip").is_err());
        assert!(parse_sha256_file("abc  f.zip", "f.zip").is_err());
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("hanstat-update-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn install_replaces_files_and_cleans_up() {
        let dir = temp_dir("ok");
        std::fs::write(dir.join("my-hanstat.exe"), b"old gui").unwrap();
        std::fs::write(dir.join("hanstat-cli.exe"), b"old cli").unwrap();
        std::fs::write(dir.join("notes.txt"), b"user file").unwrap();
        let top = "hanstat-v9.9.9-x86_64-pc-windows-msvc";
        let zip = make_zip(&[
            (&format!("{top}/hanstat.exe"), b"new gui"),
            (&format!("{top}\\hanstat-cli.exe"), b"new cli"),
            (&format!("{top}/README.md"), b"readme"),
        ]);
        install(&zip, &dir, "my-hanstat.exe").unwrap();
        assert_eq!(
            std::fs::read(dir.join("my-hanstat.exe")).unwrap(),
            b"new gui"
        );
        assert_eq!(
            std::fs::read(dir.join("hanstat-cli.exe")).unwrap(),
            b"new cli"
        );
        assert_eq!(std::fs::read(dir.join("README.md")).unwrap(), b"readme");
        assert!(!dir.join("hanstat.exe").exists());
        assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"user file");
        let old = dir.join(format!(
            "my-hanstat.exe.{}.{OLD_SUFFIX}",
            std::process::id()
        ));
        assert_eq!(std::fs::read(&old).unwrap(), b"old gui");
        clean_up(&dir);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "README.md",
                "hanstat-cli.exe",
                "my-hanstat.exe",
                "notes.txt"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_rolls_back_on_failure() {
        let dir = temp_dir("fail");
        std::fs::write(dir.join("hanstat.exe"), b"old gui").unwrap();
        std::fs::write(dir.join("README.md"), b"old readme").unwrap();
        // A non-empty folder where README.md would be moved aside makes the
        // second swap fail after the first succeeded.
        let blocker = dir.join(format!("README.md.{}.{OLD_SUFFIX}", std::process::id()));
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("x"), b"x").unwrap();
        let zip = make_zip(&[("t/hanstat.exe", b"new gui"), ("t/README.md", b"readme")]);
        assert!(install(&zip, &dir, "hanstat.exe").is_err());
        assert_eq!(std::fs::read(dir.join("hanstat.exe")).unwrap(), b"old gui");
        assert_eq!(std::fs::read(dir.join("README.md")).unwrap(), b"old readme");
        std::fs::remove_dir_all(&blocker).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["README.md", "hanstat.exe"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsafe_or_incomplete_zips_are_refused() {
        let dir = temp_dir("bad");
        let zip = make_zip(&[("t/hanstat.exe", b"x"), ("t/a:b", b"x")]);
        assert!(install(&zip, &dir, "hanstat.exe").is_err());
        let zip = make_zip(&[("t/hanstat-cli.exe", b"x")]);
        assert!(install(&zip, &dir, "hanstat.exe").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
