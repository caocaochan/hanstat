# Changelog

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
