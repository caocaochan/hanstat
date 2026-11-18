//! Yomitan dictionaries for the reader's popup: importing their zips into
//! hanstat's own files ([`format`], [`import`]), looking text up in them
//! ([`store`], [`lookup`], on a worker thread: [`service`]), managing the
//! list ([`manage`]) and updating them from their authors' sites
//! ([`revision`], [`update`]). [`sc`] turns their structured-content
//! glossaries into HTML for the popup.
//!
//! The design follows hoshidicts (Manhhao; GPL, so no code of it is used):
//! glossaries compressed once each with a trained zstd dictionary, hashed
//! headword indexes in memory-mapped files.

mod bloom;
pub mod format;
pub mod import;
pub mod jobs;
pub mod lookup;
pub mod manage;
pub mod revision;
pub mod sc;
pub mod service;
pub mod store;
#[cfg(test)]
mod tests;
pub mod update;
pub mod xxh64;

/// Where the dictionaries are kept: `dictionaries/` in the app folder.
pub fn root() -> std::path::PathBuf {
    crate::models::app_dir().join("dictionaries")
}
