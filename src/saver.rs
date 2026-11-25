//! Files written off the UI thread. The window calls [`defer`] at startup:
//! from then on [`write`] only queues the bytes, and a writer thread writes
//! them a moment later, keeping only the newest bytes of a file written
//! several times meanwhile (marking word after word known rewrites the
//! History once, not once per word). [`read`] returns queued bytes not yet
//! on disk, so a file always reads as last written. Without [`defer`] (the
//! command line, tests) [`write`] writes at once.
//!
//! Every file is written to a temporary name, flushed to the disk and
//! renamed, so neither a crash nor a power cut leaves half a file. [`flush`]
//! waits for the queue to empty: call it before exiting, or before another
//! process reads the files.
//!
//! [`load_json`] reads a file written here. One that can't be read is
//! renamed aside (never overwritten by the next save) and reported through
//! [`take_notices`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

/// How long queued bytes wait for newer ones before they are written.
const QUIET: Duration = Duration::from_millis(400);

static DEFERRED: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();

#[derive(Default)]
struct State {
    pending: HashMap<PathBuf, Arc<Vec<u8>>>,
    /// [`flush`] is waiting: write without the pause.
    hurry: bool,
    error: Option<String>,
}

struct Saver {
    state: Mutex<State>,
    cv: Condvar,
}

static SAVER: LazyLock<Saver> = LazyLock::new(|| Saver {
    state: Mutex::new(State::default()),
    cv: Condvar::new(),
});

fn lock() -> MutexGuard<'static, State> {
    SAVER.state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Files set aside by [`load_json`], for the window to show.
static NOTICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Read JSON file `path`. `None` if it is missing, or can't be read: then
/// it is renamed to `<name>.unreadable-<unix time>`, so that saving the
/// defaults in its place destroys nothing, and a notice says so.
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = match read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            notice(format!(
                "Cannot read {}: {e}. Defaults are used for now.",
                path.display()
            ));
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            let name = |p: &Path| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            };
            let msg = match set_aside(path) {
                Ok(kept) => format!(
                    "{} couldn't be read ({e}): kept as {} in the same folder; defaults are used.",
                    name(path),
                    name(&kept)
                ),
                Err(e2) => format!(
                    "{} couldn't be read ({e}) or kept aside ({e2}); defaults are used.",
                    path.display()
                ),
            };
            notice(msg);
            None
        }
    }
}

fn notice(msg: String) {
    NOTICES.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
}

/// What [`load_json`] had to set aside since the last call.
pub fn take_notices() -> Vec<String> {
    std::mem::take(&mut *NOTICES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Rename `path` to a free `<name>.unreadable-<unix time>[-n]` beside it.
fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    // A queued write would bring the file back over the one kept.
    lock().pending.remove(path);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    for n in 0.. {
        let kept = path.with_file_name(if n == 0 {
            format!("{name}.unreadable-{secs}")
        } else {
            format!("{name}.unreadable-{secs}-{n}")
        });
        if !kept.exists() {
            std::fs::rename(path, &kept)?;
            return Ok(kept);
        }
    }
    unreachable!()
}

/// From now on, [`write`] queues instead of writing.
pub fn defer() {
    DEFERRED.store(true, Ordering::Relaxed);
}

/// Write `bytes` to `path` (atomically), now or, after [`defer`], soon. A
/// queued write that fails is reported by [`take_error`].
pub fn write(path: &Path, bytes: Vec<u8>) -> Result<(), String> {
    if !DEFERRED.load(Ordering::Relaxed) {
        return write_now(path, &bytes);
    }
    START.call_once(|| {
        std::thread::Builder::new()
            .name("hanstat-saver".into())
            .spawn(run)
            .expect("cannot start the file writer thread");
    });
    lock().pending.insert(path.to_owned(), Arc::new(bytes));
    SAVER.cv.notify_all();
    Ok(())
}

/// The file's contents: bytes queued for it, or else what is on disk.
pub fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    if let Some(b) = lock().pending.get(path) {
        return Ok(b.to_vec());
    }
    std::fs::read(path)
}

pub fn is_pending(path: &Path) -> bool {
    lock().pending.contains_key(path)
}

/// Wait until every queued file is written.
pub fn flush() {
    let mut st = lock();
    if st.pending.is_empty() {
        return;
    }
    st.hurry = true;
    SAVER.cv.notify_all();
    while !st.pending.is_empty() {
        st = SAVER.cv.wait(st).unwrap_or_else(|e| e.into_inner());
    }
}

/// The last queued write that failed, once.
pub fn take_error() -> Option<String> {
    lock().error.take()
}

fn run() {
    loop {
        let mut st = lock();
        while st.pending.is_empty() {
            st = SAVER.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        let until = Instant::now() + QUIET;
        while !st.hurry {
            let now = Instant::now();
            if now >= until {
                break;
            }
            st = SAVER
                .cv
                .wait_timeout(st, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let batch: Vec<(PathBuf, Arc<Vec<u8>>)> = st
            .pending
            .iter()
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        drop(st);
        let mut failed = None;
        for (p, b) in &batch {
            if let Err(e) = write_now(p, b) {
                failed = Some(e);
            }
        }
        let mut st = lock();
        for (p, b) in batch {
            // Unless newer bytes came meanwhile.
            if st.pending.get(&p).is_some_and(|x| Arc::ptr_eq(x, &b)) {
                st.pending.remove(&p);
            }
        }
        if failed.is_some() {
            st.error = failed;
        }
        if st.pending.is_empty() {
            st.hurry = false;
        }
        SAVER.cv.notify_all();
    }
}

/// Write under a temporary name, then rename.
fn write_now(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    // On the disk before the rename: a power cut then leaves the old file
    // or the new one, never an empty or half one.
    let synced = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    synced.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Cannot write {}: {e}", tmp.display())
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Cannot write {}: {e}", path.display())
    })?;
    // The rename itself, on systems where a directory is synced apart.
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_files_are_set_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("hanstat-saver-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.json");
        assert_eq!(load_json::<Vec<u32>>(&p), None);
        write(&p, b"[1,2]".to_vec()).unwrap();
        assert_eq!(load_json::<Vec<u32>>(&p), Some(vec![1, 2]));
        std::fs::write(&p, b"{broken").unwrap();
        let _ = take_notices();
        assert_eq!(load_json::<Vec<u32>>(&p), None);
        assert!(!p.exists(), "set aside");
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("s.json.unreadable-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"{broken");
        let notes = take_notices();
        assert!(
            notes.iter().any(|n| n.contains("couldn't be read")),
            "{notes:?}"
        );
        // The defaults saved now don't touch the copy kept.
        write(&p, b"[]".to_vec()).unwrap();
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"{broken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_now_unless_deferred() {
        let dir = std::env::temp_dir().join(format!("hanstat-saver-{}", std::process::id()));
        let p = dir.join("a.json");
        write(&p, b"one".to_vec()).unwrap();
        assert_eq!(read(&p).unwrap(), b"one");
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
