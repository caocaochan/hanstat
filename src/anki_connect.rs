//! Adding notes to Anki through AnkiConnect, the add-on Yomitan uses: a
//! small HTTP server inside the running Anki (`http://127.0.0.1:8765` by
//! default). Requests use its version-6 API at `/`, which the original
//! add-on (2055492159), its forks (AnkiConnect Extended) and replacements
//! (Tsunagi) all answer. The collection itself is never touched directly:
//! Anki holds it locked while it runs, and adding a note right means
//! generating its cards from the note type's templates, which only Anki does.
//!
//! Calls block for as long as Anki takes (it answers on its main thread, so a
//! modal dialog there holds them up): make them off the UI thread.

use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8765";

/// AnkiConnect's answer when a note's first field matches an existing note.
const DUPLICATE: &str = "cannot create note because it is a duplicate";

pub struct Client {
    url: String,
    key: Option<String>,
    agent: ureq::Agent,
}

/// A note to add: its deck, note type and fields (name, value: HTML).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub deck: String,
    pub model: String,
    pub fields: Vec<(String, String)>,
    pub tags: Vec<String>,
    /// The duplicate check looks at notes of every type, not only this one.
    pub check_all_models: bool,
}

/// Whether a note could be added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanAdd {
    Yes,
    /// Its first field matches a note already in the collection.
    Duplicate,
    /// Anything else AnkiConnect objects to (an empty first field, a deck or
    /// note type not found).
    No(String),
}

impl Note {
    fn to_json(&self, allow_duplicate: bool) -> Value {
        let fields: serde_json::Map<String, Value> = self
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        json!({
            "deckName": self.deck,
            "modelName": self.model,
            "fields": fields,
            "tags": self.tags,
            "options": {
                "allowDuplicate": allow_duplicate,
                "duplicateScope": "collection",
                "duplicateScopeOptions": {
                    "checkChildren": false,
                    "checkAllModels": self.check_all_models,
                },
            },
        })
    }
}

impl Client {
    /// `url`: AnkiConnect's address; `key`: its API key, if one is set.
    pub fn new(url: &str, key: Option<&str>) -> Client {
        let agent = ureq::Agent::config_builder()
            // Never through a proxy from the environment: Anki is local.
            .proxy(None)
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
            .into();
        Client {
            url: url.trim().trim_end_matches('/').to_owned(),
            key: key
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_owned),
            agent,
        }
    }

    /// Call `action`; its `result`, or the error AnkiConnect (or the
    /// connection) gave.
    pub fn call(&self, action: &str, params: Value) -> Result<Value, String> {
        let mut body = json!({ "action": action, "version": 6, "params": params });
        if let Some(k) = &self.key {
            body["key"] = Value::String(k.clone());
        }
        let mut resp = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json; charset=utf-8")
            .send(body.to_string())
            .map_err(|e| match e {
                ureq::Error::Io(_)
                | ureq::Error::ConnectionFailed
                | ureq::Error::HostNotFound
                | ureq::Error::Timeout(ureq::Timeout::Connect | ureq::Timeout::Resolve) => {
                    format!(
                        "Anki isn't answering at {}: is it running, with AnkiConnect installed?",
                        self.url
                    )
                }
                ureq::Error::Timeout(_) => {
                    "Anki took too long to answer (is a dialog open in it?)".to_owned()
                }
                e => e.to_string(),
            })?;
        let status = resp.status();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("AnkiConnect answered {status}: {}", text.trim()));
        }
        parse_reply(&text)
    }

    /// The API version (6 for every current AnkiConnect).
    pub fn version(&self) -> Result<u64, String> {
        self.call("version", json!({}))?
            .as_u64()
            .ok_or_else(|| "Not AnkiConnect: no version".to_owned())
    }

    pub fn deck_names(&self) -> Result<Vec<String>, String> {
        strings(self.call("deckNames", json!({}))?)
    }

    pub fn model_names(&self) -> Result<Vec<String>, String> {
        strings(self.call("modelNames", json!({}))?)
    }

    /// The note type's fields, in order.
    pub fn model_field_names(&self, model: &str) -> Result<Vec<String>, String> {
        strings(self.call("modelFieldNames", json!({ "modelName": model }))?)
    }

    /// Whether each note could be added.
    pub fn can_add(&self, notes: &[Note]) -> Result<Vec<CanAdd>, String> {
        if notes.is_empty() {
            return Ok(Vec::new());
        }
        let v = self.call(
            "canAddNotesWithErrorDetail",
            json!({ "notes": notes.iter().map(|n| n.to_json(false)).collect::<Vec<_>>() }),
        )?;
        let a = v
            .as_array()
            .ok_or("canAddNotesWithErrorDetail: not a list")?;
        if a.len() != notes.len() {
            return Err("canAddNotesWithErrorDetail: wrong number of answers".into());
        }
        Ok(a.iter().map(can_add_of).collect())
    }

    /// Add `note` (refused if it is a duplicate); its id.
    pub fn add_note(&self, note: &Note) -> Result<i64, String> {
        self.call("addNote", json!({ "note": note.to_json(false) }))?
            .as_i64()
            .ok_or_else(|| "addNote: no note id".to_owned())
    }

    /// Show notes matching `query` in Anki's browser.
    /// Copy the file at `path` into Anki's media folder as `filename`
    /// (replacing one of that name): the name Anki gave it.
    pub fn store_media(&self, filename: &str, path: &std::path::Path) -> Result<String, String> {
        let v = self.call(
            "storeMediaFile",
            json!({ "filename": filename, "path": path.to_string_lossy() }),
        )?;
        match v {
            Value::String(s) => Ok(s),
            Value::Null => Ok(filename.to_owned()),
            other => Err(format!("Anki stored the audio but said {other}")),
        }
    }

    pub fn gui_browse(&self, query: &str) -> Result<(), String> {
        self.call("guiBrowse", json!({ "query": query }))
            .map(|_| ())
    }
}

/// A reply's `result`, or its `error`.
fn parse_reply(text: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| {
        format!(
            "Not AnkiConnect's answer: {}",
            text.chars().take(120).collect::<String>()
        )
    })?;
    match v.get("error") {
        Some(Value::String(e)) => Err(e.clone()),
        Some(Value::Null) | None if v.get("result").is_some() => Ok(v["result"].clone()),
        // Very old versions answer the bare result.
        _ => Ok(v),
    }
}

fn can_add_of(v: &Value) -> CanAdd {
    if v.get("canAdd").and_then(Value::as_bool) == Some(true) {
        return CanAdd::Yes;
    }
    match v.get("error").and_then(Value::as_str) {
        Some(e) if e.contains(DUPLICATE) => CanAdd::Duplicate,
        Some(e) => CanAdd::No(e.to_owned()),
        None => CanAdd::No("can't be added".into()),
    }
}

fn strings(v: Value) -> Result<Vec<String>, String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .ok_or_else(|| "Expected a list of names".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(parse_reply(r#"{"result": 6, "error": null}"#), Ok(json!(6)));
        assert_eq!(
            parse_reply(r#"{"result": null, "error": "unsupported action"}"#),
            Err("unsupported action".into())
        );
        assert!(parse_reply("<html>").is_err());
    }

    #[test]
    fn can_add_answers() {
        // As AnkiConnect (Extended, Anki 26.9) answers.
        let v: Value = serde_json::from_str(
            r#"[{"canAdd": false, "error": "cannot create note because it is a duplicate"},
                {"canAdd": false, "error": "cannot create note because it is empty"},
                {"canAdd": true}]"#,
        )
        .unwrap();
        let got: Vec<CanAdd> = v.as_array().unwrap().iter().map(can_add_of).collect();
        assert_eq!(
            got,
            [
                CanAdd::Duplicate,
                CanAdd::No("cannot create note because it is empty".into()),
                CanAdd::Yes
            ]
        );
    }

    #[test]
    fn note_json() {
        let n = Note {
            deck: "中文".into(),
            model: "Lapis Chinese".into(),
            fields: vec![("Expression".into(), "说".into())],
            tags: vec!["hanstat".into()],
            check_all_models: true,
        };
        let j = n.to_json(false);
        assert_eq!(j["fields"]["Expression"], "说");
        assert_eq!(j["options"]["duplicateScope"], "collection");
        assert_eq!(
            j["options"]["duplicateScopeOptions"]["checkAllModels"],
            true
        );
    }

    #[test]
    fn media_is_stored_from_a_path() {
        // A stand-in for AnkiConnect on a free port (never the user's Anki).
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then the body (Content-Length long).
            loop {
                let n = sock.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some(h) = text.find("\r\n\r\n") {
                    let len: usize = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if req.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let body = r#"{"result": "hanstat_ab_1.mp3", "error": null}"#;
            write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8(req).unwrap()
        });
        let c = Client::new(&format!("http://127.0.0.1:{port}"), None);
        let path = std::path::Path::new("C:/clips/ab.mp3");
        assert_eq!(
            c.store_media("hanstat_ab.mp3", path).unwrap(),
            "hanstat_ab_1.mp3",
            "Anki's name is used"
        );
        let req = server.join().unwrap();
        let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["action"], "storeMediaFile");
        assert_eq!(body["params"]["filename"], "hanstat_ab.mp3");
        assert_eq!(body["params"]["path"], "C:/clips/ab.mp3");
    }

    #[test]
    fn nothing_listening() {
        // A port nothing listens on: a clear error, quickly.
        let c = Client::new("http://127.0.0.1:9", None);
        let t = std::time::Instant::now();
        let e = c.version().unwrap_err();
        assert!(e.contains("isn't answering"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
