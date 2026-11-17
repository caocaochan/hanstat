//! Files written off the UI thread. The window calls [`defer`] at startup:
//! from then on [`write`] only queues the bytes, and a writer thread writes
//! them a moment later, keeping only the newest bytes of a file written
//! several times meanwhile (marking word after word known rewrites the
//! History once, not once per word). [`read`] returns queued bytes not yet
//! on disk, so a file always reads as last written. Without [`defer`] (the
//! command line, tests) [`write`] writes at once.
//!
//! Every file is written to a temporary name and renamed, so a crash never
//! leaves half a file. [`flush`] waits for the queue to empty: call it
//! before exiting, or before another process reads the files.

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
    std::fs::write(&tmp, bytes).map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Cannot write {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
