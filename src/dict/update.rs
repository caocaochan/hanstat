//! Dictionary updates over the network: reading a dictionary's announced
//! revision from its `indexUrl`, and downloading the new zip to a file (never
//! into memory: Wikipedia's is 295 MB).
//!
//! Requests go only to the URLs the dictionaries carry, with the app's
//! user agent and nothing about the user. https only, except in tests.

use super::import::valid_url;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const USER_AGENT: &str = concat!("hanstat/", env!("CARGO_PKG_VERSION"));
/// Index files are a few hundred bytes; anything this big is not one.
const MAX_INDEX: u64 = 1 << 20;
/// No dictionary zip is this large (the largest known is 295 MB).
const MAX_ZIP: u64 = 8 << 30;
/// Checks at once.
const PARALLEL: usize = 8;

/// What a dictionary's `indexUrl` announces.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub revision: String,
    /// The zip to download; the dictionary's own `downloadUrl` if absent.
    pub download_url: Option<String>,
}

pub fn agent(https_only: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(https_only)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(10)))
        .max_redirects(8)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Read a remote index: it must be JSON with a string `revision`; a
/// `downloadUrl`, if any, must be an http(s) URL (https with `https_only`).
pub fn parse_remote(text: &str, https_only: bool) -> Result<Remote, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("not a dictionary index ({e})"))?;
    let revision = match v.get("revision") {
        Some(serde_json::Value::String(r)) if !r.trim().is_empty() => r.trim().to_owned(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return Err("the index has no revision".into()),
    };
    let download_url = match v.get("downloadUrl") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(u))
            if valid_url(u) && (!https_only || u.starts_with("https://")) =>
        {
            Some(u.clone())
        }
        Some(_) => return Err("the index's downloadUrl is not a web address".into()),
    };
    Ok(Remote {
        revision,
        download_url,
    })
}

pub fn check(agent: &ureq::Agent, index_url: &str, https_only: bool) -> Result<Remote, String> {
    let mut r = agent.get(index_url).call().map_err(|e| e.to_string())?;
    let text = r
        .body_mut()
        .with_config()
        .limit(MAX_INDEX)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_remote(&text, https_only)
}

/// Check several dictionaries at once (up to 8 requests in parallel); one
/// failing never holds up the others. Results come back in the given order.
pub fn check_all(
    urls: Vec<(String, String)>,
    https_only: bool,
) -> Vec<(String, Result<Remote, String>)> {
    let n = urls.len();
    let queue = Arc::new(Mutex::new(urls.into_iter().enumerate().collect::<Vec<_>>()));
    let results = Arc::new(Mutex::new(vec![None; n]));
    let agent = agent(https_only);
    std::thread::scope(|s| {
        for _ in 0..PARALLEL.min(n) {
            let (queue, results, agent) = (queue.clone(), results.clone(), agent.clone());
            s.spawn(move || {
                loop {
                    let next = queue.lock().unwrap().pop();
                    let Some((i, (id, url))) = next else { break };
                    let r = check(&agent, &url, https_only);
                    results.lock().unwrap()[i] = Some((id, r));
                }
            });
        }
    });
    Arc::try_unwrap(results)
        .ok()
        .unwrap()
        .into_inner()
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}

/// Download `url` to `dest`, 64 KB at a time. `room(len)` is asked once the
/// size is known (it may refuse, e.g. for lack of disk space); `progress`
/// gets (bytes so far, total if known). On error or `cancel` the partial
/// file is deleted.
pub fn download(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    room: &dyn Fn(Option<u64>) -> Result<(), String>,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let r = (|| {
        let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
        let total = resp.body().content_length();
        if total.is_some_and(|t| t > MAX_ZIP) {
            return Err(format!(
                "{} MB: too large for a dictionary",
                total.unwrap() >> 20
            ));
        }
        room(total)?;
        let mut f = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut reader = resp.body_mut().with_config().limit(MAX_ZIP).reader();
        let mut buf = vec![0u8; 64 << 10];
        let mut done = 0u64;
        loop {
            if cancel.load(Relaxed) {
                return Err("cancelled".into());
            }
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n])
                .map_err(|e| format!("{}: {e}", dest.display()))?;
            done += n as u64;
            progress(done, total);
        }
        f.flush().map_err(|e| e.to_string())?;
        if total.is_some_and(|t| t != done) {
            return Err(format!(
                "the download stopped at {done} of {} bytes",
                total.unwrap()
            ));
        }
        Ok(done)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn remote_index_validation() {
        let r = parse_remote(r#"{"title":"CC-CEDICT [2026-11-17]","revision":"2026-11-17","downloadUrl":"https://x.org/d.zip"}"#, true).unwrap();
        assert_eq!(r.revision, "2026-11-17");
        assert_eq!(r.download_url.as_deref(), Some("https://x.org/d.zip"));
        // No downloadUrl: the stored one is used.
        assert_eq!(
            parse_remote(r#"{"revision":"2"}"#, true)
                .unwrap()
                .download_url,
            None
        );
        assert!(
            parse_remote(
                r#"{"revision":"2","downloadUrl":"http://x.org/d.zip"}"#,
                true
            )
            .is_err(),
            "https only"
        );
        assert!(
            parse_remote(
                r#"{"revision":"2","downloadUrl":"http://x.org/d.zip"}"#,
                false
            )
            .is_ok()
        );
        assert!(parse_remote(r#"{"revision":"2","downloadUrl":"file:///c:/x"}"#, false).is_err());
        assert!(parse_remote(r#"{"title":"no revision"}"#, true).is_err());
        assert!(parse_remote("<html>404</html>", true).is_err());
    }

    /// A one-file-per-path HTTP server on localhost for tests: serves
    /// `files` (path → bytes) until dropped; 404 for anything else.
    pub(crate) struct TestServer {
        pub base: String,
        stop: Arc<AtomicBool>,
    }

    impl TestServer {
        pub(crate) fn start(files: Vec<(&'static str, Vec<u8>)>) -> TestServer {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.set_nonblocking(true).unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let s2 = stop.clone();
            std::thread::spawn(move || {
                while !s2.load(Relaxed) {
                    let Ok((mut c, _)) = l.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    c.set_nonblocking(false).unwrap();
                    let mut req = Vec::new();
                    let mut b = [0u8; 1024];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = c.read(&mut b).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        req.extend_from_slice(&b[..n]);
                    }
                    let line = String::from_utf8_lossy(&req);
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    match files.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let _ = write!(
                                c,
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = c.write_all(body);
                        }
                        None => {
                            let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                }
            });
            TestServer { base, stop }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Relaxed);
        }
    }

    #[test]
    fn checks_and_downloads_over_http() {
        let zip = vec![7u8; 300_000];
        let srv = TestServer::start(vec![
            (
                "/index.json",
                br#"{"revision":"2026.11.17","downloadUrl":"http://unused/d.zip"}"#.to_vec(),
            ),
            ("/bad.json", b"<html>".to_vec()),
            ("/d.zip", zip.clone()),
        ]);
        let urls = vec![
            ("a".to_string(), format!("{}/index.json", srv.base)),
            ("b".to_string(), format!("{}/bad.json", srv.base)),
            ("c".to_string(), format!("{}/missing.json", srv.base)),
        ];
        let r = check_all(urls, false);
        assert_eq!(r[0].0, "a");
        assert_eq!(r[0].1.as_ref().unwrap().revision, "2026.11.17");
        assert!(r[1].1.is_err() && r[2].1.is_err());
        // https only refuses plain http.
        assert!(check(&agent(true), &format!("{}/index.json", srv.base), true).is_err());

        let dir = crate::dict::import::tests::temp_dir("download");
        let dest = dir.join("d.zip");
        let no = AtomicBool::new(false);
        let seen = Mutex::new(0u64);
        let n = download(
            &agent(false),
            &format!("{}/d.zip", srv.base),
            &dest,
            &|_| Ok(()),
            &|d, _| *seen.lock().unwrap() = d,
            &no,
        )
        .unwrap();
        assert_eq!(n, zip.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), zip);
        assert_eq!(*seen.lock().unwrap(), n);
        // Refused for room: no file left.
        std::fs::remove_file(&dest).unwrap();
        let e = download(
            &agent(false),
            &format!("{}/d.zip", srv.base),
            &dest,
            &|len| Err(format!("no room for {}", len.unwrap())),
            &|_, _| {},
            &no,
        )
        .unwrap_err();
        assert_eq!(e, "no room for 300000");
        assert!(!dest.exists());
        // Cancelled: no file left.
        let yes = AtomicBool::new(true);
        assert!(
            download(
                &agent(false),
                &format!("{}/d.zip", srv.base),
                &dest,
                &|_| Ok(()),
                &|_, _| {},
                &yes
            )
            .is_err()
        );
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
