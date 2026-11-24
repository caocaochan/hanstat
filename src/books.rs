//! The Library's own copies of its books, in `books/` next to the history
//! file (`%LOCALAPPDATA%\hanstat\books`). A book opened or added is copied
//! there, and the Library, the reader and everything else read the copy:
//! the original can be moved or deleted.
//!
//! Each book has a folder named by the start of its id (the SHA-256 of the
//! file), holding the file under its original name. So the same book added
//! twice is stored once, two books of the same file name don't collide, and
//! a TXT book keeps the name its title comes from. A copy is never changed,
//! only added and deleted; nothing but the folders is needed to find one.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Hex digits of the id that name a book's folder (64 bits).
const DIR_LEN: usize = 16;

/// Copies being written, under temporary names, before they move into place.
const INCOMING: &str = ".incoming";

/// `books/` next to the history file `history`.
pub fn dir_in(history: &Path) -> PathBuf {
    history.with_file_name("books")
}

/// The default `books/` folder.
pub fn dir() -> PathBuf {
    dir_in(&crate::history::History::path())
}

/// Book `id`'s folder; `None` for what is not an id, so that nothing else
/// is ever taken for one.
fn folder(books: &Path, id: &str) -> Option<PathBuf> {
    let name = id.get(..DIR_LEN)?;
    name.bytes()
        .all(|b| b.is_ascii_hexdigit())
        .then(|| books.join(name))
}

/// The copy of book `id`, if there is one.
pub fn stored(books: &Path, id: &str) -> Option<PathBuf> {
    std::fs::read_dir(folder(books, id)?)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
}

/// Delete the copy of book `id`, if there is one.
pub fn delete(books: &Path, id: &str) {
    if let Some(f) = folder(books, id)
        && f.is_dir()
    {
        let _ = std::fs::remove_dir_all(f);
    }
}

/// Whether `path` is inside `app`, hanstat's folder (a copy here, a
/// DuChinese story): such a file is used where it is. Windows paths compare
/// without regard to case.
pub fn is_own_in(app: &Path, path: &Path) -> bool {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let app = std::path::absolute(app).unwrap_or_else(|_| app.to_owned());
    if !cfg!(windows) {
        return abs.starts_with(&app);
    }
    let mut a = abs.components();
    app.components().all(|c| {
        a.next().is_some_and(|x| {
            x.as_os_str().to_string_lossy().to_lowercase()
                == c.as_os_str().to_string_lossy().to_lowercase()
        })
    })
}

/// [`is_own_in`] hanstat's default folder.
pub fn is_own(path: &Path) -> bool {
    is_own_in(&crate::models::app_dir(), path)
}

/// A book in the Library's folder.
#[derive(Debug)]
pub struct Imported {
    pub path: PathBuf,
    /// SHA-256 of the file, lowercase hex.
    pub id: String,
    /// Copied just now (not there before).
    pub copied: bool,
}

/// Copy the book at `path` into `books`, unless it is there already or is
/// one of hanstat's own files (then used where it is). The file is read
/// once: hashed while it is written under a temporary name, synced, then
/// moved into its folder. With `expect`, a file whose id is not that (it
/// changed since it was analysed) is not kept.
pub fn import(books: &Path, path: &Path, expect: Option<&str>) -> Result<Imported, String> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let app = books.parent().unwrap_or(books);
    if is_own_in(app, &abs) {
        let id = crate::history::file_id(&abs)
            .map_err(|e| format!("Cannot read {}: {e}", abs.display()))?;
        return Ok(Imported {
            path: abs,
            id,
            copied: false,
        });
    }
    let name = abs
        .file_name()
        .ok_or_else(|| format!("{} is not a file", abs.display()))?;
    let incoming = books.join(INCOMING);
    static N: AtomicU64 = AtomicU64::new(0);
    let tmp = incoming.join(format!(
        "{}-{}.part",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let copied = (|| -> std::io::Result<String> {
        std::fs::create_dir_all(&incoming)?;
        let mut src = std::fs::File::open(&abs)?;
        let mut dst = std::fs::File::create(&tmp)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            dst.write_all(&buf[..n])?;
        }
        dst.sync_all()?;
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })();
    let fail = |e: String| {
        let _ = std::fs::remove_file(&tmp);
        Err(e)
    };
    let id = match copied {
        Ok(id) => id,
        Err(e) => {
            return fail(format!(
                "Cannot copy {} into the Library: {e}",
                abs.display()
            ));
        }
    };
    if expect.is_some_and(|x| x != id) {
        return fail(format!("{} changed since it was analysed", abs.display()));
    }
    if let Some(p) = stored(books, &id) {
        let _ = std::fs::remove_file(&tmp);
        return Ok(Imported {
            path: p,
            id,
            copied: false,
        });
    }
    let dest = folder(books, &id).unwrap().join(name);
    let moved =
        std::fs::create_dir_all(dest.parent().unwrap()).and_then(|_| std::fs::rename(&tmp, &dest));
    match moved {
        Ok(()) => Ok(Imported {
            path: dest,
            id,
            copied: true,
        }),
        // Put there meanwhile (the same book added twice at once)?
        Err(e) => match stored(books, &id) {
            Some(p) => {
                let _ = std::fs::remove_file(&tmp);
                Ok(Imported {
                    path: p,
                    id,
                    copied: false,
                })
            }
            None => fail(format!(
                "Cannot copy {} into the Library: {e}",
                abs.display()
            )),
        },
    }
}

/// Delete copies left half-written (hanstat was closed while copying) more
/// than an hour ago.
pub fn clean_incoming(books: &Path) {
    let Ok(dir) = std::fs::read_dir(books.join(INCOMING)) else {
        return;
    };
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for e in dir.flatten() {
        if e.metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < hour_ago)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hanstat-books-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn copies_once_and_keeps_the_name_and_id() {
        let d = scratch("once");
        let app = d.join("hanstat");
        let books = app.join("books");
        let a = d.join("小说.txt");
        std::fs::write(&a, "一二三").unwrap();
        let first = import(&books, &a, None).unwrap();
        assert!(first.copied);
        assert_eq!(first.id, crate::history::file_id(&a).unwrap());
        assert_eq!(first.path.file_name().unwrap(), "小说.txt");
        assert!(first.path.starts_with(&books));
        // The same contents under another name: the copy there already.
        let b = d.join("other.txt");
        std::fs::copy(&a, &b).unwrap();
        let again = import(&books, &b, None).unwrap();
        assert!(!again.copied);
        assert_eq!(again.path, first.path);
        assert_eq!(stored(&books, &first.id), Some(first.path.clone()));
        // The original going does not matter.
        std::fs::remove_file(&a).unwrap();
        assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "一二三");
        // A copy given again is used where it is.
        let own = import(&books, &first.path, None).unwrap();
        assert!(!own.copied);
        assert_eq!(own.path, first.path);
        assert!(
            std::fs::read_dir(books.join(INCOMING))
                .unwrap()
                .next()
                .is_none()
        );
        delete(&books, &first.id);
        assert_eq!(stored(&books, &first.id), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_file_is_not_kept() {
        let d = scratch("changed");
        let books = d.join("hanstat").join("books");
        let a = d.join("a.txt");
        std::fs::write(&a, "new").unwrap();
        let wrong = "0".repeat(64);
        assert!(import(&books, &a, Some(&wrong)).is_err());
        assert!(
            std::fs::read_dir(&books).unwrap().all(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.'))
        );
        assert!(
            std::fs::read_dir(books.join(INCOMING))
                .unwrap()
                .next()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_ids_name_folders() {
        let books = Path::new("books");
        assert_eq!(folder(books, ""), None);
        assert_eq!(folder(books, "../../../../etc"), None);
        assert_eq!(
            folder(books, "0123456789abcdef"),
            Some(books.join("0123456789abcdef"))
        );
    }

    #[test]
    fn own_files_compare_without_case_on_windows() {
        let app = Path::new(r"C:\Users\x\AppData\Local\hanstat");
        if cfg!(windows) {
            assert!(is_own_in(
                app,
                Path::new(r"c:\users\X\appdata\local\HANSTAT\books\a.epub")
            ));
            assert!(!is_own_in(
                app,
                Path::new(r"C:\Users\x\AppData\Local\hanstat2\a.epub")
            ));
            assert!(!is_own_in(app, Path::new(r"D:\Books\a.epub")));
        }
    }
}
