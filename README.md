# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese and Japanese `.epub` and `.txt` files, and word statistics for Korean ones: totals, unique and once-only characters and words, coverage, per-section breakdowns, reading time, and how much of the book your Anki cards already cover.

## Install

1. Download [`hanstat-x86_64-pc-windows-msvc.zip`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-pc-windows-msvc.zip) (Windows x64).
2. Unzip it and run `hanstat.exe` from the `hanstat` folder.
3. Open a file or drag it onto the window.

The exe isn't code-signed, so SmartScreen may ask you to confirm the first run. The app updates itself; click the version number in the status bar to check now or turn automatic updates off.

On first start the app downloads the word-segmentation models (LTP 32 MB, CKIP 46 MB) from Hugging Face into `%LOCALAPPDATA%\hanstat\models`. They aren't bundled because their licences differ (see below). The Korean analyser (Kiwi, 126 MB from its GitHub release) is downloaded the first time you open a Korean book, and the Japanese dictionary (SudachiDict full, about 137 MB from its newest GitHub release, checked at most once a day) the first time you open a Japanese book.

A book's word counts are saved in `%LOCALAPPDATA%\hanstat\cache`, so opening it again doesn't segment it again (45 s down to 0.2 s for a novel with CKIP). The saved counts are used only when the text, the segmenter and its model, the forced and known words, and hanstat's analysis code are all the same as when they were saved; anything else is analysed afresh. The file's name, location and date don't matter. The folder is kept under 1 GB by deleting the analyses unused the longest, and **Notes & method** has a button to delete them all.

## Korean

hanstat detects the book's language: a book with more Hangul than Chinese characters is Korean. The badge at the right of the toolbar shows the mode (简体中文, 繁體中文, 한국어 or 日本語). Korean books are analysed with [Kiwi](https://github.com/bab2min/Kiwi) and measured in words only. Words are counted in dictionary form: 먹었어요 counts as 먹다, 걸어서 as 걷다, and 공부했다 as 공부하다. Particles and endings aren't words. If Kiwi splits a character's name, apply it from the **Suggested words** tab.

Anki decks and fields, word lists, marked words and the user dictionary are kept separately for Chinese and Korean. Korean settings are saved to `anki-ko.json` and `known-ko.json`. Every ticked Anki field is analysed with Kiwi, so a card for 먹다 or 먹어요 matches the book's 먹다. Korean word lists should hold dictionary forms. With no book open, click the badge to choose which language's settings the Anki and word-list windows show.

## Japanese

A book whose kana are at least a tenth as many as its kanji is Japanese (badge 日本語). Japanese books are analysed with [Sudachi](https://github.com/WorksApplications/sudachi.rs) and its long-unit mode, so compounds a dictionary lists whole stay whole (事務所, 一生懸命).

- **Words** are counted by Sudachi's normalized form, which merges inflections and spellings: わかった and 分かる both count as 分かる, and かわいく as 可愛い. The Words tab shows each word as the book usually spells it. Particles and auxiliaries (は, た, ます) aren't words.
- **Kanji** are the characters counted, so there are kanji totals, coverage and known-kanji statistics.
- **Dialogue.** Before analysis, emphatic small っ (ですっ), stretched endings (ますぅ, まーす) and stutters (い、いいんですか) are removed, because Sudachi misreads them.

Settings are kept separately from Chinese and Korean, in `anki-ja.json` and `known-ja.json`. Anki fields and imported word lists are read with Sudachi, so a card for わかる matches the book's 分かった, and furigana in `会[あ]う` style is ignored.

Sudachi keeps most character names whole. If it splits one, the user dictionary (`hanstat-cli --dict`) keeps it whole.

## Word segmentation

Character counts are exact; word counts depend on the segmentation model. The default, **Auto**, uses LTP for Simplified and CKIP for Traditional text; jieba is available as a fast offline option. If names get split, apply the ones the **Suggested words** tab finds for you; they're then always segmented as single words.

Words from your imported word lists and words you marked known (not the HSK levels) also guide segmentation in Chinese books. Where the segmenter split such a word into pieces, the pieces are joined back into one word (一/口/气 → 一口气). This applies everywhere the word appears, so a known word that is also a common phrase can be joined where the phrase was meant: with 个人 known, 一个人 reads as 一/个人. This only happens when the word starts and ends on the segmenter's own boundaries. It never cuts through a word the segmenter kept whole: 研究/生命 stays as it is even if 研究生 is on your list. If two known words overlap (地/点/头 with 地点 and 点头), neither is joined. After you mark words or import a list, the book is re-segmented when you return to the **Overview** tab.

## Anki

Click **🃏 Anki**, tick your decks and the fields that hold the word (sentence fields work too), and hanstat shows which of the book's characters and words you already know, what share of the running text that covers, and a difficulty rating from it. Young and mature cards count as known. Mature means an interval of 21+ days, as in Anki, or, if you use FSRS and tick **Mature by FSRS stability**, a stability of at least the days you set on the slider, which doesn't shift with desired retention. Your collection is only read from a copy, never modified.

## Vocabulary levels and word lists

Click **📚 HSK & word lists** (Korean: **📚 Levels & word lists**; Japanese: **📚 JLPT & word lists**) to mark words as known without Anki cards. You can tick vocabulary levels, or import `.txt` word lists with one word per line (exports with pinyin or definitions after a tab or comma work too). You can also click **+** next to any unknown word in the **Words** tab to mark that word known. These words count as known alongside your Anki cards, or on their own if Anki is off. Your choices, imported lists and marked words are saved to `%LOCALAPPDATA%\hanstat\known.json` (Korean: `known-ko.json`, Japanese: `known-ja.json`) and kept between runs.

- **Chinese:** HSK 3.0 levels from the 2025 《HSK 考试大纲》 (11,000 entries, 10,896 distinct words).
- **Korean:** levels 1–6급 from 국립국어원's 국제 통용 한국어 표준 교육과정 vocabulary (10,635 entries, 9,977 words). TOPIK publishes no word list of its own; this is the list that TOPIK vocabulary lists by level follow. Most 하다/되다 verbs are listed only through their noun, with the affixes -하다 and -되다 taught at 2급. So with 2급 ticked, 말하다 and 생각되다 count as known when 말 and 생각 are in a ticked level. A few words the list teaches as part of a grammar pattern count at that pattern's level: 싶다 (-고 싶다, 1급), 때문 (-기 때문에, 2급), 듯하다 (-는 듯하다, 5급) and similar.
- **Japanese:** a **Kaishi 1.5k deck** checkbox marks every word of the [Kaishi 1.5k](https://github.com/donkuri/Kaishi) Anki deck as known (1,396 words). It's for when you studied the deck somewhere hanstat doesn't read; if the deck is in your Anki collection, tick it in the Anki window instead, so only the cards you've learned count.
- **Japanese:** JLPT N5–N1 from Jonathan Waller's lists, the ones jisho.org uses (8,293 entries, 7,452 words). The JLPT has published no word list since 2010, so these are an informed estimate. A long word that isn't on the list itself counts as known when all its parts are: 冒険者 counts once 冒険 and 者 are known. This applies to Anki and word lists too.

## Command line

```
hanstat.exe [file] [--tab overview|characters|words|sections|suggestions|notes]
hanstat-cli.exe file.epub [--ltp|--ckip|--jieba] [--dict names.txt] [--auto-dict] [--no-cache] [--json] [--chars-csv f] [--words-csv f] [--anki] [--known] [--level N] [--kaishi] [--known-list words.txt] [--lang zh|ko|ja]
```

`--level N` marks levels known in the book's language: HSK 1 to N for Chinese, 1 to N급 for Korean, and N5 down to N`N` for Japanese (`--level 3` is N5–N3). `--hsk N` still works for Chinese books.

`--no-cache` (or the environment variable `HANSTAT_NO_CACHE`, which the window obeys too) analyses the book afresh and saves nothing. `HANSTAT_CACHE_DIR` moves the saved analyses to another folder.

Run `hanstat-cli.exe --help` for all options.

## Licence

MIT or Apache-2.0, at your option. Bundled OpenCC data is Apache-2.0; the jieba dictionary is MIT. The HSK word list comes from the official syllabus via [ofou/hsk](https://github.com/ofou/hsk) (MIT). The JLPT list is Jonathan Waller's (CC BY), with JMdict ids and spelling fixes from [stephenmk/yomitan-jlpt-vocab](https://github.com/stephenmk/yomitan-jlpt-vocab), and is CC BY-SA 4.0 (see `data/JLPT-LICENSE.txt`). The Kaishi 1.5k word list holds only the deck's headwords, from [donkuri/Kaishi](https://github.com/donkuri/Kaishi), which publishes no licence. The Korean level list is 국립국어원's [국제 통용 한국어 표준 교육과정 vocabulary list](https://www.korean.go.kr/front/reportData/reportDataView.do?mn_id=45&report_seq=932), free to use under Article 24-2 of the Korean Copyright Act (public works). The downloaded models have their own licences: [LTP](https://huggingface.co/LTP/legacy) is free for research and individual use (commercial use needs a licence from HIT-SCIR), [CKIP bert-tiny](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) is GPL-3.0, [Kiwi](https://github.com/bab2min/Kiwi) is Apache-2.0, and [SudachiDict](https://github.com/WorksApplications/SudachiDict) is Apache-2.0.
