# Changelog

## [2026.11.17.7] - 2026-11-17

### Added
- **Leave out words that occur only once** (⚙). A book's words that occur only once (known or not) no longer count toward the unique and running words you know, the difficulty, the top unknown words, the reading path or Library Stats. Off by default, kept in `settings.json`, and used by `hanstat-cli` too. The Overview's **Unique words** tile says when they are left out, and the running-words tile's hover says how many. Word counts and the coverage table still include them; a Library entry's numbers follow when the book is next opened.

## [2026.11.17.6] - 2026-11-17

### Fixed
- **Writing style on AMD GPUs.** On an RX 7600 XT (Windows, Adrenalin 32.0.32015.7029) llama.cpp's Vulkan build crashed with an access violation (0xC0000005) on the first sample of every book, and every book in the Library was marked as not rated within minutes. It works there with cooperative matrices off (`GGML_VK_DISABLE_COOPMAT=1`) or with flash attention off. Now, when llama.cpp crashes while rating, the book is rated again with llama.cpp run more safely: on the GPU without cooperative matrices, then also without flash attention, then on the CPU only; only if it crashes on the CPU too does rating stop, with the error in ⚙. The setting that worked is kept in `style-llama.json` next to `style.json`, so later starts don't crash first, until a new llama.cpp build is downloaded. ⚙ says when a slower setting is in use.
- Writing style: a book that failed because llama.cpp stopped is no longer blamed for it. A crash closes the connection before the process is gone, so the next book used to fail too (connection refused) before llama.cpp was restarted; hanstat now waits up to 3 s for the process to exit.

### Added
- `style.log` lists llama.cpp's devices (`--list-devices`) once per run and gives llama.cpp's exit code when it stops while rating (0xC0000005 is an access violation).

## [2026.11.17.5] - 2026-11-17

### Added
- **Library search.** The Library's **Books** and **Reading path** tabs have a search box that filters the books by title and file name. It is fuzzy: a word's letters need only appear in order (`hp` finds Harry Potter, `黑森` finds 黑暗森林), and several words must all match. A word with `*` (any characters) or `?` (one character) must match the whole title or file name, as in Everything: `三体*` finds the titles that start with 三体, `*.txt` the TXT files. Case is ignored and Traditional characters count as Simplified (`三體` finds 三体). Esc or **✖** clears it. The reading path keeps each book's step number while filtered.
- **Suggested words** have **+** and **−** to mark a word known or unmark it, as in the Words tab, and a **Status** column (Anki, the ticked levels and word lists, or marked). A word marked known is also kept whole by the segmenter from the next analysis.
- **Writing style: rate again.** **Rate the N books without a rating** in the ⚙ menu starts the rater afresh and tries every Chinese book in the Library without a rating, those that failed included. It replaces turning the setting off and on.
- **Writing style: log.** The rater writes what it did to `style.log` next to `style.json`, anew each time it starts: the version and platform, whether llama.cpp and the model are downloaded, each download, llama.cpp's command line and load time, each book's sections and Chinese characters, each sample's position, length and five answers, the book's rating and time, and why a book was not rated. When llama.cpp fails to start or stops while rating, the last 80 lines of its own output are copied in (the CPU-only retry used to overwrite the GPU attempt's). **Open the log** in ⚙ opens it.

### Fixed
- Writing style: an error answer from llama.cpp (HTTP 4xx or 5xx) is reported with its message instead of only its status, and an answer that is not a digit lists the model's most likely tokens. An internal error while rating a book is reported as that book's failure instead of leaving the rater stuck on it.

## [2026.11.17.4] - 2026-11-17

### Added
- **Writing style.** A new **⚙** menu in the status bar, beside the version, has a permanent setting, **Rate writing style with a local AI model** (off by default, saved in `settings.json`). It rates how hard a Chinese book's prose is to read when you know its words: sentence structure, written and classical register, rhetoric and description, what you must infer. Two books with the same share of words known can read very differently.
  - **How.** Qwen3.5-4B (Q4_K_M, Apache-2.0) runs through llama.cpp's server (b12464, MIT), started in the background on a local port: the Vulkan build on Windows x64 and Linux x86-64 (any GPU with a Vulkan driver, else the CPU), the Metal build on Apple Silicon. It rates four samples of 3,000 characters at 30, 45, 60 and 75% of the body text, asking five questions of each (overall, syntax, register, rhetoric, narrative) against five example passages for levels 1, 3, 5, 7 and 9, with the instruction to assume every word is known; each answer counts as the expected value of the digit from the model's probabilities. Samples are not chapters, which run from 600 to 40,000 characters depending on the book, and a paragraph that is a whole chapter is cut to fit.
  - **Downloads.** llama.cpp (38 MB) and the model (2.7 GB) are downloaded on first use, pinned and checked by SHA-256, into the model folder. Nothing leaves the computer.
  - **Speed.** About 5 seconds a book on an RTX 5070; the 70 Chinese novels of a test library took 6 minutes. The Library's Chinese books are rated in the background, the open one first, and the server is stopped after a minute with nothing to rate (freeing the GPU's memory) and when hanstat closes.
  - **Shown** with two decimals and a word (plain, typical, literary, very literary) in the Overview's **Difficulty** tile, in a **Writing style** column of the Library (sortable) and of the reading path, and saved in `style.json` next to `history.json`.
  - **Difficulty** combines it with the words you know: a book rated a point above typical (5.95) counts as **Weight** points less of its running words known (2 by default, set in ⚙; a judgment, not a measurement), one below as more. The Overview, the Library and Library Stats band books by it, and the reading path raises (lowers) each book's target by as much (under a fixed number of words, its reading coverage counts that much lower). Weight 0 shows the rating without counting it.
  - **Checked** on 20 novels: the agreement between samples of one book (ICC) is 0.57 for one sample and 0.84 for the mean of four; the rating correlates −0.54 with the share of words known, and what it adds beyond that repeats between halves of a book's samples (r = 0.62). A first prompt (one question, "ignore vocabulary", only odd levels described) mostly restated the share known (0.13 beyond it). It matched a reader's judgment on the three pairs checked.

## [2026.11.17.3] - 2026-11-17

### Added
- **Linux.** Each release now has `hanstat-x86_64-linux.flatpak`, a Flatpak bundle built in the freedesktop 26.08 SDK (`flatpak/io.github.caocaochan.hanstat.yml`); install it with `flatpak install --user hanstat-x86_64-linux.flatpak`, and Flatpak fetches the runtime from Flathub. It isn't on Flathub.
  - **Fonts.** No CJK font ships with every distribution, so the Flatpak brings its own: Noto Sans CJK SC (BMP CJK, kana, Hangul) and Plangothic P1 and P2 (Extensions B–F and G–H), all SIL OFL 1.1, about 49 MB. Noto's CFF outlines draw correctly in egui, unlike Apple's (checked on Noto Sans CJK SC 2.004). A build run from Cargo has none and uses the system's fonts through fontconfig.
  - **Korean.** Kiwi's Linux library (x86-64 and aarch64, built for glibc 2.17) is downloaded with the model, as on Windows and macOS.
  - **Anki** is found in `~/.local/share/Anki2` (or `$XDG_DATA_HOME/Anki2`), then in Anki's Flatpak (`~/.var/app/net.ankiweb.Anki/data/Anki2`). The Flatpak reads both, read-only.
  - **Folders.** Settings and the Library go in `~/.local/share/hanstat`, the models and saved analyses in `~/.cache/hanstat` (in the Flatpak, under `~/.var/app/io.github.caocaochan.hanstat`). Windows and macOS are unchanged.
  - **Updates.** The Flatpak doesn't update itself (its files are read-only); the version menu says to install the newer bundle.
  - **Drag and drop** works under X11 but not under Wayland, where winit 0.30 doesn't deliver dropped files; the empty window then says to use **Open…** instead. The window has the Wayland app ID `io.github.caocaochan.hanstat`, matching its `.desktop` file.
- CI tests every push on Linux too and builds the Flatpak; releases include it.

## [2026.11.17.2] - 2026-11-17

### Changed
- The Library's **Reading path** and **Stats** tabs no longer have icons, matching **Books**.

## [2026.11.17.1] - 2026-11-17

### Added
- **Library stats.** The Library has a new **📊 Stats** tab beside the reading path. It totals every book of every language from its last analysis (books, characters and kana, running words, reading time at the Overview's speeds, the longest book, and a row per language with its average book and length-weighted share known), then measures the current language's books against what is known now: distinct words and characters across the library, words used by one book and by all, the most frequent words making up 95% and 98% of it, the shares of running words, distinct words and characters known, and the books per difficulty. Charts show book lengths coloured by difficulty, length against the share known, the library's coverage curve and how many books each word appears in, with hover details, and a table lists the 100 unknown words (not names) found in the most books, each with **+** to mark it known as in the Words tab. The vocabulary is counted in the background from the books' word data while the app runs, whatever view is open, each time the books, their word data or the known words change (about 0.5 s for 64 novels, once Anki and the word lists have loaded), and saved in `library-stats.json` (`-ko`, `-ja`), so the tab opens at once with the last count and keeps it on screen until the new one is ready. The library built is shared with the reading path. `hanstat --tab stats` opens it.

## [2026.11.17.0] - 2026-11-17

### Changed
- **History is now the Library.** The toolbar button is **📖 Library**, and the view's heading, hover texts and messages say Library. `hanstat --tab library` and `hanstat-cli --library` are the new names; `--tab history` and `--history` still work. The files keep their names (`history.json`, `covers`, `profiles`), so nothing is lost.

### Fixed
- **The reading path keeps a series in order.** It could put a later volume before an earlier one (`SCI谜案集（第二部）` before `（第一部）`) when the later one was easier. Books whose titles name one series are now read in volume order, with other books between them where that helps: the search only offers a volume once the earlier ones on the path are read, and easiest-first and orders set by hand (including ones saved before) are put in series order. Titles are matched after taking out the volume marker, tags such as `[星际]` or `（完结）`, spaces, punctuation and full-width forms, and folding Traditional to Simplified. Markers are recognised at the end, before a subtitle or at the start, bracketed or not: `第二部`, `第3卷`, `二册`, `卷三`, `上`/`中`/`下`, `上册`, `前篇`/`後編`, `续`, `4巻`, `2권`, `제3부`, `상`/`하`, `Vol. 3`, `Volume IV`, `Book Two`, `Part 5`, `#6`, `2.5`, with Arabic, Chinese (`十二`, `一百零五`, `贰`) and Roman numerals (`III`, `Ⅲ`) and ①②; nested ones too (`第一部（下）`). A book of the series with no number counts as the first; extras (番外, 外传, 外伝) come after every volume. Joined to a title without a space or bracket, only unambiguous markers count (`三体2`, `X第二部`, `X上册`), so `天下` or `1984` stay whole. ⏶⏷ no longer move a volume past an earlier or later one of its series.

## [2026.11.16.9] - 2026-11-16

### Added
- **Reading path.** History has a new **🧭 Reading path** view that orders the analysed books of the current language for reading. Before each book you study some of its unknown words, until a target share of its running words is known (95% by default) or a fixed number of them. Those words stay known for the books after it. Each row shows the share known now, when the path reaches the book and while reading it, the words to study, what reading it next would take instead, and how many of its words earlier books taught. It is recomputed in the background whenever the known words, the books or the settings change: 3–6 s for 69 novels.
  - **Study words that the rest of the library needs.** A word's value is its share of this book's running words plus **Look ahead** (default 2) times its share in every book still to come. On 69 JJWXC novels, for learners knowing HSK 1–3, 1–5 and 1–9 with targets of 95% and 98%, this cut the words studied in all by 20–26% against each book's most frequent unknown words (HSK 1–5, 95%: 34,817 instead of 46,840). The total was lowest at a weight of about 2, so that is the default.
  - **Order.** Orders are compared by their longest study list, then by the words studied in all, and found by beam search, keeping greedy or easiest-first when either does better. Order matters less than the choice of words: easiest-first with the same study words needs 36,801 words with at most 5,319 before one book, against 34,817 and 3,859. On random 7-book subsets, the search matched the best of all 5,040 orders on the longest list in 7 of 8, within 1% in the eighth, and within 0.3% on the total. Comparing every step (largest, then next largest) was tried first and rejected: one book needing about 5,400 words wherever it went set the largest step, and the search traded total words for smaller second and third steps, ending 7% above easiest-first.
  - **Your own order**: ⏶⏷ move a book, and the planner's order is shown beside yours. ✖ leaves a book out. **💾** saves a book's study list (word, tab, count in the book). **Words read N+ times are learned** (off by default) counts incidental learning.
  - **Word data.** Each book's word counts, names and (Japanese) word parts are saved in `profiles/<id>.json.gz` next to `history.json` when it is analysed (about 100 KB per novel), and deleted with its History row. Books analysed by an earlier version have none; **Analyse** reads them again in the background. Settings are in `plan.json`.
- **CLI.** `hanstat-cli file --history` records a book in the History (and its word data) as the window does. `hanstat-cli --plan [--lang zh|ko|ja] [--target P | --learn N] [--incidental M] [--lookahead X] [--objective peak|total] [--width W] [--study-lists DIR] [--json]` prints the reading path against the known words given (`--anki`, `--known`, `--level` …).
- **Open several books at once.** Select several files in **📂 Open…**, drop several onto the window, or pass several to `hanstat.exe`: all but the last are analysed one after another in the background ("Analysing 2 of 4: …" in the status bar, with **Stop**) and added to the History with their covers and word data, with what counts as known in their language when it is loaded. Then the last is opened, so the book shown is the last one analysed. Each uses the current segmenter, its language's dictionary and its default sections, as opening it alone would.
- `hanstat --tab history` and `--tab path` start in the History view.
- `examples/plan_eval.rs` measures the reading path on the books of a History.

### Changed
- Saved analyses are made again once after updating: the analysis fingerprint no longer includes `lib.rs` (its module list) or the History, so later changes there keep them.

## [2026.11.16.8] - 2026-11-16

### Added
- **History.** A new **🕘 History** button in the toolbar lists every book analysed: its cover (EPUBs), length, unique words, the share of running words known, difficulty, reading time and when it was analysed. Columns sort; a title opens the book again; ✖ removes a row. Analysing a book again replaces its row, as does changing its sections, segmenter or what counts as known while it's open. A book is identified by the SHA-256 of its file (a renamed or moved copy is the same book) or by its path (a file changed in place). Saved in `history.json` next to the other settings; cover thumbnails (at most 120 × 168 pixels) go in `covers`, and a row's cover is deleted with it.
- **Keyboard in the Words tab.** ↑/↓ select a word (Page Up/Down, Home and End jump), and the list scrolls to follow; clicking a row selects it too. **A** marks the selected word known, or unmarks a word you marked, like the +/− button. A word known from Anki or a ticked level or list can't be unmarked; **A** on it says so. With the **Not known** filter, a word marked known leaves the list and the next one is selected, so you can work down the list pressing **A**. The keys do nothing while the filter box is being typed in.

## [2026.11.16.7] - 2026-11-16

### Fixed
- **Table rows are vertically centred.** In the Characters, Words, Sections and Suggested words tables, text sat at the top of each row, so numbers and labels looked misaligned against the larger Chinese, Japanese and Korean text. Cells now centre their content, on Windows and macOS alike. Cell text no longer wraps: an over-long section name is clipped instead.

## [2026.11.16.6] - 2026-11-16

From [@linlin56](https://github.com/linlin56)'s [#1](https://github.com/caocaochan/hanstat/pull/1), with fixes.

### Fixed
- **Missing strokes in Chinese and Japanese text on macOS.** egui draws Apple's CFF-outline fonts (Hiragino, PingFang, Apple SD Gothic Neo) with strokes missing (了 as 亅, 我 as 找), and the newest PingFang not at all. hanstat now loads TrueType fonts that ship with macOS first: Heiti SC, then Heiti TC for extra Traditional coverage, AppleGothic for Hangul and Apple Symbols. Only characters none of them has still go to the system's font fallback.

### Added
- **The Mac app updates itself**, like the Windows one: the new `hanstat.app` is unpacked next to the running one and replaces it, and anything that fails puts the old one back. It needs the app out of the folder it was unzipped in (macOS runs it from a read-only copy until it's moved). **2026.11.16.5 can't update itself; download this release once by hand.**
- **The Mac release zip now holds `hanstat.app` alone**, with `hanstat-cli`, the README, changelog and licences inside the bundle (`hanstat.app/Contents/MacOS/hanstat-cli`), so an update replaces all of it. `macos/bundle.sh` builds it on any Mac.

### Changed
- Updates are off on Intel Macs and other platforms without a release of their own, rather than installing a build that can't run there.
- An update can't race the startup clean-up of the previous one: on a slow start the clean-up could delete the new version while it was being unpacked.

## [2026.11.16.5] - 2026-11-16

### Added
- **macOS build.** Releases now include `hanstat-aarch64-apple-darwin.zip` for Apple Silicon Macs (macOS 11 or later): `hanstat.app` and `hanstat-cli`, with the same features as on Windows, Korean included (Kiwi's macOS library is downloaded instead of `kiwi.dll`).
  - **First start.** The app is ad-hoc signed but not notarised, so macOS blocks it the first time; allow it under **System Settings → Privacy & Security → Open Anyway**. `hanstat-cli` needs `xattr -d com.apple.quarantine hanstat-cli` before Terminal will run it.
  - **Where things are saved.** `~/Library/Application Support/hanstat` (models, saved analyses and settings). Anki profiles are looked up in `~/Library/Application Support/Anki2`.
  - **Fonts.** Chinese, Japanese and Korean text is drawn with the system's fonts, found per character through macOS's font fallback.
  - **Not yet.** The Mac version doesn't update itself; download new releases from GitHub. Opening a book from Finder (**Open With**) isn't supported; open it from the app or drag it onto the window.
- **Both platforms are tested.** CI runs the tests on Windows and macOS, and publishes a release only when both pass. Every CI run keeps both packaged builds as downloadable artifacts for 14 days.

## [2026.11.16.4] - 2026-11-16

### Added
- **TOCFL levels.** In Chinese mode the known-words window (now **📚 HSK, TOCFL & word lists**) has checkboxes for TOCFL Novice 1, Novice 2 and Level 1–5, with an **All up to** row like HSK's. They can be ticked alongside or instead of HSK levels, and the toolbar shows both (`HSK 1–3 + TOCFL Novice 1–Level 2`). Level 6 (精通級) has no word list of its own.
  - **The list.** SC-TOP's official 華語八千詞表, edition 2024-09-23 (7,517 entries: 160/234/347/485/1,173/2,342/2,776 per level), from tocfl.edu.tw. Each entry is split into its variants (你/妳, 台灣/臺灣, 爸爸/爸), and both forms of an optional part are kept (車(子) gives 車 and 車子). Zhuyin reading notes are dropped. This gives 7,496 words, each at the first level it appears.
  - **Simplified books.** The list is in Traditional characters. It matches Simplified books when **Match Traditional ↔ Simplified** is on (the default). 着 is added for 著 read zhe/zháo/zhuó (著, 接著, 著急), because character folding keeps 著 as 著. 反复, 答复, 借口 and 复苏 are added for the same reason.
  - **Measured.** On 黄金台 (Simplified), TOCFL Novice 1–Level 5 covers 71.0% of running words, HSK 1–6 covers 68.4%, and both together cover 73.6%.
  - **CLI.** `--tocfl N` marks levels 1 to N (1–2 are Novice 1–2, 3–7 are Level 1–5). `--known` includes the TOCFL levels ticked in the window.
  - Saved in `known.json` as `tocfl`. Older versions ignore the key.

## [2026.11.16.3] - 2026-11-16

### Added
- **Saved analyses.** A book's counts are saved after it is analysed and read back the next time, so reopening a book, or switching back to a segmenter already tried, no longer segments it again. On a 830,000-character novel this takes 0.05 s instead of 45 s with CKIP and 1.3 s with LTP; on a 3.5-million-character one, 0.2 s instead of 2 s with LTP.
  - **When they are used.** Only when everything the counts depend on is unchanged: the extracted text (not the file, so a renamed or moved book still matches, and a TXT read with another encoding doesn't), the language and script, the segmenter and its model or dictionary version, the forced words, the known words given to the segmenter, and hanstat's analysis code. Releases that don't change the analysis keep the saved results; one that does starts afresh. The reports are identical to a fresh analysis.
  - **What never needs a new analysis.** Including or excluding sections, Anki, and vocabulary levels, as before.
  - **Where.** `%LOCALAPPDATA%\hanstat\cache`, one gzip-compressed JSON file per analysis (0.1–7 MB). The folder is kept under 1 GB by deleting the analyses unused the longest. **Notes & method** shows whether the open book was read from a saved analysis and how much is saved, with a **Delete all** button.
  - **Turning it off.** `hanstat-cli --no-cache`, or the environment variable `HANSTAT_NO_CACHE` for both programs. `HANSTAT_CACHE_DIR` moves the folder.
  - **CLI.** The timing line on stderr now shows the analysis time and `(from cache)`.

## [2026.11.16.2] - 2026-11-16

### Changed
- **Japanese reading time.** Reading time for a Japanese book is now measured in characters per minute (300 by default) instead of words per minute. The characters are kanji and kana; punctuation, digits and Latin letters are not counted. The rate is set separately from the Chinese and Korean ones.

### Added
- **Kana count.** For a Japanese book, the Overview's Composition table has a **Kana (total)** row, and the JSON report has `kana_total`.

## [2026.11.16.1] - 2026-11-16

### Added
- **Japanese books.** A book whose kana are at least a tenth as many as its kanji is analysed as Japanese, with the badge 日本語.
  - **Analyser.** Words come from [Sudachi](https://github.com/WorksApplications/sudachi.rs) (v0.7.0) and SudachiDict full (Apache-2.0), which adds proper nouns and set phrases to the core dictionary, so more names and nicknames stay whole. The dictionary (137 MB for 20261015) is downloaded from SudachiDict's newest GitHub release the first time a Japanese book is opened. hanstat checks for a newer release at most once a day, and uses the tested 20261015 if the newest one can't be downloaded or loaded.
  - **Word units.** Sudachi runs in its long-unit mode, so compounds that a dictionary lists whole stay whole (事務所, 一生懸命, 冒険者).
  - **Counting.** Each word is counted by Sudachi's normalized form, which merges inflections and spellings (わかった, 分かる → 分かる; かわいく → 可愛い). The Words tab, exports and CLI show the book's own usual spelling (する and こと, not the normalized 為る and 事).
  - **What isn't a word.** Particles, auxiliaries, punctuation, digits and Latin words are not counted.
  - **Kanji.** Kanji are counted as characters, so a Japanese book has a Characters tab, kanji coverage and known-kanji statistics.
  - **Why Sudachi.** It was chosen over UniDic analysers (Lindera with UniDic 2.1.2 and CWJ 2025.12, Vibrato with CWJ 3.1.1) after a comparison on five novels. It keeps character names whole, which UniDic splits (花邑 → 花 + 邑), and on other words the two were about even.
  - **Dialogue filter.** Before analysis, a filter removes noise Sudachi misreads in dialogue:
    - emphatic small っ (ですっ」);
    - stretched endings (ますぅ, まーす);
    - stutters (い、いいんですか, which Sudachi would read as 行く).
    
    Of 81 changed places judged by hand, it was better in 49 and worse in 6.
- **JLPT levels.** In Japanese mode the known-words window (**📚 JLPT & word lists**) has N5–N1 checkboxes. The levels come from Jonathan Waller's lists, as used by jisho.org, with JMdict ids and spelling fixes from stephenmk/yomitan-jlpt-vocab (CC BY-SA 4.0). The JLPT has published no word list since 2010.
  - **Keys.** The 8,293 entries are keyed by Sudachi's normalized form, giving 7,452 words: N5 639, N4 604, N3 1,579, N2 1,679, N1 2,951. 122 set phrases (気を付ける, ごめんください) are not single words and are left out.
  - **Long words.** A long word that is neither on a list nor in Anki counts as known when all its short-unit parts are known (冒険者 with 冒険 and 者). With this, JLPT N5–N1 cover 75–89% of running words in five test novels, and Kaishi 1.5k adds 2–3 points. Waller's list lacks a few basic words such as 顔.
  - **Imported lists.** Japanese word lists are read with Sudachi, so 分かる, わかる and わかった all mark 分かる.
  - **Anki.** Ticked fields are analysed with Sudachi too, with furigana readings (会[あ]う) removed.
  - **Saved settings.** Japanese settings are saved to `anki-ja.json` and `known-ja.json`.
  - **CLI.** `--lang ja` for `--anki` without a file. `--level N` marks N5 down to N`N` (`--level 3` is N5–N3).
- **Kaishi 1.5k toggle.** The Japanese known-words window has a **Kaishi 1.5k deck** checkbox that marks every word of the [Kaishi 1.5k](https://github.com/donkuri/Kaishi) Anki deck as known. This is for decks studied outside the collection hanstat reads; if the deck is in that collection, tick it in the Anki window instead, so that only the cards you have learned count.
  - **The list.** It holds the Word field of the 1,500 cards of release v2.4.3, keyed by Sudachi like the JLPT list (1,396 words). A set phrase such as お願いします marks each of its words.
  - **Saving.** The choice is saved in `known-ja.json` as `kaishi`.
  - **CLI.** `--kaishi` marks the words for one run; `--known` includes the saved choice.

### Changed
- TXT files: wrapped lines that end in kana are now joined like those ending in kanji.

## [2026.11.16.0] - 2026-11-16

### Added
- **Korean vocabulary levels.** In Korean mode, the known-words window (now **📚 Levels & word lists**) has checkboxes for levels 1–6급. The list is 국립국어원's 국제 통용 한국어 표준 교육과정 vocabulary (2017, revised 2020; 10,635 entries, 9,977 words after splitting variant pairs, merging homographs and dropping affixes). TOPIK publishes no word list of its own; this is the list that TOPIK vocabulary lists by level follow. The list has most 하다/되다 verbs only as their noun plus the 2급 affixes -하다/-되다. So once 2급 is ticked, a book's X하다 or X되다 counts as known when X is in a ticked level. Fifteen words the list teaches only inside a grammar pattern (싶다 from -고 싶다, 때문 from -기 때문에, 듯하다, 척하다, 뿐, 만큼…) are added at that pattern's level, and six (않다, 수, 적, 대로, 탓, 뻔하다) move down to it. On 404호 호러맨스 1, 1–6급 cover 91.0% of running words, with 45.1% at 1급, 64.7% at 1–2급 and 83.3% at 1–4급. CLI: `--level N` marks levels 1 to N in the book's language (`--hsk N` still works for Chinese).

### Changed
- `known.json` and `known-ko.json` store the ticked levels as `levels` instead of `hsk`. Older files are still read. Versions 2026.11.15.1 and earlier don't read the new key, so they show no levels ticked.

## [2026.11.15.1] - 2026-11-15

### Added
- **Known words guide segmentation.** In Chinese books, words from imported word lists and words marked known with **+** now join pieces the segmenter split (就/是 → 就是, 一/口/气 → 一口气), so they count as one known word. A word is joined only where it starts and ends on the segmenter's own boundaries, so it never cuts through a word the model kept whole. Where two known words overlap, neither is joined. HSK levels are not used. With HSK 1–6 as a word list, SIGHAN F1 changed by −0.2 (PKU, LTP), +0.04 (MSR, LTP) and −0.5 (AS, CKIP). Forcing the same words like a user dictionary scored 65–70. After you mark words or import a list, the book is re-segmented when you return to the **Overview** tab. The CLI applies `--known` and `--known-list` words the same way. `examples/known_eval.rs` reproduces the measurement.

### Removed
- The **User dictionary…** button. Forced words come from the **Suggested words** tab; the toolbar shows how many there are, with ✖ to clear them. The CLI's `--dict` is unchanged.

## [2026.11.15.0] - 2026-11-15

### Added
- **Mature by FSRS stability.** The Anki window has a **Mature by FSRS stability** option with a slider (1–365 days). When it's on, a review card is mature once its FSRS stability reaches that many days, instead of at Anki's interval of 21+ days. The interval depends on desired retention: at 90% it is about the stability, above 90% it is shorter and below 90% longer. Stability doesn't change with desired retention, so the same memory strength counts as mature in every preset. Cards with no FSRS data keep the interval rule, and the window says how many of them there are. The setting is saved per language with the rest of the Anki selection. CLI: `--anki-mature-stability DAYS`, where 0 means the interval rule. The JSON report's `anki` object gains `mature_stability_days` and `no_stability`.

### Changed
- **New version numbers.** Releases are now numbered by date, `yyyy.mm.dd.n`, where n counts releases on the same day from 0 (2026.11.15.0, 2026.11.15.1, 2026.11.16.0). This release follows 0.7.1. Version 0.7.1 and earlier can't read the new numbers, so their automatic update reports an unrecognised release tag. Update them once by hand: download the zip and replace the hanstat folder. Updates work automatically from this release on.

### Fixed
- **Suggested words** no longer lists common words such as 什麼, 關係, 朋友 and 廁所. A word inside a longer word that the segmenter kept whole (什麼 in 為什麼, 關係 in 沒關係, 朋友 in 男朋友) was counted as split. Such occurrences now count as kept whole, so **Split** only counts occurrences that a word boundary actually cuts through.
- Suggested words in Traditional Chinese books: the particles that rule out a candidate (麼, 著, 過, 這, 還, 沒, 來…) were only recognised in Simplified. So strings ending in one (什麼, 怎麼) were listed, and phrases containing one (捺著性子) could be pre-ticked on statistical evidence alone.

## [0.7.1] - 2026-11-15

### Fixed
- EPUB chapters with an empty `<title/>` in their head (common in Readmoo/Pubu books made with Sigil) were read as empty: the whole chapter ended up inside the title. XML-style self-closing tags such as `<title/>` and `<a id="x"/>` are now read as empty elements.

## [0.7.0] - 2026-11-15

### Added
- **Korean books.** hanstat now detects the book's language. A book with more Hangul than Chinese characters is analysed as Korean with [Kiwi](https://github.com/bab2min/Kiwi), a Korean morphological analyser (Apache-2.0). Kiwi is downloaded from its GitHub release the first time you open a Korean book: 126 MB, pinned and checked by SHA-256. Korean books are measured in words only, with no character or syllable counts. Words are counted in dictionary form: irregular stems are restored (걸어서 → 걷다), predicates get 다 (먹었어요 → 먹다), and a noun plus 하 is one word (공부했다 → 공부하다). Particles and endings aren't counted. The Overview shows word totals, unique words, word coverage, sentence and paragraph length in words, and reading time in words per minute. The Characters and Suggested words tabs are hidden for Korean books. The user dictionary works for Korean too: Kiwi keeps its words whole as proper nouns.
- **Language badge.** A coloured badge at the right of the toolbar shows the mode: 简体中文, 繁體中文, 简繁混合, 中文 or 한국어. With no book open, click it to choose which language's settings the Anki and word-list windows show.
- **Settings per language.** Anki collection, decks and fields, word lists, marked words and the user dictionary are kept separately for Chinese and Korean. Chinese settings stay in `anki.json` and `known.json`; Korean ones go to `anki-ko.json` and `known-ko.json`. For Korean, every ticked Anki field is analysed with Kiwi, so a card for 먹다 or 먹어요 matches the book's 먹다. Korean word lists are read as Hangul words, one per line.
- TXT files: EUC-KR/CP949 in the encoding menu, Korean chapter headings (제1장, 12화, 프롤로그, 외전…) and hard-wrapped Korean text, whose lines are rejoined with a space.
- CLI: Korean books work the same way. `--known`, `--known-list` and `--anki` use the book's language, and `--lang zh|ko` picks the language for `--anki` without a file.

### Changed
- The JSON report has a new `language` field. Chinese results are otherwise unchanged.

## [0.6.1] - 2026-11-15

### Added
- **Mark single words as known from the Words tab.** Each word that isn't known has a **+** button next to it. Click it to add the word to your known words. Words you marked show **−** instead, which unmarks them. Marked words are saved to `known.json` with your HSK levels and word lists, so they're kept between runs. They count like a word list: they appear with the status "Word list", feed the Overview and difficulty, and make their characters known. The **📚 Known** window lists them, newest first, with ✖ to unmark one and **Clear all**. `hanstat-cli --known` includes them too.

## [0.6.0] - 2026-11-15

### Added
- **HSK levels and word lists as known words.** A new **📚 HSK & word lists** toolbar button opens a window with two parts. You can tick HSK 3.0 levels (1–6 and 7–9, from the 2025 《HSK 考试大纲》, bundled) and import `.txt` word lists. These words count as known alongside young and mature Anki cards, or on their own when Anki is off. They feed the Overview's known-word tiles and difficulty, the export of words you don't know, and the Characters and Words tabs, which show them with the status "Word list". A young or mature Anki status still takes precedence. Choices and imported lists are saved to `%LOCALAPPDATA%\hanstat\known.json`, so they persist between runs. The CLI gets `--known`, `--hsk N` and `--known-list FILE`.

### Changed
- The Characters and Words tabs' status column is now called "Status", and the filter says "Not known"/"Known" instead of "Not young/mature"/"Young or mature".

## [0.5.0] - 2026-11-15

### Added
- **Difficulty rating.** The Overview's Anki section has two new tiles. **Running words you know** is the share of the book's word occurrences that are on a young or mature card, so frequent words weigh more, along with how often an unknown word comes up. **Difficulty for you** rates the book from that share: Easy at 98% or more, Moderate from 95%, Challenging from 90%, Hard from 80%, and Very hard below 80%. These cut-offs follow L2 reading research (Hu & Nation 2000, Laufer 1989). Hover either tile for the counts and the band table. Names don't count as unknown words: any word containing a name from your user dictionary or the recommended Suggested words is left out, as coverage studies do with proper nouns. The CLI prints the same figures with `--anki`, and the JSON report adds `running_words`, `running_chars`, `name_tokens` and `difficulty`.

### Changed
- The main window opens centred on the primary monitor.
- In the Anki window, the deck and field lists fill the width of their column, so the field list's scrollbar is at the window edge instead of next to the longest field name.

## [0.4.2] - 2026-11-15

### Changed
- The Anki window opens larger (780×680 instead of 720×560). The suspended/Traditional↔Simplified options and the Cards/Words/Characters table are pinned to the bottom of the window, so they stay visible when a note type's fields are expanded. Only the deck and field lists scroll, and they use whatever height the window has.

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
