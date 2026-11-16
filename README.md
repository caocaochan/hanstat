# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese, Japanese and Korean `.epub` and `.txt` files: totals, unique and once-only characters and words, coverage, per-section breakdowns, reading time, and how much of the book your Anki cards and vocabulary levels already cover.

![hanstat's Overview tab for a Chinese novel: character and word totals, HSK 1–5 coverage and difficulty, composition, coverage table and word-length distribution](docs/screenshot.png)

## Install

**Windows (x64)**

1. Download [`hanstat-x86_64-pc-windows-msvc.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-pc-windows-msvc.zip).
2. Unzip it and run `hanstat.exe` from the `hanstat` folder.
3. Open a file or drag it onto the window.

The exe isn't code-signed, so SmartScreen may ask you to confirm the first run.

**macOS (Apple silicon)**

1. Download [`hanstat-aarch64-apple-darwin.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-aarch64-apple-darwin.zip).
2. Unzip it and move `hanstat.app` to Applications (or any folder you can write to, so it can update itself).
3. Open it. The app isn't notarised by Apple, so the first time macOS refuses: open **System Settings → Privacy & Security** and click **Open Anyway**.
4. Open a file or drag it onto the window. The command-line tool is `hanstat.app/Contents/MacOS/hanstat-cli`.

The app updates itself; click the version number in the status bar to check now or turn automatic updates off.

The analysers are downloaded on first use into `%LOCALAPPDATA%\hanstat\models` (macOS: `~/Library/Application Support/hanstat/models`): the Chinese models (LTP 32 MB, CKIP 46 MB) on first start, Kiwi (126 MB) with your first Korean book, and SudachiDict (about 137 MB) with your first Japanese book. Analyses are saved, so reopening a book is instant.

## Languages

The language is detected from the text and shown in the badge at the right of the toolbar. Anki decks, word lists and marked words are kept separately per language.

- **Chinese:** characters and words. **Auto** segments Simplified text with LTP and Traditional with CKIP; jieba is a fast offline alternative.
- **Japanese:** kanji and words, with [Sudachi](https://github.com/WorksApplications/sudachi.rs). Words are counted in dictionary form across spellings (わかった and 分かる are both 分かる).
- **Korean:** words only, with [Kiwi](https://github.com/bab2min/Kiwi), in dictionary form (먹었어요 is 먹다). Particles and endings aren't words.

If a name gets split, apply it from the **Suggested words** tab, or pass a user dictionary with `hanstat-cli --dict`.

## Anki

Click **🃏 Anki**, tick your decks and the fields that hold the word (sentence fields work too), and hanstat shows which of the book's characters and words you already know, what share of the running text that covers, and a difficulty rating. Young and mature cards count as known; with FSRS you can judge maturity by stability instead of interval. Your collection is only read from a copy, never modified.

## Vocabulary levels and word lists

Click **📚 HSK & word lists** (Korean: **Levels**, Japanese: **JLPT**) to mark words known without Anki: tick vocabulary levels, import `.txt` word lists (one word per line), or click **+** next to a word in the **Words** tab.

- **Chinese:** HSK 3.0 levels 1–6 and 7–9 (2025 syllabus), and TOCFL Novice 1–2 and Level 1–5 (SC-TOP's 華語八千詞表, 2024 edition).
- **Korean:** 1–6급 from 국립국어원's standard curriculum, the list TOPIK vocabulary lists follow.
- **Japanese:** JLPT N5–N1 (Jonathan Waller's lists, as used by jisho.org), and the Kaishi 1.5k deck.

## Command line

```
hanstat.exe [file] [--tab overview|characters|words|sections|suggestions|notes]
hanstat-cli.exe file.epub [--ltp|--ckip|--jieba] [--dict names.txt] [--auto-dict] [--json] [--chars-csv f] [--words-csv f] [--anki] [--level N] [--tocfl N] [--known-list words.txt] [--lang zh|ko|ja]
```

Run `hanstat-cli.exe --help` for all options.

## Licence

MIT or Apache-2.0, at your option. Bundled data: OpenCC (Apache-2.0); jieba dictionary (MIT); HSK list from the official syllabus via [ofou/hsk](https://github.com/ofou/hsk) (MIT); TOCFL list from [SC-TOP's 華語八千詞表](https://tocfl.edu.tw/tocfl/index.php/exam/download) (free download, no licence stated; only the words are bundled); JLPT list by Jonathan Waller (CC BY) with fixes from [stephenmk/yomitan-jlpt-vocab](https://github.com/stephenmk/yomitan-jlpt-vocab), CC BY-SA 4.0 (see `data/JLPT-LICENSE.txt`); Kaishi 1.5k headwords from [donkuri/Kaishi](https://github.com/donkuri/Kaishi) (no licence published); Korean levels from 국립국어원's [국제 통용 한국어 표준 교육과정 vocabulary list](https://www.korean.go.kr/front/reportData/reportDataView.do?mn_id=45&report_seq=932) (public work, Korean Copyright Act Art. 24-2). Downloaded models: [LTP](https://huggingface.co/LTP/legacy) is free for research and individual use (commercial use needs a licence from HIT-SCIR), [CKIP bert-tiny](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) is GPL-3.0, [Kiwi](https://github.com/bab2min/Kiwi) and [SudachiDict](https://github.com/WorksApplications/SudachiDict) are Apache-2.0.
