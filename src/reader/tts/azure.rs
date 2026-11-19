//! Azure AI Speech text to speech over its REST API: one request per
//! sentence, MP3 back. The key goes in `Ocp-Apim-Subscription-Key` (bearer
//! tokens last ten minutes and only add a request).
//!
//! <https://learn.microsoft.com/azure/ai-services/speech-service/rest-text-to-speech>

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 24 kHz mono MP3 at 48 kbit/s: about 6 KB a second, so a whole book's
/// audio stays in the hundreds of megabytes.
pub const FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// Why a request failed.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// 429: too many requests; wait this long (Retry-After) and again.
    Throttled(Option<Duration>),
    /// The key or region was refused: no use trying again.
    Refused(String),
    /// Anything else (network, 5xx): worth another try.
    Other(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Throttled(_) => f.write_str("Azure is busy (too many requests)"),
            Fail::Refused(s) | Fail::Other(s) => f.write_str(s),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Voice {
    pub short_name: String,
    pub display_name: String,
    pub local_name: String,
    pub gender: String,
    pub locale: String,
    pub locale_name: String,
    pub voice_type: String,
    pub status: String,
    pub voice_tag: VoiceTag,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct VoiceTag {
    pub tailored_scenarios: Vec<String>,
}

impl Voice {
    /// How the voice is listed: its Chinese name, English name, gender and
    /// what it was made for.
    pub fn label(&self) -> String {
        let mut s = if self.local_name.is_empty() || self.local_name == self.display_name {
            self.display_name.clone()
        } else {
            format!("{} {}", self.local_name, self.display_name)
        };
        if !self.gender.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.gender);
        }
        if !self.voice_tag.tailored_scenarios.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.voice_tag.tailored_scenarios.join(", "));
        }
        s
    }
}

/// The voices worth offering for Chinese books: Chinese (Mandarin,
/// Cantonese, Wu and the regional ones), neural (Azure's natural voices,
/// HD ones included), not deprecated or in preview. Mandarin first
/// (Simplified, then Taiwan), then the others; by name within each.
pub fn chinese_voices(all: Vec<Voice>) -> Vec<Voice> {
    let mut v: Vec<Voice> = all
        .into_iter()
        .filter(|v| {
            v.locale_name.starts_with("Chinese")
                && v.voice_type.contains("Neural")
                && v.status.eq_ignore_ascii_case("GA")
        })
        .collect();
    let rank = |v: &Voice| match v.locale.as_str() {
        "zh-CN" => 0,
        "zh-TW" => 1,
        "zh-HK" => 2,
        "yue-CN" => 3,
        _ => 4,
    };
    v.sort_by(|a, b| {
        (rank(a), &a.locale_name, &a.short_name).cmp(&(rank(b), &b.locale_name, &b.short_name))
    });
    v
}

/// The default voice for a book: Xiaoxiao (made for audiobooks among
/// others) for Simplified, HsiaoChen for Traditional.
pub fn default_voice(traditional: bool) -> &'static str {
    if traditional {
        "zh-TW-HsiaoChenNeural"
    } else {
        "zh-CN-XiaoxiaoNeural"
    }
}

/// A region as Azure names it (`eastus`, `westeurope`): it goes
/// into a host name.
pub fn valid_region(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 40
        && r.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Not allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// The SSML for `text`: `rate` is the speed change in percent (0: the
/// voice's own).
pub fn ssml(text: &str, voice: &str, rate: i32) -> String {
    let lang = voice.rsplit_once('-').map_or("zh-CN", |(l, _)| l);
    let body = if rate == 0 {
        escape(text)
    } else {
        format!("<prosody rate='{rate:+}%'>{}</prosody>", escape(text))
    };
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{lang}'>\
         <voice name='{}'>{body}</voice></speak>",
        escape(voice)
    )
}

/// Characters Azure bills for `text` read with `rate`: everything inside
/// `<voice>` (the prosody tag too), each Chinese character twice.
pub fn billed(text: &str, rate: i32) -> usize {
    let tag = if rate == 0 {
        0
    } else {
        "<prosody rate='+00%'></prosody>".len()
    };
    tag + text
        .chars()
        .map(|c| if hanstat::chars::is_han(c) { 2 } else { 1 })
        .sum::<usize>()
}

pub struct Azure {
    agent: ureq::Agent,
    region: String,
    key: String,
}

impl Azure {
    pub fn new(region: &str, key: &str) -> Azure {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .user_agent(concat!("hanstat/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Azure {
            agent,
            region: region.trim().to_owned(),
            key: key.trim().to_owned(),
        }
    }

    fn host(&self) -> Result<String, Fail> {
        if !valid_region(&self.region) {
            return Err(Fail::Refused(format!(
                "“{}” isn't an Azure region name (such as eastus or westeurope)",
                self.region
            )));
        }
        Ok(format!("https://{}.tts.speech.microsoft.com", self.region))
    }

    fn net(e: ureq::Error) -> Fail {
        match e {
            ureq::Error::HostNotFound => {
                Fail::Other("Azure can't be reached: no network, or a wrong region".to_owned())
            }
            ureq::Error::Timeout(_) => Fail::Other("Azure took too long to answer".to_owned()),
            e => Fail::Other(format!("Azure can't be reached: {e}")),
        }
    }

    fn status(status: u16, body: &str, retry_after: Option<Duration>) -> Fail {
        let detail = body.trim();
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {}", detail.chars().take(200).collect::<String>())
        };
        match status {
            401 | 403 => Fail::Refused(format!(
                "Azure refused the key ({status}): check the key and that the region is the resource's own"
            )),
            429 => Fail::Throttled(retry_after),
            400 | 404 | 415 => {
                Fail::Refused(format!("Azure refused the request ({status}){detail}"))
            }
            _ => Fail::Other(format!("Azure answered {status}{detail}")),
        }
    }

    /// `text` read by `voice`, as MP3 ([`FORMAT`]).
    pub fn synth(&self, text: &str, voice: &str, rate: i32) -> Result<Vec<u8>, Fail> {
        let url = format!("{}/cognitiveservices/v1", self.host()?);
        let mut resp = self
            .agent
            .post(&url)
            .header("Ocp-Apim-Subscription-Key", &self.key)
            .header("Content-Type", "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", FORMAT)
            .send(ssml(text, voice, rate))
            .map_err(Self::net)?;
        let status = resp.status().as_u16();
        if status != 200 {
            let retry = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(Self::status(status, &body, retry));
        }
        let audio = resp
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()
            .map_err(Self::net)?;
        if audio.is_empty() {
            return Err(Fail::Other("Azure sent no audio".to_owned()));
        }
        Ok(audio)
    }

    /// Every voice of the region.
    pub fn voices(&self) -> Result<Vec<Voice>, Fail> {
        let url = format!("{}/cognitiveservices/voices/list", self.host()?);
        let mut resp = self
            .agent
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", &self.key)
            .call()
            .map_err(Self::net)?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(16 << 20)
            .read_to_string()
            .map_err(Self::net)?;
        if status != 200 {
            return Err(Self::status(status, &body, None));
        }
        serde_json::from_str(&body).map_err(|e| Fail::Other(format!("Azure's voice list: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssml_escapes_and_names_the_language() {
        let s = ssml("“你好”<b>&'", "zh-CN-henan-YundengNeural", 0);
        assert_eq!(
            s,
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='zh-CN-henan'>\
             <voice name='zh-CN-henan-YundengNeural'>“你好”&lt;b&gt;&amp;&apos;</voice></speak>"
        );
        let s = ssml("好", "zh-TW-HsiaoChenNeural", -20);
        assert!(s.contains("<prosody rate='-20%'>好</prosody>"));
        assert!(ssml("好", "zh-CN-XiaoxiaoNeural", 15).contains("rate='+15%'"));
    }

    #[test]
    fn billing_counts_hanzi_twice() {
        assert_eq!(billed("你好，ab", 0), 2 + 2 + 1 + 1 + 1);
        assert!(billed("你好", 10) > billed("你好", 0));
    }

    #[test]
    fn regions() {
        assert!(valid_region("westeurope"));
        assert!(valid_region("eastus2"));
        assert!(!valid_region("evil.com/x"));
        assert!(!valid_region(""));
        assert!(!valid_region("EastUS"));
    }

    #[test]
    fn voice_filter() {
        let v = |name: &str, locale: &str, locale_name: &str, ty: &str, status: &str| Voice {
            short_name: name.into(),
            locale: locale.into(),
            locale_name: locale_name.into(),
            voice_type: ty.into(),
            status: status.into(),
            ..Voice::default()
        };
        let all = vec![
            v(
                "en-US-JennyNeural",
                "en-US",
                "English (United States)",
                "Neural",
                "GA",
            ),
            v(
                "zh-TW-HsiaoChenNeural",
                "zh-TW",
                "Chinese (Taiwanese Mandarin, Traditional)",
                "Neural",
                "GA",
            ),
            v(
                "zh-CN-YunxiNeural",
                "zh-CN",
                "Chinese (Mandarin, Simplified)",
                "Neural",
                "GA",
            ),
            v(
                "zh-CN-OldStandard",
                "zh-CN",
                "Chinese (Mandarin, Simplified)",
                "Standard",
                "GA",
            ),
            v(
                "zh-CN-GoneNeural",
                "zh-CN",
                "Chinese (Mandarin, Simplified)",
                "Neural",
                "Deprecated",
            ),
            v(
                "zh-CN-NewNeural",
                "zh-CN",
                "Chinese (Mandarin, Simplified)",
                "Neural",
                "Preview",
            ),
            v(
                "wuu-CN-XiaotongNeural",
                "wuu-CN",
                "Chinese (Wu, Simplified)",
                "Neural",
                "GA",
            ),
            v(
                "zh-CN-XiaoxiaoNeural",
                "zh-CN",
                "Chinese (Mandarin, Simplified)",
                "Neural",
                "GA",
            ),
        ];
        let names: Vec<String> = chinese_voices(all)
            .into_iter()
            .map(|v| v.short_name)
            .collect();
        assert_eq!(
            names,
            [
                "zh-CN-XiaoxiaoNeural",
                "zh-CN-YunxiNeural",
                "zh-TW-HsiaoChenNeural",
                "wuu-CN-XiaotongNeural"
            ]
        );
    }
}
