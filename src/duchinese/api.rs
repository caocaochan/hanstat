//! Talking to duchinese.net: the catalogue, a story's chapters, a chapter's
//! text, cover images, and signing in.
//!
//! Signing in is the site's own form (`/accounts/sign_in`, Rails/Devise):
//! the password goes to duchinese.net once and is never kept; what is kept
//! is the session cookie it hands back ([`Session`], saved encrypted by the
//! window). Cookies are handled here, by hand, since ureq's cookie store is
//! a crate feature this build doesn't have (a new feature would change the
//! analysis fingerprint and re-analyse every Library).
//!
//! Requests are spaced out ([`GAP`]) so that fetching a whole story doesn't
//! hammer the site.

use super::data::{Course, Lesson};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BASE: &str = "https://duchinese.net";
/// Where chapter texts, audio and images are served from.
const STATIC: &str = "https://static.duchinese.net/";
/// The least time between two requests.
const GAP: Duration = Duration::from_millis(400);
/// The catalogue's two kinds of series. (`article_collections` is a subset
/// of `courses`.)
pub const CATEGORIES: [&str; 2] = ["short_stories", "courses"];
/// Pages of a catalogue listing followed at most.
const MAX_PAGES: usize = 40;

/// What the window keeps of a sign-in, encrypted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// `name=value` pairs, as sent back in `Cookie`.
    pub cookies: BTreeMap<String, String>,
    /// The email signed in with, shown in the window (empty when pasted).
    pub email: String,
}

impl Session {
    /// From what a browser's developer tools show: `a=b; c=d`, or one
    /// value alone, taken as the session cookie.
    pub fn from_pasted(s: &str) -> Option<Session> {
        let s = s.trim().trim_start_matches("Cookie:").trim();
        let mut cookies = BTreeMap::new();
        for part in s.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                if !k.is_empty() && !v.is_empty() && !k.contains(char::is_whitespace) {
                    cookies.insert(k.to_owned(), v.to_owned());
                }
            }
        }
        if cookies.is_empty() && !s.is_empty() && !s.contains(char::is_whitespace) {
            cookies.insert(SESSION_COOKIE.to_owned(), s.to_owned());
        }
        (!cookies.is_empty()).then_some(Session {
            cookies,
            email: String::new(),
        })
    }

    fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The site's session cookie (seen 2026-11-23).
pub const SESSION_COOKIE: &str = "_reader-server_session";

#[derive(Debug)]
pub enum Error {
    /// No network, or the site didn't answer.
    Offline(String),
    /// Signed in no longer (or never): the site asked for a sign-in.
    SignedOut,
    /// Anything else, said as is.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Offline(e) => write!(f, "DuChinese can't be reached ({e})"),
            Error::SignedOut => write!(f, "Your DuChinese session expired: sign in again"),
            Error::Other(e) => f.write_str(e),
        }
    }
}

pub struct Client {
    agent: ureq::Agent,
    session: Mutex<Session>,
    last: Mutex<Option<Instant>>,
}

impl Client {
    pub fn new(session: Session) -> Client {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            // Redirects are followed by hand, keeping the cookies they set.
            .max_redirects(0)
            .user_agent(concat!("hanstat/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Client {
            agent,
            session: Mutex::new(session),
            last: Mutex::new(None),
        }
    }

    pub fn session(&self) -> Session {
        self.session.lock().unwrap().clone()
    }

    pub fn set_session(&self, s: Session) {
        *self.session.lock().unwrap() = s;
    }

    /// Wait so that requests are at least [`GAP`] apart.
    fn pace(&self) {
        let mut last = self.last.lock().unwrap();
        if let Some(t) = *last {
            let since = t.elapsed();
            if since < GAP {
                std::thread::sleep(GAP - since);
            }
        }
        *last = Some(Instant::now());
    }

    fn absorb(&self, resp: &ureq::http::Response<ureq::Body>) {
        let mut s = self.session.lock().unwrap();
        for v in resp.headers().get_all("set-cookie") {
            let Ok(v) = v.to_str() else { continue };
            let first = v.split(';').next().unwrap_or("");
            let Some((k, val)) = first.split_once('=') else {
                continue;
            };
            let (k, val) = (k.trim(), val.trim());
            let expired = v.to_ascii_lowercase().contains("max-age=0")
                || v.to_ascii_lowercase().contains("expires=thu, 01 jan 1970");
            if val.is_empty() || expired {
                s.cookies.remove(k);
            } else if !k.is_empty() {
                s.cookies.insert(k.to_owned(), val.to_owned());
            }
        }
    }

    /// GET `url` (a path on the site, or a full URL on it or the static
    /// host), following redirects on the site. Status and body.
    fn get(&self, url: &str) -> Result<(u16, Vec<u8>, String), Error> {
        let mut url = absolute(url)?;
        for _ in 0..5 {
            self.pace();
            let mut req = self.agent.get(&url);
            if on_site(&url) {
                let cookie = self.session.lock().unwrap().header();
                if !cookie.is_empty() {
                    req = req.header("Cookie", &cookie);
                }
                req = req.header("Accept", "application/json, text/html;q=0.9, */*;q=0.5");
            }
            let mut resp = req.call().map_err(offline)?;
            if on_site(&url) {
                self.absorb(&resp);
            }
            let status = resp.status().as_u16();
            if (300..400).contains(&status)
                && let Some(loc) = resp.headers().get("location").and_then(|l| l.to_str().ok())
            {
                let next = absolute(loc)?;
                if next.contains("/accounts/sign_in") {
                    return Err(Error::SignedOut);
                }
                url = next;
                continue;
            }
            let body = resp
                .body_mut()
                .with_config()
                .limit(50 << 20)
                .read_to_vec()
                .map_err(offline)?;
            return Ok((status, body, url));
        }
        Err(Error::Other(format!(
            "DuChinese redirected too often ({url})"
        )))
    }

    fn get_ok(&self, url: &str) -> Result<Vec<u8>, Error> {
        match self.get(url)? {
            (200, body, _) => Ok(body),
            (401, _, _) => Err(Error::SignedOut),
            (s @ (429 | 500..), _, _) => Err(Error::Offline(format!("it answered {s}"))),
            (s, _, u) => Err(Error::Other(format!("DuChinese answered {s} for {u}"))),
        }
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, Error> {
        let body = self.get_ok(url)?;
        serde_json::from_slice(&body).map_err(|e| {
            Error::Other(format!(
                "DuChinese sent something hanstat can't read from {url} ({e})"
            ))
        })
    }

    /// Every series of `category` ([`CATEGORIES`]), following the pages.
    pub fn courses(&self, category: &str) -> Result<Vec<Course>, Error> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            courses: Vec<Course>,
            next_page_url: Option<String>,
        }
        let mut out = Vec::new();
        let mut url = Some(format!("/lessons/courses.json?category={category}"));
        for _ in 0..MAX_PAGES {
            let Some(u) = url.take() else { break };
            let page: Page = self.json(&u)?;
            out.extend(page.courses);
            url = page.next_page_url.filter(|u| !u.is_empty());
        }
        Ok(out)
    }

    /// The latest chapters released (they carry their series: ones too new
    /// for the catalogue's listings show up here).
    pub fn latest(&self) -> Result<Vec<Lesson>, Error> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            lessons: Vec<Lesson>,
        }
        Ok(self.json::<Page>("/lessons/latest.json")?.lessons)
    }

    /// A series' chapters, in reading order.
    pub fn lessons(&self, course: &Course) -> Result<Vec<Lesson>, Error> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            lessons: Vec<Lesson>,
        }
        if !course.lessons_url.starts_with('/') {
            return Err(Error::Other(format!(
                "{} has no chapter list on DuChinese",
                course.title
            )));
        }
        let page: Page = self.json(&course.lessons_url)?;
        Ok(super::data::in_order(page.lessons))
    }

    /// A chapter's text file, or an image, from the static host.
    pub fn file(&self, url: &str) -> Result<Vec<u8>, Error> {
        if !url.starts_with(STATIC) {
            return Err(Error::Other(format!("not a DuChinese file: {url}")));
        }
        self.get_ok(url)
    }

    /// Whether the session is signed in (`None`: the site couldn't tell).
    pub fn signed_in(&self) -> Result<bool, Error> {
        if self.session.lock().unwrap().cookies.is_empty() {
            return Ok(false);
        }
        match self.get("/flashcards/status.json") {
            Ok((200, _, _)) => Ok(true),
            Ok((401 | 403, _, _)) | Err(Error::SignedOut) => Ok(false),
            Ok((s, _, _)) => Err(Error::Other(format!(
                "DuChinese answered {s} when asked who is signed in"
            ))),
            Err(e) => Err(e),
        }
    }

    /// Sign in with the site's form. On success the session holds the
    /// signed-in cookie.
    pub fn sign_in(&self, email: &str, password: &str) -> Result<(), Error> {
        let (status, page, _) = self.get("/accounts/sign_in")?;
        if status != 200 {
            return Err(Error::Other(format!(
                "DuChinese's sign-in page answered {status}"
            )));
        }
        let page = String::from_utf8_lossy(&page);
        let token = form_token(&page).ok_or_else(|| {
            Error::Other("DuChinese's sign-in page has changed: hanstat can't sign in".to_owned())
        })?;
        self.pace();
        let cookie = self.session.lock().unwrap().header();
        let resp = self
            .agent
            .post(format!("{BASE}/accounts/sign_in"))
            .header("Cookie", &cookie)
            .header("Origin", BASE)
            .header("Referer", format!("{BASE}/accounts/sign_in"))
            .send_form([
                ("authenticity_token", token.as_str()),
                ("account[email]", email.trim()),
                ("account[password]", password),
            ])
            .map_err(offline)?;
        self.absorb(&resp);
        let status = resp.status().as_u16();
        let to = resp
            .headers()
            .get("location")
            .and_then(|l| l.to_str().ok())
            .unwrap_or("");
        let accepted = (300..400).contains(&status) && !to.contains("/accounts/sign_in");
        if !accepted {
            return Err(Error::Other(match status {
                429 => "DuChinese refused for now (too many tries): wait a little".to_owned(),
                500.. => format!("DuChinese answered {status}: try again later"),
                _ => "DuChinese didn't accept that email and password".to_owned(),
            }));
        }
        if !self.signed_in()? {
            return Err(Error::Other(
                "DuChinese accepted the sign-in but doesn't show you signed in".to_owned(),
            ));
        }
        self.session.lock().unwrap().email = email.trim().to_owned();
        Ok(())
    }
}

fn offline(e: ureq::Error) -> Error {
    match e {
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::Io(_)
        | ureq::Error::Timeout(_) => Error::Offline(e.to_string()),
        e => Error::Other(format!("DuChinese can't be reached: {e}")),
    }
}

/// A path or URL as a URL on the site or its static host.
fn absolute(url: &str) -> Result<String, Error> {
    if url.starts_with('/') && !url.starts_with("//") {
        Ok(format!("{BASE}{url}"))
    } else if on_site(url) || url.starts_with(STATIC) {
        Ok(url.to_owned())
    } else {
        Err(Error::Other(format!(
            "DuChinese pointed outside its site ({url})"
        )))
    }
}

/// On duchinese.net itself (where the session cookie may go).
fn on_site(url: &str) -> bool {
    url.strip_prefix(BASE).is_some_and(|r| r.starts_with('/'))
}

/// The sign-in form's `authenticity_token`.
fn form_token(page: &str) -> Option<String> {
    let form = page
        .find("action=\"/accounts/sign_in\"")
        .map_or(page, |i| &page[i..]);
    let i = form.find("name=\"authenticity_token\"")?;
    // The input's attributes, on either side of its name.
    let start = form[..i].rfind('<')?;
    let end = i + form[i..].find('>')?;
    let tag = &form[start..end];
    let v = tag.find("value=\"")? + 7;
    let value = &tag[v..v + tag[v..].find('"')?];
    (!value.is_empty()).then(|| value.replace("&amp;", "&").replace("&#x2F;", "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_token_is_found() {
        let page = r#"<form action="/search"><input type="hidden" name="authenticity_token" value="wrong"></form>
            <form class="x" action="/accounts/sign_in" method="post">
            <input type="hidden" value="AbC+/1&amp;2" name="authenticity_token" autocomplete="off" />
            <input type="email" name="account[email]"></form>"#;
        assert_eq!(form_token(page).as_deref(), Some("AbC+/1&2"));
        assert_eq!(form_token("<html></html>"), None);
    }

    #[test]
    fn pasted_cookies() {
        let s = Session::from_pasted("Cookie: _reader-server_session=abc%3D; other=1").unwrap();
        assert_eq!(s.cookies["_reader-server_session"], "abc%3D");
        assert_eq!(s.cookies["other"], "1");
        assert_eq!(s.header(), "_reader-server_session=abc%3D; other=1");
        let s = Session::from_pasted("  justthevalue  ").unwrap();
        assert_eq!(s.cookies[SESSION_COOKIE], "justthevalue");
        assert!(Session::from_pasted("   ").is_none());
        assert!(Session::from_pasted("two words").is_none());
    }

    #[test]
    fn only_the_site_is_fetched() {
        assert_eq!(
            absolute("/lessons/latest.json").unwrap(),
            "https://duchinese.net/lessons/latest.json"
        );
        assert!(absolute("https://static.duchinese.net/documents/1/x.crd").is_ok());
        assert!(absolute("https://evil.example/x").is_err());
        assert!(absolute("//evil.example/x").is_err());
        assert!(absolute("https://duchinese.net.evil.example/x").is_err());
        assert!(on_site("https://duchinese.net/x") && !on_site("https://duchinese.netx/"));
    }
}
