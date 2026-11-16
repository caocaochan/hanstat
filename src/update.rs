//! Self-update from GitHub releases.
//!
//! The latest release is looked up through the GitHub API. Its zip is
//! downloaded into memory and checked against the release's `.sha256` file.
//! On Windows each file in it then replaces the one next to the running exe.
//! Windows lets a running exe be renamed but not overwritten, so the old file
//! is renamed to `<name>.<pid>.hanstat-old` first and deleted at the next start.
//! Every rename is rolled back if any step fails, so a failed update leaves the
//! old version in place.
//!
//! On macOS (from linlin56's PR #1) the zip holds `hanstat.app`, which
//! replaces the whole bundle the running exe is in: it is unpacked next to it
//! as `hanstat.app.hanstat-new`, the running bundle is renamed to
//! `hanstat.app.<pid>.hanstat-old` (macOS keeps running a moved executable)
//! and the new one takes its name. Writing new files rather than overwriting
//! the running ones also keeps the kernel's cached code signature valid.

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

/// Releases are versioned `yyyy.mm.dd.n` (n counts releases on that day from
/// 0), tagged without a `v`. Older releases (up to v0.7.1) were `v0.x.y`, read as `(0, x, y, 0)`,
/// so they sort before every dated one.
pub type Version = (u64, u64, u64, u64);

/// `2026.11.15.0`, or a legacy `0.8.0` / `v0.8.0`; anything else
/// (pre-release suffixes included) is `None`.
pub fn parse_version(s: &str) -> Option<Version> {
    let parts: Vec<u64> = s
        .strip_prefix('v')
        .unwrap_or(s)
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    match parts[..] {
        [y, m, d, n] if (1..=12).contains(&m) && (1..=31).contains(&d) => Some((y, m, d, n)),
        [a, b, c] => Some((a, b, c, 0)),
        _ => None,
    }
}

/// Cargo needs a SemVer version without leading zeros, so Cargo.toml holds
/// `yyyy.Mdd.n` (2026.1115.0 for 2026.11.15.0, 2027.105.2 for 2027.01.05.2).
fn from_cargo(s: &str) -> Option<Version> {
    let mut it = s.split('.').map(|p| p.parse::<u64>().ok());
    let (y, md, n) = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    let (m, d) = (md / 100, md % 100);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d, n))
}

pub fn current_version() -> Version {
    from_cargo(env!("CARGO_PKG_VERSION")).expect("Cargo.toml version is yyyy.Mdd.n")
}

/// `2026.11.15.0` (legacy versions as `0.8.0`).
pub fn format_version((a, b, c, n): Version) -> String {
    if a == 0 {
        format!("{a}.{b}.{c}")
    } else {
        format!("{a}.{b:02}.{c:02}.{n}")
    }
}

/// This build's version as released, e.g. `2026.11.15.0`.
pub fn version_string() -> String {
    format_version(current_version())
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

/// The release zip's asset name for this platform. It has no version in it,
/// so `.../releases/latest/download/<this>` is a permanent link. Platforms
/// without a release of their own never update (see [`disabled_reason`]).
#[cfg(not(target_os = "macos"))]
const ZIP_NAME: &str = "hanstat-x86_64-pc-windows-msvc.zip";
#[cfg(target_os = "macos")]
const ZIP_NAME: &str = "hanstat-aarch64-apple-darwin.zip";

/// Whether this build's platform has a release to update to: Windows x64
/// and Apple silicon. An Intel Mac must not install the arm64 app.
const HAS_RELEASE: bool = cfg!(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
));

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

/// Why this copy shouldn't update itself, if it shouldn't: a platform with
/// no release (see [`HAS_RELEASE`]), a debug build, a build run from Cargo's
/// `target` folder, `HANSTAT_NO_UPDATE` set, or on macOS an exe that isn't
/// inside an `.app` bundle or that macOS runs from a read-only copy.
pub fn disabled_reason() -> Option<String> {
    if !HAS_RELEASE {
        return Some(
            "No release for this platform: updates are off. New versions are on hanstat's GitHub releases page."
                .into(),
        );
    }
    if cfg!(debug_assertions) {
        return Some("Debug build: updates are off.".into());
    }
    if std::env::var_os("HANSTAT_NO_UPDATE").is_some() {
        return Some("HANSTAT_NO_UPDATE is set: updates are off.".into());
    }
    let p = match exe_path() {
        Err(e) => return Some(e),
        Ok(p) => p,
    };
    let in_target = p
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|n| n.eq_ignore_ascii_case("target"));
    if in_target {
        return Some("Running from a Cargo build folder: updates are off.".into());
    }
    if cfg!(target_os = "macos") {
        if bundle_of(p).is_none() {
            return Some("Not running from hanstat.app: updates are off.".into());
        }
        if is_translocated(p) {
            return Some(
                "macOS is running hanstat from a read-only copy. Move hanstat.app to Applications to get updates."
                    .into(),
            );
        }
    }
    None
}

/// Whether macOS runs this app through App Translocation: a quarantined app
/// opened where it was unpacked runs from a randomised read-only mount, so
/// the bundle there can't be replaced.
fn is_translocated(exe: &Path) -> bool {
    exe.components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("AppTranslocation"))
}

/// Held while [`clean_up_here`] or [`install_here`] work in the install
/// folder: the startup clean-up deletes `*.hanstat-new`, which is exactly what
/// an update installed meanwhile stages.
static INSTALL_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Install a downloaded release zip over the running copy: the files next to
/// the exe on Windows, the `.app` bundle on macOS.
pub fn install_here(zip: &[u8]) -> Result<(), String> {
    let _lock = INSTALL_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let exe = exe_path()?;
    #[cfg(target_os = "macos")]
    {
        let bundle = bundle_of(exe)
            .ok_or_else(|| format!("{} is not inside an .app bundle", exe.display()))?;
        install_app(zip, bundle)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
            return Err(format!("Unexpected exe path {}", exe.display()));
        };
        install(zip, dir, &name.to_string_lossy())
    }
}

/// Delete what an earlier update left next to the running copy (see [`clean_up`]).
pub fn clean_up_here() {
    let _lock = INSTALL_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(exe) = exe_path() else { return };
    let dir = if cfg!(target_os = "macos") {
        bundle_of(exe).and_then(Path::parent)
    } else {
        exe.parent()
    };
    if let Some(dir) = dir {
        clean_up(dir);
    }
}

/// The `.app` bundle an exe is in: `<name>.app/Contents/MacOS/<exe>`.
fn bundle_of(exe: &Path) -> Option<&Path> {
    let macos = exe
        .parent()
        .filter(|p| p.file_name() == Some("MacOS".as_ref()))?;
    let contents = macos
        .parent()
        .filter(|p| p.file_name() == Some("Contents".as_ref()))?;
    contents
        .parent()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
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
#[cfg(any(not(target_os = "macos"), test))]
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
#[cfg(any(not(target_os = "macos"), test))]
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

/// The `hanstat.app` bundle's files in the release zip: `(path inside the
/// bundle, contents, Unix mode)`. Anything outside `hanstat.app/` is ignored
/// (`__MACOSX/` included); links and unsafe names are refused.
#[cfg(any(target_os = "macos", test))]
fn release_app_files(zip: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>, u32)>, String> {
    let bad = |e: zip::result::ZipError| format!("Bad release zip: {e}");
    let mut ar = zip::ZipArchive::new(Cursor::new(zip)).map_err(bad)?;
    let mut out = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(bad)?;
        let name = f.name().map_err(bad)?.into_owned();
        let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
        let Some((&"hanstat.app", rest)) = parts.split_first() else {
            continue;
        };
        if f.is_dir() || rest.is_empty() {
            continue;
        }
        if f.is_symlink() {
            return Err(format!("Release zip has a link: {name:?}"));
        }
        if !rest.iter().all(|p| safe_file_name(p)) {
            return Err(format!("Release zip has an unsafe file name: {name:?}"));
        }
        if rest.last().is_some_and(|p| p.starts_with("._")) {
            continue; // AppleDouble metadata, not part of the bundle
        }
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)
            .map_err(|e| format!("Bad release zip ({name}): {e}"))?;
        // Only two modes: executable (anything in Contents/MacOS, even if the
        // zip lost its mode bits, or marked executable) or not. A zip written
        // without Unix modes reports synthesised ones (0o775 for DOS entries).
        let executable = rest.starts_with(&["Contents", "MacOS"])
            || f.unix_mode().is_some_and(|m| m & 0o111 != 0);
        let mode = if executable { 0o755 } else { 0o644 };
        out.push((rest.iter().collect(), data, mode));
    }
    for needed in ["Contents/Info.plist", "Contents/MacOS/hanstat"] {
        if !out.iter().any(|(p, ..)| p == Path::new(needed)) {
            return Err(format!("Release zip has no hanstat.app/{needed}"));
        }
    }
    Ok(out)
}

/// Replace the `bundle` (`…/hanstat.app`) with the release zip's
/// `hanstat.app`. Either the new bundle is in place, or the old one is back
/// and nothing is left behind but, at worst, a `.hanstat-new` folder that
/// [`clean_up`] removes.
#[cfg(any(target_os = "macos", test))]
pub fn install_app(zip: &[u8], bundle: &Path) -> Result<(), String> {
    let files = release_app_files(zip)?;
    let err = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());
    let new = with_suffix(bundle, NEW_SUFFIX);
    let old = with_suffix(bundle, &format!("{}.{OLD_SUFFIX}", std::process::id()));

    let unpack = || -> Result<(), String> {
        if new.exists() {
            std::fs::remove_dir_all(&new).map_err(|e| err("Cannot remove", &new, e))?;
        }
        for (rel, data, mode) in &files {
            let dest = new.join(rel);
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d).map_err(|e| err("Cannot create", d, e))?;
            }
            std::fs::write(&dest, data).map_err(|e| err("Cannot write", &dest, e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(*mode);
                std::fs::set_permissions(&dest, perm).map_err(|e| err("Cannot set", &dest, e))?;
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        Ok(())
    };
    if let Err(e) = unpack() {
        let _ = std::fs::remove_dir_all(&new);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(bundle, &old) {
        let _ = std::fs::remove_dir_all(&new);
        return Err(err("Cannot move aside", bundle, e));
    }
    if let Err(e) = std::fs::rename(&new, bundle) {
        let _ = std::fs::rename(&old, bundle);
        let _ = std::fs::remove_dir_all(&new);
        return Err(err("Cannot replace", bundle, e));
    }
    Ok(())
}

/// Delete files (macOS: bundles) an earlier update left in `dir`. The
/// previous exe is still running for a moment after the restart, so keep
/// trying for a few seconds.
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
                // `file_type` doesn't follow links: a link is removed, never
                // what it points to.
                let removed = match e.file_type() {
                    Ok(t) if t.is_dir() => std::fs::remove_dir_all(e.path()),
                    _ => std::fs::remove_file(e.path()),
                };
                left |= removed.is_err();
            }
        }
        if !left {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Saved in `update.json` in [`crate::models::app_dir`].
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
        assert_eq!(parse_version("v0.3.1"), Some((0, 3, 1, 0)));
        assert_eq!(parse_version("10.20.30"), Some((10, 20, 30, 0)));
        assert_eq!(parse_version("2026.11.15.0"), Some((2026, 11, 15, 0)));
        assert_eq!(parse_version("2027.01.05.12"), Some((2027, 1, 5, 12)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("2026.13.01.0"), None);
        assert_eq!(parse_version("v1.2.3-rc1"), None);
        assert_eq!(parse_version("1.2.3.4.5"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
        assert!(parse_version("2026.11.15.0") > parse_version("v0.8.0"));
        assert!(parse_version("2026.11.15.1") > parse_version("2026.11.15.0"));
        assert!(parse_version("2026.11.16.0") > parse_version("2026.11.15.9"));
        assert!(parse_version("2027.01.01.0") > parse_version("2026.12.31.3"));

        assert_eq!(from_cargo("2026.1115.0"), Some((2026, 11, 15, 0)));
        assert_eq!(from_cargo("2027.105.2"), Some((2027, 1, 5, 2)));
        assert_eq!(from_cargo("0.8.0"), None);
        assert_eq!(format_version((2027, 1, 5, 2)), "2027.01.05.2");
        assert_eq!(format_version((0, 8, 0, 0)), "0.8.0");
        // The release tag (made by CI from Cargo.toml) round-trips.
        assert_eq!(parse_version(&version_string()), Some(current_version()));
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
        assert_eq!(r.version, (9, 8, 7, 0));
        let r = parse_release(&api_json("2026.11.15.1", REPO)).unwrap();
        assert_eq!(r.version, (2026, 11, 15, 1));
        let r = parse_release(&api_json("v9.8.7", REPO)).unwrap();
        assert_eq!(r.size, 123);
        assert_eq!(r.notes, "### Fixed\n- Things.");
        assert!(
            r.zip_url
                .ends_with(&format!("/releases/download/v9.8.7/{ZIP_NAME}"))
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

    fn make_app_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data, mode) in entries {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(*mode);
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn make_bundle(dir: &Path, exe: &[u8]) -> PathBuf {
        let app = dir.join("hanstat.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), b"old plist").unwrap();
        std::fs::write(app.join("Contents/MacOS/hanstat"), exe).unwrap();
        std::fs::write(app.join("Contents/MacOS/gone"), b"only in the old one").unwrap();
        app
    }

    fn app_zip() -> Vec<u8> {
        make_app_zip(&[
            ("hanstat.app/Contents/Info.plist", b"new plist", 0o644),
            ("hanstat.app/Contents/MacOS/hanstat", b"new gui", 0o755),
            ("hanstat.app/Contents/MacOS/hanstat-cli", b"new cli", 0o755),
            ("hanstat.app/Contents/MacOS/._hanstat", b"xattrs", 0o644),
            ("__MACOSX/hanstat.app/._Info.plist", b"xattrs", 0o644),
        ])
    }

    #[test]
    fn bundle_paths() {
        let exe = Path::new("/Applications/hanstat.app/Contents/MacOS/hanstat");
        assert_eq!(bundle_of(exe), Some(Path::new("/Applications/hanstat.app")));
        assert_eq!(bundle_of(Path::new("/usr/local/bin/hanstat")), None);
        assert_eq!(bundle_of(Path::new("/x/Contents/MacOS/hanstat")), None);
    }

    #[test]
    fn install_app_replaces_bundle_and_cleans_up() {
        let dir = temp_dir("app-ok");
        let app = make_bundle(&dir, b"old gui");
        install_app(&app_zip(), &app).unwrap();
        let exe = app.join("Contents/MacOS/hanstat");
        assert_eq!(std::fs::read(&exe).unwrap(), b"new gui");
        assert_eq!(
            std::fs::read(app.join("Contents/Info.plist")).unwrap(),
            b"new plist"
        );
        assert!(!app.join("Contents/MacOS/gone").exists());
        assert!(!app.join("Contents/MacOS/._hanstat").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&exe), 0o755);
            assert_eq!(mode(&app.join("Contents/Info.plist")), 0o644);
        }
        let old = dir.join(format!("hanstat.app.{}.{OLD_SUFFIX}", std::process::id()));
        assert_eq!(
            std::fs::read(old.join("Contents/MacOS/hanstat")).unwrap(),
            b"old gui"
        );
        std::fs::write(dir.join("notes.txt"), b"user file").unwrap();
        clean_up(&dir);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["hanstat.app", "notes.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_app_rolls_back_on_failure() {
        let dir = temp_dir("app-fail");
        let app = make_bundle(&dir, b"old gui");
        // A non-empty folder where the bundle would be moved aside.
        let blocker = dir.join(format!("hanstat.app.{}.{OLD_SUFFIX}", std::process::id()));
        std::fs::create_dir_all(blocker.join("x")).unwrap();
        assert!(install_app(&app_zip(), &app).is_err());
        assert_eq!(
            std::fs::read(app.join("Contents/MacOS/hanstat")).unwrap(),
            b"old gui"
        );
        assert!(!dir.join(format!("hanstat.app.{NEW_SUFFIX}")).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsafe_or_incomplete_app_zips_are_refused() {
        let dir = temp_dir("app-bad");
        let app = make_bundle(&dir, b"old gui");
        let no_exe = make_app_zip(&[("hanstat.app/Contents/Info.plist", b"p", 0o644)]);
        let unsafe_name = make_app_zip(&[
            ("hanstat.app/Contents/Info.plist", b"p", 0o644),
            ("hanstat.app/Contents/MacOS/hanstat", b"x", 0o755),
            ("hanstat.app/Contents/../../evil", b"x", 0o644),
        ]);
        let wrong_top = make_app_zip(&[
            ("other.app/Contents/Info.plist", b"p", 0o644),
            ("other.app/Contents/MacOS/hanstat", b"x", 0o755),
        ]);
        for zip in [no_exe, unsafe_name, wrong_top] {
            assert!(install_app(&zip, &app).is_err());
            assert_eq!(
                std::fs::read(app.join("Contents/MacOS/hanstat")).unwrap(),
                b"old gui"
            );
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["hanstat.app"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn programs_stay_executable_without_zip_modes() {
        // A zip written without Unix modes (DOS entries).
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [
            ("hanstat.app/Contents/Info.plist", &b"p"[..]),
            ("hanstat.app/Contents/MacOS/hanstat", &b"x"[..]),
        ] {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .system(zip::System::Dos);
            w.start_file(name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        let zip = w.finish().unwrap().into_inner();
        let files = release_app_files(&zip).unwrap();
        let mode = |p: &str| files.iter().find(|(q, ..)| q == Path::new(p)).unwrap().2;
        assert_eq!(mode("Contents/MacOS/hanstat"), 0o755);
        assert_eq!(mode("Contents/Info.plist"), 0o644);
    }

    #[test]
    fn translocated_paths() {
        let t = Path::new(
            "/private/var/folders/ab/T/AppTranslocation/1234/d/hanstat.app/Contents/MacOS/hanstat",
        );
        assert!(is_translocated(t));
        assert!(!is_translocated(Path::new(
            "/Applications/hanstat.app/Contents/MacOS/hanstat"
        )));
    }
}
