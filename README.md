# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese `.epub` and `.txt` files: totals, unique and once-only characters and words, coverage, per-section breakdowns, reading time, and how much of the book your Anki cards already cover.

## Install

1. Download [`hanstat-x86_64-pc-windows-msvc.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-pc-windows-msvc.zip) (Windows x64).
2. Unzip it and run `hanstat.exe` from the `hanstat` folder.
3. Open a file or drag it onto the window.

The exe isn't code-signed, so SmartScreen may ask you to confirm the first run. The app updates itself; click the version number in the status bar to check now or turn automatic updates off.

On first start the app downloads the word-segmentation models (LTP 32 MB, CKIP 46 MB) from Hugging Face into `%LOCALAPPDATA%\hanstat\models`. They aren't bundled because their licences differ (see below).

## Word segmentation

Character counts are exact; word counts depend on the segmentation model. The default, **Auto**, uses LTP for Simplified and CKIP for Traditional text; jieba is available as a fast offline option. If names get split, add them to a user dictionary (one word per line), or apply the ones the **Suggested words** tab finds for you.

## Anki

Click **🃏 Anki**, tick your decks and the fields that hold the word (sentence fields work too), and hanstat shows which of the book's characters and words you already know, what share of the running text that covers, and a difficulty rating from it. Young and mature cards count as known. Your collection is only read from a copy, never modified.

## HSK levels and word lists

Click **📚 HSK & word lists** to mark words as known without Anki cards. You can tick HSK 3.0 levels, or import `.txt` word lists with one word per line (exports with pinyin or definitions after a tab or comma work too). These words count as known alongside your Anki cards, or on their own if Anki is off. The HSK vocabulary is the 2025 《HSK 考试大纲》 (11,000 entries, 10,896 distinct words), bundled with the app. Your choices and imported lists are saved to `%LOCALAPPDATA%\hanstat\known.json` and kept between runs.

## Command line

```
hanstat.exe [file] [--tab overview|characters|words|sections|suggestions|notes]
hanstat-cli.exe file.epub [--ltp|--ckip|--jieba] [--dict names.txt] [--auto-dict] [--json] [--chars-csv f] [--words-csv f] [--anki] [--known] [--hsk N] [--known-list words.txt]
```

Run `hanstat-cli.exe --help` for all options.

## Licence

MIT or Apache-2.0, at your option. Bundled OpenCC data is Apache-2.0; the jieba dictionary is MIT. The HSK word list comes from the official syllabus via [ofou/hsk](https://github.com/ofou/hsk) (MIT). The downloaded models have their own licences: [LTP](https://huggingface.co/LTP/legacy) is free for research and individual use (commercial use needs a licence from HIT-SCIR), [CKIP bert-tiny](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) is GPL-3.0.
