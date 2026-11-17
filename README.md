# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese, Japanese and Korean `.epub` and `.txt` files: totals, unique and once-only characters and words, coverage, per-section breakdowns, reading time, and how much of the book your Anki cards and vocabulary levels already cover.

Runs on Windows and macOS. The Mac app's font handling and self-updating are by [@linlin56](https://github.com/linlin56), from [#1](https://github.com/caocaochan/hanstat/pull/1).

![hanstat's Overview tab for a Chinese novel: character and word totals, HSK 1–5 coverage and difficulty, composition, coverage table and word-length distribution](docs/screenshot.png)

## Install

### Windows

1. Download [`hanstat-x86_64-pc-windows-msvc.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-pc-windows-msvc.zip) (Windows x64).
2. Unzip it and run `hanstat.exe` from the `hanstat` folder.
3. Open a file or drag it onto the window (several at once are all analysed and added to the Library, and the last is shown).

The exe isn't code-signed, so SmartScreen may ask you to confirm the first run.

### macOS

1. Download [`hanstat-aarch64-apple-darwin.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-aarch64-apple-darwin.zip) (Apple Silicon, macOS 11 or later).
2. Unzip it and move `hanstat.app` to Applications (or any folder you can write to, so it can update itself).
3. Open it. The app isn't notarised, so macOS refuses the first time: go to **System Settings → Privacy & Security** and click **Open Anyway**.
4. Open a file or drag it onto the window (several at once are all analysed and added to the Library, and the last is shown).

The command-line tool is inside the app, at `hanstat.app/Contents/MacOS/hanstat-cli`. If Terminal refuses to run it, clear the download's quarantine flag once: `xattr -dr com.apple.quarantine /Applications/hanstat.app`.

### Updates

The app updates itself; click the version number in the status bar to check now or turn automatic updates off. On macOS this needs `hanstat.app` moved out of the folder it was unzipped in, since macOS runs it from a read-only copy until then. The Mac release 2026.11.16.5 can't update itself; download a newer one once by hand.

### Data

The analysers are downloaded on first use into `%LOCALAPPDATA%\hanstat\models` (macOS: `~/Library/Application Support/hanstat/models`): the Chinese models (LTP 32 MB, CKIP 46 MB) on first start, Kiwi (about 130 MB) with your first Korean book, and SudachiDict (about 137 MB) with your first Japanese book. Analyses are saved, so reopening a book is instant.

## Languages

The language is detected from the text and shown in the badge at the right of the toolbar. Anki decks, word lists and marked words are kept separately per language.

- **Chinese:** characters and words. **Auto** segments Simplified text with LTP and Traditional with CKIP; jieba is a fast offline alternative.
- **Japanese:** kanji and words, with [Sudachi](https://github.com/WorksApplications/sudachi.rs). Words are counted in dictionary form across spellings (わかった and 分かる are both 分かる).
- **Korean:** words only, with [Kiwi](https://github.com/bab2min/Kiwi), in dictionary form (먹었어요 is 먹다). Particles and endings aren't words.

If a name gets split, apply it from the **Suggested words** tab, or pass a user dictionary with `hanstat-cli --dict`.

## Anki

Click **🃏 Anki**, tick your decks and the fields that hold the word (sentence fields work too), and hanstat shows which of the book's characters and words you already know, what share of the running text that covers, and a difficulty rating. Young and mature cards count as known; with FSRS you can judge maturity by stability instead of interval. Your collection is only read from a copy, never modified.

## Vocabulary levels and word lists

Click **📚 HSK & word lists** (Korean: **Levels**, Japanese: **JLPT**) to mark words known without Anki: tick vocabulary levels, import `.txt` word lists (one word per line), or click **+** next to a word in the **Words** tab (or select it with ↑/↓ and press **A**; **A** again unmarks it).

- **Chinese:** HSK 3.0 levels 1–6 and 7–9 (2025 syllabus), and TOCFL Novice 1–2 and Level 1–5 (SC-TOP's 華語八千詞表, 2024 edition).
- **Korean:** 1–6급 from 국립국어원's standard curriculum, the list TOPIK vocabulary lists follow.
- **Japanese:** JLPT N5–N1 (Jonathan Waller's lists, as used by jisho.org), and the Kaishi 1.5k deck.

## Library

Click **📖 Library** to list every book you've analysed, with its cover: length, unique words, the share of running words you knew, difficulty and reading time. Click a column to sort, or a title to open the book again. Analysing a book again replaces its row; a book is recognised by its file's contents, so a renamed or moved copy is still the same book. The list is kept in `%LOCALAPPDATA%\hanstat\history.json` (macOS: `~/Library/Application Support/hanstat/history.json`), and small copies of the covers in `covers` next to it.

## Reading path

**📖 Library → 🧭 Reading path** puts the analysed books of one language in an order to read them. Before each book you study some of its unknown words, until a share of its running words is known (95% by default) or a fixed number of them. Those words stay known for every book after it, so each book makes the next ones easier. For each book the table shows how much you know now, how much when the path reaches it, the words to study, what reading it next would take instead, and how many of its words the earlier books taught. **💾** saves a book's study list (word, tab, count) to import as a word list once learned, or to make Anki cards from.

- **Which words.** A book's study words are weighed by how often the books still to come use them too (**Look ahead**, 2 by default), not only by their frequency in this book. On 69 novels from JJWXC, this cut the words studied in all by 20–26% against studying each book's most frequent unknown words, for learners knowing HSK 1–3, 1–5 or 1–9 with targets of 95% and 98%.
- **Which order.** The longest study list is made as short as it can be, then the words studied in all. Against reading the easiest book first, the order matters less than the choice of words: on the same books (HSK 1–5, 95%) it needs 34,817 words with at most 3,859 before one book, where easiest-first needs 36,801 with at most 5,319.
- **Series stay in order.** Books whose titles name one series are read in volume order, with other books between them where that helps: `SCI谜案集（第二部）`, `三体2：黑暗森林`, `X 第3卷`, `X（下）`, `X 上册`, `とある… 22`, `X 第4巻`, `X 2권`, `X Vol. 3`, `X Book Two`, `[01] X` and so on, with Arabic, Chinese and Roman numerals, ①②, and any kind of bracket. A book of the series with no number counts as the first, and extras (番外, 外传) come last. Tags such as `[星际]` or `（完结）` don't stop titles matching. Only titles are read, so a sequel with a title of its own isn't recognised: leave it out or move it by hand.
- **Your own order.** ⏶⏷ move a book; the planner's order is shown alongside, with a button to go back to it. A volume can't be moved past an earlier or later one of its series. ✖ leaves a book out (read, or not to be read).
- **Words learned from reading** can count too: tick **Words read N+ times are learned**. It's off by default, as how many encounters it takes varies widely.
- Books analysed by an earlier version have no word data yet; **Analyse** reads them again in the background.

Each book's words are kept in `profiles` next to `history.json`, about 100 KB per novel, and deleted with its Library row. The settings are in `plan.json`.

## Command line

```
hanstat.exe [file...] [--tab overview|characters|words|sections|suggestions|notes|library|path]
hanstat-cli.exe file.epub [--ltp|--ckip|--jieba] [--dict names.txt] [--auto-dict] [--json] [--chars-csv f] [--words-csv f] [--anki] [--level N] [--tocfl N] [--known-list words.txt] [--lang zh|ko|ja] [--library]
hanstat-cli.exe --plan [--lang zh|ko|ja] [--target 95 | --learn N] [--incidental M] [--lookahead X] [--objective peak|total] [--width W] [--study-lists DIR] [--json] [--anki] [--known] [--level N] ...
```

`--library` records a book in the Library as the window does, so books can be added from a script. `--plan` prints the reading path of the Library's books against the known words given; `--study-lists DIR` writes each book's study list.

Run `hanstat-cli.exe --help` for all options. On macOS the CLI is `hanstat.app/Contents/MacOS/hanstat-cli`, and the app can be started with a file as `open -a hanstat --args "$PWD/file.epub"` (the path must be absolute).

## Licence

MIT or Apache-2.0, at your option. Bundled data: OpenCC (Apache-2.0); jieba dictionary (MIT); HSK list from the official syllabus via [ofou/hsk](https://github.com/ofou/hsk) (MIT); TOCFL list from [SC-TOP's 華語八千詞表](https://tocfl.edu.tw/tocfl/index.php/exam/download) (free download, no licence stated; only the words are bundled); JLPT list by Jonathan Waller (CC BY) with fixes from [stephenmk/yomitan-jlpt-vocab](https://github.com/stephenmk/yomitan-jlpt-vocab), CC BY-SA 4.0 (see `data/JLPT-LICENSE.txt`); Kaishi 1.5k headwords from [donkuri/Kaishi](https://github.com/donkuri/Kaishi) (no licence published); Korean levels from 국립국어원's [국제 통용 한국어 표준 교육과정 vocabulary list](https://www.korean.go.kr/front/reportData/reportDataView.do?mn_id=45&report_seq=932) (public work, Korean Copyright Act Art. 24-2). Downloaded models: [LTP](https://huggingface.co/LTP/legacy) is free for research and individual use (commercial use needs a licence from HIT-SCIR), [CKIP bert-tiny](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) is GPL-3.0, [Kiwi](https://github.com/bab2min/Kiwi) and [SudachiDict](https://github.com/WorksApplications/SudachiDict) are Apache-2.0.
