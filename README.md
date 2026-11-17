# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese, Japanese and Korean `.epub` and `.txt` files: totals, unique and once-only characters and words, coverage, per-section breakdowns, reading time, and how much of the book your Anki cards and vocabulary levels already cover.

Runs on Windows, macOS and Linux. The Mac app's font handling and self-updating are by [@linlin56](https://github.com/linlin56), from [#1](https://github.com/caocaochan/hanstat/pull/1).

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

### Linux

1. Download [`hanstat-x86_64-linux.flatpak`](https://github.com/caocaochan/hanstat/releases/latest/download/hanstat-x86_64-linux.flatpak) (x86-64).
2. Install it with `flatpak install --user hanstat-x86_64-linux.flatpak`, or open it in your software centre. Flatpak fetches the freedesktop runtime from Flathub if it isn't installed yet.
3. Start **hanstat** from your applications, or run `flatpak run io.github.caocaochan.hanstat`, and open a file.

The Flatpak brings its own Chinese, Japanese and Korean fonts (Noto Sans CJK SC, and Plangothic for the rarer characters of Extensions B–H), so no CJK fonts need to be installed. It can read your home folder (read-only) and Anki's data, from Anki or Anki's Flatpak.

Under Wayland, files dropped on the window don't reach hanstat; use **📂 Open…**. To drop files, run it under X11 instead: `flatpak override --user --nosocket=wayland --socket=x11 io.github.caocaochan.hanstat`.

The command-line tool is `flatpak run --command=hanstat-cli io.github.caocaochan.hanstat`.

hanstat isn't on Flathub.

### Updates

The app updates itself; click the version number in the status bar to check now or turn automatic updates off. On macOS this needs `hanstat.app` moved out of the folder it was unzipped in, since macOS runs it from a read-only copy until then. The Mac release 2026.11.16.5 can't update itself; download a newer one once by hand. On Linux, install the newer `.flatpak` the same way as the first.

### Data

The analysers are downloaded on first use into `%LOCALAPPDATA%\hanstat\models` (macOS: `~/Library/Application Support/hanstat/models`, Linux: `~/.cache/hanstat/models`, in the Flatpak `~/.var/app/io.github.caocaochan.hanstat/cache/hanstat/models`): the Chinese models (LTP 32 MB, CKIP 46 MB) on first start, Kiwi (about 130 MB, 165 MB on Linux) with your first Korean book, and SudachiDict (about 137 MB) with your first Japanese book; llama.cpp and Qwen3.5-4B (2.7 GB) only if you turn on [writing style](#writing-style). Analyses are saved, so reopening a book is instant. On Linux, the Library and settings are in `~/.local/share/hanstat` (Flatpak: `~/.var/app/io.github.caocaochan.hanstat/data/hanstat`), and the saved analyses with the models in `~/.cache/hanstat`.

## Languages

The language is detected from the text and shown in the badge at the right of the toolbar. Anki decks, word lists and marked words are kept separately per language.

- **Chinese:** characters and words. **Auto** segments Simplified text with LTP and Traditional with CKIP; jieba is a fast offline alternative.
- **Japanese:** kanji and words, with [Sudachi](https://github.com/WorksApplications/sudachi.rs). Words are counted in dictionary form across spellings (わかった and 分かる are both 分かる).
- **Korean:** words only, with [Kiwi](https://github.com/bab2min/Kiwi), in dictionary form (먹었어요 is 먹다). Particles and endings aren't words.

If a name gets split, or glued to the word after it (蒋舸没, 蒋舸问), apply it from the **Suggested words** tab, or type it into **Force a word** there (also in the **📖 forced words** window, for every language). Forced words are saved between runs in `dictionary.txt` (`dictionary-ko.txt`, `dictionary-ja.txt`) in the app folder, one word per line, the format `hanstat-cli --dict` reads. A suggested word you already know can be marked known there with **+**, as in the Words tab.

## Anki

Click **🃏 Anki**, tick your decks and the fields that hold the word (sentence fields work too), and hanstat shows which of the book's characters and words you already know, what share of the running text that covers, and a difficulty rating. Young and mature cards count as known; with FSRS you can judge maturity by stability instead of interval. Your collection is only read from a copy, never modified.

Tick **Leave out words that occur only once** in the **⚙** menu to keep a book's once-only words (often rare words, typos or segmenter splits you'd guess or skip) out of the words you know, the difficulty, the reading path and Library Stats, known or not. The word counts and coverage table still include them, and the Library's rows are updated in the background.

## Vocabulary levels and word lists

Click **📚 HSK & word lists** (Korean: **Levels**, Japanese: **JLPT**) to mark words known without Anki: tick vocabulary levels, import `.txt` word lists (one word per line), or click **+** next to a word in the **Words** tab (or select it with ↑/↓ and press **A**; **A** again unmarks it).

- **Chinese:** HSK 3.0 levels 1–6 and 7–9 (2025 syllabus), and TOCFL Novice 1–2 and Level 1–5 (SC-TOP's 華語八千詞表, 2024 edition).
- **Korean:** 1–6급 from 국립국어원's standard curriculum, the list TOPIK vocabulary lists follow.
- **Japanese:** JLPT N5–N1 (Jonathan Waller's lists, as used by jisho.org), and the Kaishi 1.5k deck.

## Library

Click **📖 Library** to list every book you've analysed, with its cover: length, unique words, the share of running words you knew, difficulty and reading time. Click a column to sort, or a title to open the book again. The search box filters the books (and the reading path) by title and file name: letters need only appear in order (`hp` finds Harry Potter), several words must all match, and a word with `*` or `?` must match the whole title or file name (`三体*`, `*.txt`); case is ignored and `三體` finds `三体`. Analysing a book again replaces its row; a book is recognised by its file's contents, so a renamed or moved copy is still the same book. Each row keeps the sections you included, and opening the book again restores them. When your known words, the user dictionary, a setting or hanstat's analysis change, the rows they affect are analysed again in the background, 5 seconds after the last change, each with its own sections and segmenter; the status bar shows the progress, and **Stop** holds it until something changes again. Rows keep their place and date. **⟳ Recalculate library** redoes every row at once. Books of a language not used since hanstat started wait until it is, and a moved or deleted file keeps its old row. Reading times use the speeds set in **⚙** (Chinese and Japanese in characters a minute, Korean in words); a book's Overview can try another speed for that book alone. The list is kept in `%LOCALAPPDATA%\hanstat\history.json` (macOS: `~/Library/Application Support/hanstat/history.json`, Linux: `~/.local/share/hanstat/history.json`), and small copies of the covers in `covers` next to it.

## Reading path

**📖 Library → Reading path** puts the analysed books of one language in an order to read them. Before each book you study some of its unknown words, until a share of its running words is known (95% by default) or a fixed number of them. Those words stay known for every book after it, so each book makes the next ones easier. For each book the table shows how much you know now, how much when the path reaches it, the words to study, what reading it next would take instead, and how many of its words the earlier books taught. **💾** saves a book's study list (word, tab, count) to import as a word list once learned, or to make Anki cards from.

- **Which words.** A book's study words are weighed by how often the books still to come use them too (**Look ahead**, 2 by default), not only by their frequency in this book. On 69 novels from JJWXC, this cut the words studied in all by 20–26% against studying each book's most frequent unknown words, for learners knowing HSK 1–3, 1–5 or 1–9 with targets of 95% and 98%.
- **Which order.** The longest study list is made as short as it can be, then the words studied in all. Against reading the easiest book first, the order matters less than the choice of words: on the same books (HSK 1–5, 95%) it needs 34,817 words with at most 3,859 before one book, where easiest-first needs 36,801 with at most 5,319.
- **Series stay in order.** Books whose titles name one series are read in volume order, with other books between them where that helps: `SCI谜案集（第二部）`, `三体2：黑暗森林`, `X 第3卷`, `X（下）`, `X 上册`, `とある… 22`, `X 第4巻`, `X 2권`, `X Vol. 3`, `X Book Two`, `[01] X` and so on, with Arabic, Chinese and Roman numerals, ①②, and any kind of bracket. A book of the series with no number counts as the first, and extras (番外, 外传) come last. Tags such as `[星际]` or `（完结）` don't stop titles matching. Only titles are read, so a sequel with a title of its own isn't recognised: leave it out or move it by hand.
- **Your own order.** ⏶⏷ move a book; the planner's order is shown alongside, with a button to go back to it. A volume can't be moved past an earlier or later one of its series. ✖ leaves a book out (read, or not to be read).
- **Words learned from reading** can count too: tick **Words read N+ times are learned**. It's off by default, as how many encounters it takes varies widely.
- Books analysed by an earlier version have no word data yet; **Analyse** reads them again in the background.

Each book's words are kept in `profiles` next to `history.json`, about 100 KB per novel, and deleted with its Library row. The settings are in `plan.json`.

## Library stats

**📖 Library → Stats** shows the library as a whole. At the top, every book of every language: books, characters, running words and reading time in all, the longest book, and a row per language with its average book and the share of running words known at each book's last analysis. Below, the books of one language measured against what you know now:

- **Distinct words and characters** across all the books, how many words only one book uses and how many every book does, and how many of the most frequent words make up 95% and 98% of the whole library.
- **Known now**: the share of the library's running words, distinct words and characters you know, and how many books are easy, moderate, challenging, hard or very hard for you.
- **Charts**: book lengths coloured by difficulty, length against the share known, the library's vocabulary coverage curve, and how many books each word appears in. Hover a bar or point for the book or the numbers.
- **Unknown words in the most books**: the 100 words you don't know (names left out) that the most books use, the ones that make the whole library easier. **+** marks one known, as in the Words tab (− unmarks it); it leaves the list, and the figures update, a moment later.

The vocabulary is counted in the background while hanstat runs, whichever view is open, whenever the books, their word data or your known words change, and saved in `library-stats.json` next to `history.json` (`-ko`, `-ja` for the other languages). The tab opens straight away with the last count, which stays on screen (with a spinner) until the new one is ready. It is counted from the books' word data, so books analysed by an earlier version are left out until they're analysed again (from the Reading path tab).

## Writing style

Tick **Rate writing style with a local AI model** in the **⚙** menu (status bar, beside the version) to rate how hard each Chinese book's prose is to read when you know its words: sentence structure, written and classical register, rhetoric and description, and what you must infer. Two books with the same share of words known can read very differently; this is the part the word count misses.

- **How.** [Qwen3.5-4B](https://huggingface.co/Qwen/Qwen3.5-4B) runs on your computer through [llama.cpp](https://github.com/ggml-org/llama.cpp)'s server, on the GPU (Vulkan on Windows and Linux, Metal on macOS) or else the CPU. It rates four samples of 3,000 characters, at 30, 45, 60 and 75% of the book, from 1 (plain, spoken) to 9 (near classical Chinese), against five example passages, assuming every word is known. Most novels come out between 5.0 and 6.6. The first use downloads llama.cpp (38 MB) and the model (2.7 GB); books never leave your computer. A book takes about 5 seconds on a GPU (RTX 5070), much longer on a CPU. The Library's Chinese books are rated in the background, the open one first.
- **Where.** The rating shows in the **Difficulty** tile, the Library and the reading path, with two decimals.
- **Difficulty.** It counts toward each book's difficulty: a book rated a point above typical (5.95) counts as **Weight** points less of its running words known (2 by default, set in ⚙), and one below as more. The reading path raises (lowers) that book's target by as much, and Library Stats colours books the same way. The weight is a judgment, not a measurement; 0 shows the rating without counting it.
- **Checked** on 20 novels: samples of one book agree with each other (ICC 0.84 for the mean of four), the rating repeats between halves of a book even after the share of words known is taken out, and it matched a reader's judgment on the three pairs checked. Chinese books only; Windows x64, Apple Silicon Macs and Linux x86-64.

Ratings are kept in `style.json` next to `history.json`, the settings in `settings.json`. **Rate the books without a rating** in ⚙ tries again every Chinese book that has none, those that failed included. What the rater did in its last run (downloads, starting llama.cpp, each book's samples and answers, and why a book failed, with the end of llama.cpp's own output) is written to `style.log` next to them, anew each time it starts; **Open the log** in ⚙ opens it. If llama.cpp crashes while rating, the book is rated again with llama.cpp run more safely (on the GPU without cooperative matrices, then also without flash attention, then on the CPU only); the setting that worked is kept in `style-llama.json` until llama.cpp is updated, and ⚙ says which one is in use.

## Command line

```
hanstat.exe [file...] [--tab overview|characters|words|sections|suggestions|notes|library|path|stats]
hanstat-cli.exe file.epub [--ltp|--ckip|--jieba] [--dict names.txt] [--auto-dict] [--json] [--chars-csv f] [--words-csv f] [--anki] [--level N] [--tocfl N] [--known-list words.txt] [--lang zh|ko|ja] [--library]
hanstat-cli.exe --plan [--lang zh|ko|ja] [--target 95 | --learn N] [--incidental M] [--lookahead X] [--objective peak|total] [--width W] [--study-lists DIR] [--json] [--anki] [--known] [--level N] ...
```

`--library` records a book in the Library as the window does, so books can be added from a script. `--plan` prints the reading path of the Library's books against the known words given; `--study-lists DIR` writes each book's study list.

Run `hanstat-cli.exe --help` for all options. On macOS the CLI is `hanstat.app/Contents/MacOS/hanstat-cli`, and the app can be started with a file as `open -a hanstat --args "$PWD/file.epub"` (the path must be absolute). In the Linux Flatpak the app is `flatpak run io.github.caocaochan.hanstat [file...]` and the CLI `flatpak run --command=hanstat-cli io.github.caocaochan.hanstat`.

## Licence

MIT or Apache-2.0, at your option. Bundled data: OpenCC (Apache-2.0); jieba dictionary (MIT); general-language headwords from [CC-CEDICT](https://cc-cedict.org/wiki/) for word discovery, CC BY-SA 4.0 (see `data/CEDICT-LICENSE.txt`); HSK list from the official syllabus via [ofou/hsk](https://github.com/ofou/hsk) (MIT); TOCFL list from [SC-TOP's 華語八千詞表](https://tocfl.edu.tw/tocfl/index.php/exam/download) (free download, no licence stated; only the words are bundled); JLPT list by Jonathan Waller (CC BY) with fixes from [stephenmk/yomitan-jlpt-vocab](https://github.com/stephenmk/yomitan-jlpt-vocab), CC BY-SA 4.0 (see `data/JLPT-LICENSE.txt`); Kaishi 1.5k headwords from [donkuri/Kaishi](https://github.com/donkuri/Kaishi) (no licence published); Korean levels from 국립국어원's [국제 통용 한국어 표준 교육과정 vocabulary list](https://www.korean.go.kr/front/reportData/reportDataView.do?mn_id=45&report_seq=932) (public work, Korean Copyright Act Art. 24-2). Downloaded models: [LTP](https://huggingface.co/LTP/legacy) is free for research and individual use (commercial use needs a licence from HIT-SCIR), [CKIP bert-tiny](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) is GPL-3.0, [Kiwi](https://github.com/bab2min/Kiwi) and [SudachiDict](https://github.com/WorksApplications/SudachiDict) are Apache-2.0; for writing style, [Qwen3.5-4B](https://huggingface.co/Qwen/Qwen3.5-4B) (GGUF by [Unsloth](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF)) is Apache-2.0 and [llama.cpp](https://github.com/ggml-org/llama.cpp) MIT. Fonts in the Linux Flatpak: [Noto Sans CJK](https://github.com/notofonts/noto-cjk) and [Plangothic](https://github.com/Fitzgerald-Porthmouth-Koenigsegg/Plangothic-Project), both SIL OFL 1.1.
