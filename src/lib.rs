pub mod analyze;
pub mod anki;
pub mod cache;
pub mod chars;
pub mod child_guard;
pub mod ckip;
pub mod discover;
pub mod extract;
pub mod history;
pub mod japanese;
pub mod kiwi;
pub mod known;
pub mod lang;
pub mod models;
pub mod plan;
pub mod profile;
pub mod saver;
pub mod script;
pub mod search;
pub mod segment;
pub mod series;
pub mod settings;
pub mod style;
pub mod update;

/// Read a user dictionary: one word per line; anything after the first
/// whitespace (jieba-style frequency/tag) is ignored; `#` starts a comment.
pub fn read_user_dict(path: &std::path::Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Cannot read dictionary {}: {e}", path.display()))?;
    let (text, _, _, _) = extract::txt::decode(&bytes, extract::EncodingChoice::Auto);
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect())
}

/// Where the app keeps `lang`'s forced words: `dictionary.txt` (Chinese),
/// `dictionary-ko.txt`, `dictionary-ja.txt` in the app folder, in the format
/// [`read_user_dict`] reads, so the file also works with `hanstat-cli --dict`.
pub fn user_dict_path(lang: lang::Language) -> std::path::PathBuf {
    models::app_dir().join(format!("dictionary{}.txt", lang.file_suffix()))
}

/// The app's saved forced words for `lang` (none if the file doesn't exist),
/// with a warning if it exists but can't be read.
pub fn load_user_dict(lang: lang::Language) -> (Vec<String>, Option<String>) {
    load_user_dict_from(&user_dict_path(lang))
}

fn load_user_dict_from(p: &std::path::Path) -> (Vec<String>, Option<String>) {
    if !p.exists() {
        return (Vec::new(), None);
    }
    match read_user_dict(p) {
        Ok(mut w) => {
            let mut seen = std::collections::HashSet::new();
            w.retain(|x| seen.insert(x.clone()));
            (w, None)
        }
        Err(e) => (Vec::new(), Some(e)),
    }
}

/// Save `lang`'s forced words (an empty list removes the file).
pub fn save_user_dict(lang: lang::Language, words: &[String]) -> Result<(), String> {
    save_user_dict_to(&user_dict_path(lang), words)
}

fn save_user_dict_to(p: &std::path::Path, words: &[String]) -> Result<(), String> {
    let err = |e: std::io::Error| format!("Cannot save {}: {e}", p.display());
    if words.is_empty() {
        return match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        };
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(err)?;
    }
    let mut text = String::from(
        "# hanstat forced words: each is always counted as one word. One per line; # starts a comment.\n",
    );
    for w in words {
        text.push_str(w);
        text.push('\n');
    }
    std::fs::write(p, text).map_err(err)
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_dict_round_trip() {
        let dir = std::env::temp_dir().join(format!("hanstat-dict-{}", std::process::id()));
        let p = dir.join("sub").join("dictionary.txt");
        let words: Vec<String> = ["蒋舸", "林原", "蒋舸"].map(str::to_owned).to_vec();
        super::save_user_dict_to(&p, &words).unwrap();
        let (back, warn) = super::load_user_dict_from(&p);
        assert_eq!(back, ["蒋舸", "林原"]);
        assert!(warn.is_none());
        assert_eq!(
            super::read_user_dict(&p).unwrap().len(),
            3,
            "the format --dict reads"
        );
        super::save_user_dict_to(&p, &[]).unwrap();
        assert!(!p.exists());
        assert_eq!(super::load_user_dict_from(&p), (Vec::new(), None));
        let _ = std::fs::remove_dir_all(dir);
    }
}
