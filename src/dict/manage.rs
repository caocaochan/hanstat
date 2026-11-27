//! The dictionaries' folders: putting an import or update in place (a swap
//! that is rolled back if any step fails), cleaning up after a crash, finding
//! a dictionary already installed, deleting.
//!
//! A dictionary's folder is only ever replaced whole: the new one is
//! written to `.tmp-<id>`, the old one renamed to `.old-<id>`, the new one
//! renamed in, and the old one deleted. The lookup worker must have dropped
//! the old one's maps first (Windows can't rename or delete a mapped file).

use super::format::{self, Entry, Manifest, Meta};
use super::import::Index;
use std::path::{Path, PathBuf};

pub fn tmp_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".tmp-{id}"))
}

pub fn old_dir(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".old-{id}"))
}

pub fn download_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".download-{id}.zip"))
}

/// The failure points of [`swap`], for tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fail {
    MoveOld,
    MoveNew,
}

fn rename_path(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Put `.tmp-<id>` in place as `<id>`. If any step fails, the old folder is
/// back where it was and the error is returned.
pub fn swap(root: &Path, id: &str) -> Result<(), String> {
    swap_impl(root, id, rename_path, rename_path)
}

#[cfg(test)]
fn swap_with(root: &Path, id: &str, fail: Fail) -> Result<(), String> {
    fn fail_rename(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("test failure"))
    }
    match fail {
        Fail::MoveOld => swap_impl(root, id, fail_rename, rename_path),
        Fail::MoveNew => swap_impl(root, id, rename_path, fail_rename),
    }
}

fn swap_impl(
    root: &Path,
    id: &str,
    move_old: fn(&Path, &Path) -> std::io::Result<()>,
    move_new: fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let (cur, tmp, old) = (root.join(id), tmp_dir(root, id), old_dir(root, id));
    if !tmp.join(format::META).exists() {
        return Err(format!(
            "{}: the new dictionary is incomplete",
            tmp.display()
        ));
    }
    let _ = std::fs::remove_dir_all(&old);
    let had_old = cur.exists();
    if had_old {
        move_old(&cur, &old).map_err(|e| format!("{}: {e}", cur.display()))?;
    }
    if let Err(e) = move_new(&tmp, &cur) {
        if had_old {
            let _ = std::fs::rename(&old, &cur);
        }
        return Err(format!("{}: {e}", cur.display()));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

/// After a crash: put back a dictionary left as `.old-<id>` with no `<id>`,
/// and delete unfinished imports and downloads.
pub fn recover(root: &Path) {
    let Ok(dir) = std::fs::read_dir(root) else {
        return;
    };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        if let Some(id) = name.strip_prefix(".old-") {
            if root.join(id).join(format::META).exists() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_dir_all(root.join(id));
                let _ = std::fs::rename(&p, root.join(id));
            }
        } else if name.starts_with(".tmp-") {
            let _ = std::fs::remove_dir_all(&p);
        } else if name.starts_with(".download-") {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The dictionaries with their metadata (`None`: unreadable or from another
/// format version).
pub fn list(root: &Path) -> Vec<(Entry, Option<Meta>)> {
    Manifest::load(root)
        .dicts
        .into_iter()
        .map(|e| {
            let m = read_meta(&root.join(&e.id));
            (e, m)
        })
        .collect()
}

pub fn read_meta(dir: &Path) -> Option<Meta> {
    serde_json::from_slice(&std::fs::read(dir.join(format::META)).ok()?).ok()
}

/// An installed dictionary the zip `index` would replace: the same update
/// URLs, or else the same title.
pub fn existing(root: &Path, index: &Index) -> Option<String> {
    let list = list(root);
    if let Some(u) = &index.update
        && let Some((e, _)) = list.iter().find(|(_, m)| {
            m.as_ref()
                .and_then(|m| m.update.as_ref())
                .is_some_and(|mu| mu.index_url == u.index_url || mu.download_url == u.download_url)
        })
    {
        return Some(e.id.clone());
    }
    list.iter()
        .find(|(_, m)| m.as_ref().is_some_and(|m| m.title == index.title))
        .map(|(e, _)| e.id.clone())
}

/// Changes to the list go through here, one at a time (imports and updates
/// finish on another thread than the window's).
static MANIFEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Change the list and save it.
pub fn edit_manifest(root: &Path, f: impl FnOnce(&mut Manifest)) -> Result<(), String> {
    let _g = MANIFEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = Manifest::load(root);
    f(&mut m);
    m.save(root)
}

/// Add `id` to the end of the list (enabled), or keep its place if it is there.
pub fn add_to_manifest(root: &Path, id: &str) -> Result<(), String> {
    edit_manifest(root, |m| {
        if let Some(d) = m.dicts.iter_mut().find(|d| d.id == id) {
            // A new revision: the last check's result no longer applies.
            d.check = None;
        } else {
            m.dicts.push(Entry {
                id: id.to_owned(),
                enabled: true,
                check: None,
                collapsed: false,
            });
        }
    })
}

/// Remove `id`'s folder and its place in the list. Its maps must be dropped.
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let dir = root.join(id);
    if dir.exists() {
        // Renamed first, so a failed delete never leaves half a dictionary in use.
        let old = old_dir(root, id);
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&dir, &old).map_err(|e| format!("{}: {e}", dir.display()))?;
        let _ = std::fs::remove_dir_all(&old);
    }
    edit_manifest(root, |m| m.dicts.retain(|d| d.id != id))
}

/// Free bytes on the disk holding `path` (`None` where unknown).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut p = path.to_path_buf();
        while !p.exists() {
            p = p.parent()?.to_path_buf();
        }
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0u64;
        // Safety: a NUL-terminated path and a valid out pointer.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut p = path.to_path_buf();
        while !p.exists() {
            p = p.parent()?.to_path_buf();
        }
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        // Safety: a NUL-terminated path and a zeroed out struct.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
        (ok == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        None
    }
}

/// Room needed to install a dictionary from a zip of `zip_bytes`: the zip,
/// the imported files (measured at about 2.5 times the zip; 3 allowed) and
/// a 1 GB margin.
pub fn space_needed(zip_bytes: u64) -> u64 {
    zip_bytes * 4 + (1 << 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::import::tests::temp_dir;

    fn fake_dict(dir: &Path, title: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let m = Meta {
            version: format::VERSION,
            title: title.into(),
            ..Default::default()
        };
        std::fs::write(dir.join(format::META), serde_json::to_vec(&m).unwrap()).unwrap();
    }

    fn title(dir: &Path) -> String {
        read_meta(dir).unwrap().title
    }

    #[test]
    fn swap_replaces_and_rolls_back() {
        let root = temp_dir("swap");
        fake_dict(&root.join("a"), "old");
        // Each failure leaves the old one in place.
        for fail in [Fail::MoveOld, Fail::MoveNew] {
            fake_dict(&tmp_dir(&root, "a"), "new");
            assert!(swap_with(&root, "a", fail).is_err(), "{fail:?}");
            assert_eq!(title(&root.join("a")), "old", "{fail:?}");
            assert!(!old_dir(&root, "a").exists(), "{fail:?}");
        }
        fake_dict(&tmp_dir(&root, "a"), "new");
        swap(&root, "a").unwrap();
        assert_eq!(title(&root.join("a")), "new");
        assert!(!tmp_dir(&root, "a").exists() && !old_dir(&root, "a").exists());
        // A first install has nothing to move away.
        fake_dict(&tmp_dir(&root, "b"), "first");
        swap(&root, "b").unwrap();
        assert_eq!(title(&root.join("b")), "first");
        // An incomplete import is never swapped in.
        std::fs::create_dir_all(tmp_dir(&root, "a")).unwrap();
        assert!(swap(&root, "a").is_err());
        assert_eq!(title(&root.join("a")), "new");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recovery_after_a_crash() {
        let root = temp_dir("recover");
        // Crashed between moving the old one away and the new one in.
        fake_dict(&old_dir(&root, "a"), "a");
        // Crashed after the new one was in, before the old one was deleted.
        fake_dict(&root.join("b"), "b new");
        fake_dict(&old_dir(&root, "b"), "b old");
        // Unfinished import and download.
        fake_dict(&tmp_dir(&root, "c"), "c");
        std::fs::write(download_path(&root, "d"), b"partial").unwrap();
        recover(&root);
        assert_eq!(title(&root.join("a")), "a");
        assert_eq!(title(&root.join("b")), "b new");
        let left: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|n| !n.starts_with('.')), "{left:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn free_space_is_known_here() {
        if cfg!(any(windows, target_os = "linux")) {
            assert!(free_space(&std::env::temp_dir()).unwrap() > 0);
        }
    }
}
