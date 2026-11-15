pub mod analyze;
pub mod anki;
pub mod chars;
pub mod ckip;
pub mod discover;
pub mod extract;
pub mod kiwi;
pub mod known;
pub mod lang;
pub mod models;
pub mod script;
pub mod segment;
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
