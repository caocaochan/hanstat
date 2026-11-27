//! The lookup worker: one thread owns every opened dictionary, so lookups
//! never wait on the UI thread nor it on them, and a dictionary can be
//! dropped (its maps closed) before its folder is replaced or deleted.
//!
//! Requests are taken in order, except lookups: only the newest waiting one
//! is done (the pointer has moved on from the others).

use super::lookup::{DictInfo, Engine, Lookup};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

enum Request {
    Lookup { id: u64, text: String },
    Reload { freq_sort: Option<String> },
    Refresh,
    Unload { ids: Vec<String>, done: Sender<()> },
}

#[derive(Clone)]
pub enum Reply {
    Lookup {
        id: u64,
        result: Arc<Lookup>,
        micros: u64,
    },
    /// The dictionaries now loaded (after start, a reload or an unload).
    Loaded {
        dicts: Vec<DictInfo>,
        scan_len: usize,
        errors: Vec<String>,
    },
}

pub struct Service {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    ready: Arc<(Mutex<bool>, Condvar)>,
    next_id: u64,
}

impl Service {
    /// Start the worker on `root`; `wake` is called when a reply is ready
    /// (to repaint the window).
    pub fn start(
        root: PathBuf,
        freq_sort: Option<String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Service {
        let (tx, rx_req) = channel::<Request>();
        let (tx_rep, rx) = channel::<Reply>();
        let ready = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_ready = Arc::clone(&ready);
        std::thread::Builder::new()
            .name("dict-lookup".into())
            .spawn(move || worker(root, freq_sort, worker_ready, rx_req, tx_rep, wake))
            .expect("dictionary thread");
        Service {
            tx,
            rx,
            ready,
            next_id: 0,
        }
    }

    /// Ask for a lookup; its reply carries the returned id.
    pub fn lookup(&mut self, text: String) -> u64 {
        self.next_id += 1;
        let _ = self.tx.send(Request::Lookup {
            id: self.next_id,
            text,
        });
        self.next_id
    }

    /// Open the dictionaries again from the list (after a change to it).
    pub fn reload(&self, freq_sort: Option<String>) {
        let _ = self.tx.send(Request::Reload { freq_sort });
    }

    /// A handle that can unload from another thread (an import or update
    /// putting its files in place).
    pub fn unloader(&self) -> Unloader {
        Unloader {
            tx: self.tx.clone(),
            ready: Arc::clone(&self.ready),
        }
    }

    pub fn try_recv(&self) -> Option<Reply> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<Reply> {
        self.rx.recv_timeout(d).ok()
    }
}

#[derive(Clone)]
pub struct Unloader {
    tx: Sender<Request>,
    ready: Arc<(Mutex<bool>, Condvar)>,
}

impl Unloader {
    /// Wait until startup crash recovery on the lookup worker has finished,
    /// so a new `.tmp-` or `.download-` is never removed by it.
    pub(crate) fn wait_ready(&self) {
        let (lock, cvar) = &*self.ready;
        let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !*done {
            done = cvar.wait(done).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Reopen the dictionaries after a job finishes putting files in place.
    pub fn refresh(&self) {
        let _ = self.tx.send(Request::Refresh);
    }

    pub fn unload(&self, ids: Vec<String>) -> Result<(), String> {
        let (done, wait) = channel();
        self.tx
            .send(Request::Unload { ids, done })
            .map_err(|_| "the dictionary thread stopped".to_string())?;
        wait.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "the dictionary thread didn't answer".to_string())
    }
}

fn worker(
    root: PathBuf,
    freq_sort: Option<String>,
    ready: Arc<(Mutex<bool>, Condvar)>,
    rx: Receiver<Request>,
    tx: Sender<Reply>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    struct MarkReady(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for MarkReady {
        fn drop(&mut self) {
            let (lock, cvar) = &*self.0;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cvar.notify_all();
        }
    }
    // Recovery deletes unfinished imports and downloads before opening the
    // dictionaries (run on this thread so startup never blocks the UI thread).
    {
        let _mark = MarkReady(ready);
        super::manage::recover(&root);
    }
    let mut freq_sort = freq_sort;
    let loaded = |e: &Engine, errors: Vec<String>| Reply::Loaded {
        dicts: e.dicts.clone(),
        scan_len: e.scan_len(),
        errors,
    };
    let (mut engine, errors) = Engine::load(&root, freq_sort.as_deref());
    if tx.send(loaded(&engine, errors)).is_err() {
        return;
    }
    wake();
    while let Ok(first) = rx.recv() {
        // Everything waiting, in order; only the newest lookup is done.
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
        }
        let newest = batch
            .iter()
            .rposition(|r| matches!(r, Request::Lookup { .. }));
        for (i, r) in batch.into_iter().enumerate() {
            let reply = match r {
                Request::Lookup { id, text } => {
                    if Some(i) != newest {
                        continue;
                    }
                    let t = std::time::Instant::now();
                    let result = engine.lookup(&text);
                    Reply::Lookup {
                        id,
                        result,
                        micros: t.elapsed().as_micros() as u64,
                    }
                }
                Request::Reload { .. } | Request::Refresh => {
                    if let Request::Reload { freq_sort: wanted } = r {
                        freq_sort = wanted;
                    }
                    // Drop the old maps before opening the files again.
                    drop(std::mem::replace(&mut engine, Engine::empty()));
                    let (e, errors) = Engine::load(&root, freq_sort.as_deref());
                    engine = e;
                    loaded(&engine, errors)
                }
                Request::Unload { ids, done } => {
                    engine.unload(&ids);
                    let _ = done.send(());
                    loaded(&engine, Vec::new())
                }
            };
            if tx.send(reply).is_err() {
                return;
            }
            wake();
        }
    }
}
