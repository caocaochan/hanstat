//! Imports and updates, run one at a time on a thread of their own (an
//! import already uses every core but one), with progress and cancelling.
//! The dictionary in use keeps answering lookups until its replacement is
//! complete; then the lookup thread drops it and the folders are swapped
//! ([`super::manage::swap`]).

use super::format::{Check, CheckResult, Meta, now};
use super::import::{self, Progress};
use super::service::Unloader;
use super::{manage, revision, update};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// The dictionary installed when there is none: CC-CEDICT (MarvNC's Yomitan
/// edition, CC BY-SA 4.0), from its releases.
pub const DEFAULT: (&str, &str) = (
    "CC-CEDICT",
    "https://github.com/MarvNC/cc-cedict-yomitan/releases/latest/download/CC-CEDICT.zip",
);

pub enum Task {
    /// Download a dictionary from `url` and install it (as a new one, or
    /// replacing the same one if installed).
    Install { url: String },
    /// Import a zip; replace `replace` (or the same dictionary, if installed).
    Import {
        zip: PathBuf,
        replace: Option<String>,
    },
    /// Download dictionary `id`'s new revision from `url` and put it in place.
    Update {
        id: String,
        url: String,
        revision: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting,
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Importing,
    /// The message to show, or the error.
    Done(Result<String, String>),
}

pub struct Job {
    pub label: String,
    pub state: Mutex<State>,
    pub progress: Progress,
    pub cancel: AtomicBool,
}

impl Job {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    fn set(&self, s: State) {
        *self.state.lock().unwrap() = s;
    }
}

pub struct Jobs {
    tx: Sender<(Arc<Job>, Task)>,
    pub list: Vec<Arc<Job>>,
}

impl Jobs {
    /// `wake` is called when a job moves on (to repaint).
    pub fn start(
        root: PathBuf,
        unloader: Unloader,
        https_only: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Jobs {
        let (tx, rx) = channel::<(Arc<Job>, Task)>();
        std::thread::Builder::new()
            .name("dict-jobs".into())
            .spawn(move || {
                for (job, task) in rx {
                    let r = if job.cancel.load(Relaxed) {
                        Err("cancelled".into())
                    } else {
                        match task {
                            Task::Import { zip, replace } => {
                                job.set(State::Importing);
                                wake();
                                run_import(&root, &zip, replace, &job, &unloader)
                            }
                            Task::Install { url } => {
                                run_install(&root, &url, &job, &unloader, https_only, &*wake)
                            }
                            Task::Update { id, url, revision } => run_update(
                                &root, &id, &url, &revision, &job, &unloader, https_only, &*wake,
                            ),
                        }
                    };
                    job.set(State::Done(r));
                    wake();
                }
            })
            .expect("dictionary jobs thread");
        Jobs {
            tx,
            list: Vec::new(),
        }
    }

    pub fn add(&mut self, label: String, task: Task) -> Arc<Job> {
        let job = Arc::new(Job {
            label,
            state: Mutex::new(State::Waiting),
            progress: Progress::default(),
            cancel: AtomicBool::new(false),
        });
        let _ = self.tx.send((job.clone(), task));
        self.list.push(job.clone());
        job
    }

    /// Jobs running, waiting, or finished and not yet taken (by
    /// [`Jobs::take_finished`]): their result is not in the list yet.
    pub fn busy(&self) -> bool {
        !self.list.is_empty()
    }

    /// Finished jobs, taken off the list.
    pub fn take_finished(&mut self) -> Vec<(String, Result<String, String>)> {
        let mut out = Vec::new();
        self.list.retain(|j| match j.state() {
            State::Done(r) => {
                out.push((j.label.clone(), r));
                false
            }
            _ => true,
        });
        out
    }
}

/// Import `zip` and put it in place (see [`Task::Import`]).
pub fn run_import(
    root: &Path,
    zip: &Path,
    replace: Option<String>,
    job: &Job,
    unloader: &Unloader,
) -> Result<String, String> {
    // (No crash recovery here: it would delete an update's download. The
    // lookup thread recovers when it starts.)
    let index = import::read_index(zip)?;
    let id = replace
        .or_else(|| manage::existing(root, &index))
        .unwrap_or_else(super::format::new_id);
    let tmp = manage::tmp_dir(root, &id);
    let meta = match import::import(zip, &tmp, &job.progress, &job.cancel) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    // From here until the reload, lookups don't see this dictionary.
    let t = std::time::Instant::now();
    let placed = unloader
        .unload(vec![id.clone()])
        .and_then(|_| manage::swap(root, &id))
        .and_then(|_| manage::add_to_manifest(root, &id));
    if std::env::var_os("HANSTAT_DICT_TIMING").is_some() {
        eprintln!("{}: unloaded and swapped in {:?}", meta.title, t.elapsed());
    }
    if let Err(e) = placed {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    Ok(summary(&meta))
}

fn summary(m: &Meta) -> String {
    let what = match (m.terms, m.freqs) {
        (t, 0) => format!("{t} entries"),
        (0, f) => format!("{f} frequencies"),
        (t, f) => format!("{t} entries, {f} frequencies"),
    };
    format!("{} (revision {}): {what}", m.title, m.revision)
}

/// Download and install a dictionary's new revision (see [`Task::Update`]).
#[allow(clippy::too_many_arguments)]
pub fn run_update(
    root: &Path,
    id: &str,
    url: &str,
    revision: &str,
    job: &Job,
    unloader: &Unloader,
    https_only: bool,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<String, String> {
    let known_size = manage::read_meta(&root.join(id)).and_then(|m| m.source.map(|s| s.size));
    let msg = download_import(
        root,
        id,
        Some(id.to_owned()),
        url,
        known_size,
        job,
        unloader,
        https_only,
        wake,
    )?;
    let got = manage::read_meta(&root.join(id))
        .map(|m| m.revision)
        .unwrap_or_default();
    Ok(if got != revision && !revision.is_empty() {
        format!("{msg}; the index announced revision {revision}")
    } else {
        msg
    })
}

/// Download a dictionary not installed yet and install it.
pub fn run_install(
    root: &Path,
    url: &str,
    job: &Job,
    unloader: &Unloader,
    https_only: bool,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<String, String> {
    let tag = super::format::new_id();
    download_import(root, &tag, None, url, None, job, unloader, https_only, wake)
}

/// Download `url` to `.download-<tag>.zip` (after checking for room) and
/// import it, replacing `replace` if given.
#[allow(clippy::too_many_arguments)]
fn download_import(
    root: &Path,
    tag: &str,
    replace: Option<String>,
    url: &str,
    known_size: Option<u64>,
    job: &Job,
    unloader: &Unloader,
    https_only: bool,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<String, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let zip = manage::download_path(root, tag);
    let room = |len: Option<u64>| -> Result<(), String> {
        let need = manage::space_needed(len.or(known_size).unwrap_or(300 << 20));
        match manage::free_space(root) {
            Some(free) if free < need => Err(format!(
                "not enough disk space: {} MB needed, {} MB free",
                need >> 20,
                free >> 20
            )),
            _ => Ok(()),
        }
    };
    job.set(State::Downloading {
        done: 0,
        total: None,
    });
    wake();
    let last = Mutex::new(std::time::Instant::now());
    let progress = |done: u64, total: Option<u64>| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_millis() > 100 {
            *l = std::time::Instant::now();
            job.set(State::Downloading { done, total });
            wake();
        }
    };
    let agent = update::agent(https_only);
    update::download(&agent, url, &zip, &room, &progress, &job.cancel)?;
    job.set(State::Importing);
    wake();
    let r = run_import(root, &zip, replace, job, unloader);
    let _ = std::fs::remove_file(&zip);
    r
}

/// Check every updatable dictionary's announced revision and record the
/// results in the list. Returns how many have an update.
pub fn check_updates(root: &Path, https_only: bool) -> usize {
    let list = manage::list(root);
    let urls: Vec<(String, String)> = list
        .iter()
        .filter_map(|(e, m)| Some((e.id.clone(), m.as_ref()?.update.as_ref()?.index_url.clone())))
        .collect();
    let results = update::check_all(urls, https_only);
    let at = now();
    let mut available = 0;
    let checks: Vec<(String, CheckResult)> = results
        .into_iter()
        .map(|(id, r)| {
            let meta = list
                .iter()
                .find(|(e, _)| e.id == id)
                .and_then(|(_, m)| m.clone());
            let result = match (r, meta) {
                (Err(e), _) => CheckResult::Failed(e),
                (Ok(remote), Some(m)) => {
                    if revision::is_newer(&m.revision, &remote.revision) {
                        available += 1;
                        CheckResult::Available {
                            revision: remote.revision,
                            download_url: remote
                                .download_url
                                .or(m.update.map(|u| u.download_url))
                                .unwrap_or_default(),
                        }
                    } else {
                        CheckResult::UpToDate
                    }
                }
                (Ok(_), None) => CheckResult::Failed("unreadable dictionary".into()),
            };
            (id, result)
        })
        .collect();
    let _ = manage::edit_manifest(root, |m| {
        for (id, result) in checks {
            if let Some(e) = m.dicts.iter_mut().find(|e| e.id == id) {
                e.check = Some(Check { at, result });
            }
        }
        m.last_auto_check = at;
    });
    available
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::format::Manifest;
    use crate::dict::import::tests::{make_zip, temp_dir};
    use crate::dict::service::Service;
    use crate::dict::update::tests::TestServer;

    fn zip_bytes(dir: &Path, revision: &str, gloss: &str, base: &str) -> Vec<u8> {
        let p = dir.join(format!("src-{revision}.zip"));
        let index = format!(
            r#"{{"title":"Tiny [{revision}]","revision":"{revision}","format":3,"isUpdatable":true,
                "indexUrl":"{base}/index.json","downloadUrl":"{base}/tiny.zip"}}"#
        );
        let bank = format!(r#"[["说话","shuō huà","","",0,["{gloss}"],1,""]]"#);
        make_zip(
            &p,
            &[
                ("index.json", index.as_bytes()),
                ("term_bank_1.json", bank.as_bytes()),
            ],
        );
        std::fs::read(&p).unwrap()
    }

    /// Over the network: CC-CEDICT from `HANSTAT_DICTS` (an older revision)
    /// checked against its site and updated by downloading the new zip,
    /// into `HANSTAT_DICT_OUT` (deleted after).
    #[test]
    #[ignore]
    fn real_update() {
        let (Some(src), Some(out)) = (
            std::env::var_os("HANSTAT_DICTS"),
            std::env::var_os("HANSTAT_DICT_OUT"),
        ) else {
            panic!("set HANSTAT_DICTS and HANSTAT_DICT_OUT");
        };
        let root = PathBuf::from(out).join("real-update");
        let _ = std::fs::remove_dir_all(&root);
        let svc = Service::start(root.clone(), None, Arc::new(|| {}));
        let job = Job {
            label: "CC-CEDICT".into(),
            state: Mutex::new(State::Waiting),
            progress: Progress::default(),
            cancel: AtomicBool::new(false),
        };
        run_import(
            &root,
            &PathBuf::from(src).join("CC-CEDICT.zip"),
            None,
            &job,
            &svc.unloader(),
        )
        .unwrap();
        let id = Manifest::load(&root).dicts[0].id.clone();
        let before = manage::read_meta(&root.join(&id)).unwrap();
        let t = std::time::Instant::now();
        let n = check_updates(&root, true);
        eprintln!("checked in {:?}: {n} update", t.elapsed());
        match Manifest::load(&root).dicts[0].check.clone().unwrap().result {
            CheckResult::Available {
                revision,
                download_url,
            } => {
                let t = std::time::Instant::now();
                let msg = run_update(
                    &root,
                    &id,
                    &download_url,
                    &revision,
                    &job,
                    &svc.unloader(),
                    true,
                    &|| {},
                )
                .unwrap();
                eprintln!("updated in {:?}: {msg}", t.elapsed());
                let after = manage::read_meta(&root.join(&id)).unwrap();
                assert!(crate::dict::revision::is_newer(
                    &before.revision,
                    &after.revision
                ));
                assert_ne!(
                    before.title, after.title,
                    "CC-CEDICT's title carries its date"
                );
                assert_eq!(Manifest::load(&root).dicts.len(), 1);
            }
            other => eprintln!("no update to test: {other:?}"),
        }
        drop(svc);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Install revision 1, find revision 2 announced, update to it over HTTP
    /// while the old one is loaded, and look the new glossary up.
    #[test]
    fn update_end_to_end() {
        let root = temp_dir("update-e2e");
        // The zips' own URLs are never used: the installed dictionary is
        // pointed at the test server below.
        let base = "https://example.org";
        let v1 = zip_bytes(&root, "2026.1.1", "to speak (old)", base);
        let v2 = zip_bytes(&root, "2026.1.10", "to speak (new)", base);
        // Revision 1, installed from a file.
        std::fs::write(root.join("v1.zip"), &v1).unwrap();
        let svc = Service::start(root.clone(), None, Arc::new(|| {}));
        let job = Job {
            label: "v1".into(),
            state: Mutex::new(State::Waiting),
            progress: Progress::default(),
            cancel: AtomicBool::new(false),
        };
        let msg = run_import(&root, &root.join("v1.zip"), None, &job, &svc.unloader()).unwrap();
        assert!(msg.contains("2026.1.1"), "{msg}");
        let id = Manifest::load(&root).dicts[0].id.clone();
        // Revision 2 announced by a server.
        let srv = TestServer::start(vec![
            ("/index.json", br#"{"revision":"2026.1.10"}"#.to_vec()),
            ("/tiny.zip", v2.clone()),
        ]);
        // Point the installed dictionary at this server.
        let dir = root.join(&id);
        let mut m = manage::read_meta(&dir).unwrap();
        m.update = Some(crate::dict::format::UpdateUrls {
            index_url: format!("{}/index.json", srv.base),
            download_url: format!("{}/tiny.zip", srv.base),
        });
        std::fs::write(
            dir.join(crate::dict::format::META),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        assert_eq!(check_updates(&root, false), 1);
        let Some(Check {
            result:
                CheckResult::Available {
                    revision,
                    download_url,
                },
            ..
        }) = Manifest::load(&root).dicts[0].check.clone()
        else {
            panic!("an update is available");
        };
        assert_eq!(revision, "2026.1.10", "numbers, not strings");
        // The old one is loaded (mapped) while the update runs.
        svc.reload(None);
        let msg = run_update(
            &root,
            &id,
            &download_url,
            &revision,
            &job,
            &svc.unloader(),
            false,
            &|| {},
        )
        .unwrap();
        assert!(msg.contains("2026.1.10"), "{msg}");
        assert_eq!(
            manage::read_meta(&root.join(&id)).unwrap().revision,
            "2026.1.10"
        );
        assert_eq!(
            Manifest::load(&root).dicts.len(),
            1,
            "the same dictionary, replaced"
        );
        assert_eq!(Manifest::load(&root).dicts[0].id, id, "keeping its id");
        let (mut e, _) = crate::dict::lookup::Engine::load(&root, None);
        let r = e.lookup("说话");
        assert!(
            r.groups[0].parts[0].glossaries[0]
                .json
                .as_ref()
                .unwrap()
                .contains("new")
        );
        drop(e);
        // Nothing left over.
        let left: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|x| x.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // Installing a new dictionary from a URL.
        let other = {
            // A different dictionary: another title, no update URLs.
            let p = root.join("other.zip");
            let index = r#"{"title":"Other","revision":"7","format":3}"#;
            let bank = r#"[["长","cháng","","",0,["long"],1,""]]"#;
            make_zip(
                &p,
                &[
                    ("index.json", index.as_bytes()),
                    ("term_bank_1.json", bank.as_bytes()),
                ],
            );
            std::fs::read(&p).unwrap()
        };
        let srv2 = TestServer::start(vec![("/other.zip", other)]);
        let msg = run_install(
            &root,
            &format!("{}/other.zip", srv2.base),
            &job,
            &svc.unloader(),
            false,
            &|| {},
        )
        .unwrap();
        assert!(msg.starts_with("Other"), "{msg}");
        assert_eq!(Manifest::load(&root).dicts.len(), 2);
        // A failed download keeps the installed revision.
        let bad = format!("{}/missing.zip", srv.base);
        assert!(
            run_update(
                &root,
                &id,
                &bad,
                "2026.2.1",
                &job,
                &svc.unloader(),
                false,
                &|| {}
            )
            .is_err()
        );
        assert_eq!(
            manage::read_meta(&root.join(&id)).unwrap().revision,
            "2026.1.10"
        );
        drop(svc);
        std::fs::remove_dir_all(&root).ok();
    }
}
