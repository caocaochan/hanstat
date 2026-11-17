# Deferred improvements

Left over from the performance and UI/UX audit of 2026-11-17 (release 2026.11.17.10).
The other findings of that audit were fixed in the release after it.

## Performance

### Changing the imported word lists invalidates every saved Chinese analysis

The known words given to the segmenter are part of the analysis-cache key
(`src/cache.rs`, `Key::new`). Importing or editing a Chinese word list therefore
makes the Library's background update re-segment every Chinese book; for
Traditional books on CKIP that is minutes each, with no warning.

- At least: run the books whose analysis is not cached last, and say in the
  status bar that they need a full analysis (and roughly how long).
- The cache keeps entries of older analysis code until the 1 GB cap
  (`MAX_BYTES`); 419 entries / 397 MB at the time of the audit. Putting the code
  fingerprint in the file name (or a per-fingerprint folder) would let startup
  delete the ones no release can read any more.

### Redundant scans of the book text

Language and script detection run in `App::start` (`src/main.rs`), again in
`analyze::analyze_book_cached` (`src/analyze.rs`), and again in
`discover::discover_with` (`src/discover.rs`). Each is a parallel pass over the
whole text: a few milliseconds per megabyte, so low priority. Pass the detected
language/variant down instead of detecting again.

## UI and UX

### Example sentences for the selected word

In the Words and Suggested words tabs, show a few sentences of the book that
contain the selected word (a concordance pane below or beside the table).
Deciding whether 简言 is a name, or whether to mark a word known, is hard
without context. The book is in memory (`Arc<Book>`), so finding them is cheap.
A right-click menu with Copy and a dictionary lookup would fit with it.

### Keyboard shortcuts

Only the Words tab's arrows/A and Esc in the Library search exist. Useful:
Ctrl+O open, Ctrl+F focus the current tab's filter, Ctrl+1…6 the book's tabs,
Ctrl+L the Library, Esc to close the floating windows.

### The same 📖 icon for two things

"📖 N forced words" and "📖 Library" in the toolbar share the icon. Give the
forced words another one (e.g. 📌 or 🔒).
