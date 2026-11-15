# Changelog

## [0.4.1] - 2026-11-15

### Changed
- hanstat has its own icon, replacing egui's default window icon. `hanstat.exe` and `hanstat-cli.exe` carry it too, so Explorer, shortcuts and pinned taskbar entries show it instead of the generic program icon.

## [0.4.0] - 2026-11-15

### Added
- **Automatic updates.** At startup the app checks GitHub for a newer release. It downloads it in the background, verifies its SHA-256, and replaces `hanstat.exe`, `hanstat-cli.exe` and the docs in place. Then the status bar offers a restart, which reopens the current file and tab. If any step fails, every file is put back. Click the version number in the status bar to check now or to turn automatic updates off. Releases before this one can't update themselves, so download this one by hand.

### Changed
- The release zip no longer has the version in its name. It's always `hanstat-x86_64-pc-windows-msvc.zip`, so this link always downloads the newest build: <https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-pc-windows-msvc.zip>. The folder inside it is now just `hanstat`, so to update by hand, replace your `hanstat` folder with the new one.

## [0.3.1] - 2026-11-15

### Fixed
- Dragging the window is smooth again. egui redraws on every move event, and with vsync on, each redraw held up Windows' move loop until the next refresh, so the window lagged behind the cursor. The window now presents without waiting for vsync. egui still only redraws when something changes, so the app doesn't use more GPU while idle.

## [0.3.0] - 2026-11-15

### Added
- **Models are downloaded at startup.** On first start, and whenever a model is missing from the cache folder, the app downloads LTP (32 MB) and CKIP (46 MB) in the background and loads them into memory, so no segmenter has to wait for a download. Progress shows in the status bar. A failed download is reported there and retried when the model is needed. The CLI still downloads only what a run needs.
- **Anki sentence fields are split into words.** If a ticked field value contains sentence punctuation, it's segmented with the book's segmenter and forced words, so every word and character in your example sentences counts as known. Idioms longer than 4 characters count as a whole and by their parts. With Lapis Chinese `Sentence` on the test novel, known unique words rose from 33 to 1,890.

### Changed
- When a book opens, the window grows to fit the whole Overview without scrolling. It also grows again if the Anki section or the "likely names" banner appears afterwards. Growth stops at the bottom of the screen, and a maximised window is left alone.
- The Overview's Anki section is simpler. It now shows two tiles: how many of the book's unique characters and unique words you know (young or mature), with the percentage and the number not known. The per-status columns and the running-text rows are gone. The CLI and the JSON report (`unique_chars`/`unique_words`) follow the same change. The Anki window still shows the full breakdown.

## [0.2.0] - 2026-11-15

### Added
- **Known vocabulary from Anki.** hanstat reads your local Anki collection and classifies the book's words and characters by your cards: mature (interval ≥ 21 days), young, learning, new or not in Anki. The Overview shows coverage of the book's running and unique words and characters. The Characters and Words tabs get an Anki column and a status filter. You can export the book's words that aren't young/mature, ordered by frequency.
- **Choose decks, note types and fields** in the Anki window. A deck includes its subdecks, and you can pick several fields per note type. Choices are saved to `%LOCALAPPDATA%\hanstat\anki.json`, next to the `models` folder.
- **Works while Anki is open.** The collection and its WAL are copied to a temporary folder and read from there. Nothing is written back.
- Traditional and Simplified match each other by default, so Traditional cards count against a Simplified book.
- CLI: `--anki`, `--anki-list`, `--anki-collection`, `--anki-deck`, `--anki-field "Note type:Field"`, `--anki-save`.

## [0.1.0] - 2026-11-14

This is the first release. It is a Windows desktop app (`hanstat.exe`) plus a command-line tool (`hanstat-cli.exe`) for character and word statistics on Chinese `.epub` and `.txt` files.

### Counting
- **Exact Chinese character counts.** A Chinese character is Unicode Script=Han and a letter: every CJK ideograph block (Extensions A–J), compatibility ideographs, 〇 and 々. Punctuation, Latin letters, digits and radicals are reported separately.
- **Normalisation.** Text is NFC-normalised. Variation selectors are dropped and Kangxi radicals are folded to ideographs. Zero-width and soft-hyphen characters are removed. Every change is reported.
- **EPUB reading.** Spine reading order, each document counted once. `<head>`, scripts, styles, ruby annotations, hidden elements and image alt text are excluded. TOC/nav pages are detected and excluded by default. Inline tags never split words, and HTML source line breaks follow CSS rules. DRM-protected books are detected.
- **TXT reading.** Encoding detection covers BOM, UTF-8, BOM-less UTF-16 and GBK/GB18030/Big5, with a manual override. Hard-wrapped lines are rejoined and chapters are detected from headings.
- **Per-section breakdown.** Sections can be included or excluded with instant re-totals, and each shows the new characters and words it introduces.
- **Further statistics:**
  - coverage tables (how many characters or words reach 50–100% of the text);
  - once-only counts;
  - average sentence, paragraph and word length;
  - word-length distribution;
  - Traditional/Simplified detection;
  - reading-time estimate.
- **Export.** Character and word frequency CSV, the full JSON report, and copying the unique lists to the clipboard.

### Word segmentation
- **Three segmenters:**
  - **LTP** perceptron, best on Simplified text;
  - **CKIP** BERT-tiny, best on Traditional text;
  - **jieba**.
- **Auto** (the default) picks LTP or CKIP by detected script, based on a SIGHAN 2005 benchmark (`examples/cws_eval.rs`).
- **User dictionary.** Forced words are always segmented as exactly one word, with any segmenter.
- **Suggested words.** Unsupervised discovery of book-specific names and terms that the segmenter splits. High-confidence ones are pre-ticked, applied with one click, and can be exported as a dictionary. CLI: `--suggest N`, `--auto-dict`.

### Models
- Segmentation models are **not bundled**. They are downloaded on first use from their official Hugging Face repositories, pinned to an exact commit and verified by SHA-256, then cached in `%LOCALAPPDATA%\hanstat\models`. jieba works offline.
