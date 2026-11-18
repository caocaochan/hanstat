# Popup dictionary for the EPUB reader: plan

Written 2026-11-18 from the feasibility tests of that day; not started. Performance comes
first: every phase below has a budget, and a phase is not done until its numbers are
measured and met.

## Decisions

### Own Rust engine, hoshidicts' design, not hoshidicts

- **Licence.** hoshidicts `main` is GPL-3.0; hanstat is MIT OR Apache-2.0. The MIT branch
  (`main-mit`) stopped on 2026-04-27. Take the design, never the code. The same goes for
  Yomitan's GPL renderer: work from the dictionary format and its JSON schemas only.
- **Not much of it is for Chinese.** Its deinflector and text preprocessor are Japanese. A
  Chinese lookup is a longest-prefix exact match, plus a Traditional→Simplified fallback.
- **Build.** It needs C++23, eight submodules, `/Zc:__cplusplus` to build with MSVC, and a
  UTF-8 manifest before its CLI accepts Chinese arguments. hanstat would have to run CMake
  for the Windows, Flatpak and macOS builds.
- **Speed.** A Rust prototype with the same design was faster on all 17 dictionaries in
  `dicts/`. Results are listed under Measured baseline.

### Storage: flat files, not SQLite

SQLite was measured: its single writer took 7.7 s of a 10 s Wikipedia import, and the
lookup median was 0.97 ms against 0.15 ms for flat files. Use memory-mapped files with
sorted hash indexes instead. `memmap2` is already a dependency.

### Dependencies

Each dependency change touches `Cargo.toml`, which re-analyses every Library once. Make
them all in one release and say so in the changelog.

- `zstd = "0.14.4"`. It is already in `Cargo.lock` through `include-flate-compress`.
- The `serde_json` feature `raw_value`, so a glossary is kept as its raw JSON text, never
  parsed into a tree.
- No hash crate. Write xxh64 by hand (about 60 lines) and test it against the reference
  vectors from the xxHash repository. A file format must not depend on a crate's hash
  algorithm, which can change between versions.
- `zstd-sys` with the `experimental` and `zstdmt` features, for multithreaded dictionary
  training. Add it only if single-threaded training of a 2 MB sample takes more than
  0.1 s per dictionary; measure that first. The multithreaded call is the only `unsafe`
  code in the plan.

### Where the code goes

- `src/dict/`, a library module so `hanstat-cli` and the tests can use it:
  - `xxh64.rs`
  - `format.rs`: file layout and version
  - `import.rs`
  - `store.rs`: one mapped dictionary
  - `lookup.rs`: all dictionaries, the scan, grouping
  - `freq.rs`
  - `sc.rs`: structured content → HTML
- `src/reader/popup.rs`: the popup.
- `src/reader/dicts_ui.rs`: the dictionary manager.
- Add `src/dict` to `NOT_ANALYSIS` in `build.rs`. The reader is already outside the analysis.

## Measured baseline (prototype, 5900X, warm cache, 2026-11-18)

### Import

Each importer ran as its own process; times are best of 3.

| Dictionary | Entries | Rust prototype | hoshidicts |
|---|---|---|---|
| CC-CEDICT | 203,458 terms | 0.15 s | 0.27 s |
| 汉语大词典 Light | 936,906 terms | 0.46 s | 0.76 s |
| 樂詞網 | 2,451,838 terms | 0.97 s | 1.38 s |
| Wikipedia | 3,952,585 terms (3.4 GB JSON) | 1.84 s, peak 0.55 GB | 2.45 s |
| Danmei (frequency) | 264,348 | 0.09 s | 0.16 s |
| Bilibili (frequency) | 1,958,154 | 0.15 s | 0.38 s |
| All 17 | | about 4.9 s | |

- **Output size:** within 5% of hoshidicts' (about 1.5 GB for all 17).
- **Prototype design:**
  - Banks are processed in parallel with rayon; each worker inflates a bank, parses it into
    borrowed rows (the glossary stays a `&RawValue`), hashes the glossary and claims the
    hash in a 256-shard table.
  - Only the first claimer compresses a glossary, with a zstd dictionary trained on a 2 MB
    sample of bank 1 (fastCover d=8, steps=4).
  - One writer thread appends to the files. Indexes are sorted with `par_sort_unstable` at
    the end.

### Lookup

All 16 prefixes across 15 term dictionaries, with every hit decompressed: median 0.15 ms,
p99 0.59 ms, max 2.4 ms. That was 2,000 positions of 梦魇直播间 with a warm cache. A cold
lookup was not measured.

Each hover decompresses about 45 KB of glossary JSON on average (91 MB over the 2,000
positions).

### Correctness

- **Term results:** of 450 word–dictionary pairs, all match hoshidicts' glossaries.
  hoshidicts groups rows that share a headword and reading into one result; counted by
  glossary, the results are the same.
- **Frequencies:** 58 of 58 word–list pairs match hoshidicts.

### Content of the 15 term dictionaries

- Every glossary item is `structured-content`.
- **Tags:** `span` 57.6 M, `div` 12.4 M, `li` 8.1 M, `ol` 6.5 M, `a` 6.1 M,
  `details`/`summary` 1.19 M, `ul` 0.22 M, `ruby`/`rt` 0.15 M, `br`, `img` 2,797, `table` 26.
- **Element attributes:** `data` 64 M (styled through `styles.css` as `data-sc-*`),
  `lang`, `title`, `href`, `open`.
- **The dictionaries' `styles.css`** use `inline-block` and `inline-flex` pills with
  background, border, `border-radius` and padding. They also use flex, `flex-wrap`,
  `row-gap`, `:is()`, `:has()`, `color-mix(in srgb, currentColor N%, transparent)`,
  `opacity` and `::before { content }`.
- scraper 0.29 turns on `:is()` and `:has()`. `css.rs` currently drops pseudo-elements.

### Frequency lists

- Danmei stores `{value, displayValue}` and Bilibili a bare number. Neither has per-reading
  ranks.
- **Coverage** on 300k characters of 梦魇直播间, split by greedy longest match:

  | List | Tokens with a rank | Multi-character types with a rank |
  |---|---|---|
  | Danmei | 98.7% | 95.4% |
  | Bilibili | 99.4% | 97.3% |

  The gaps are set phrases that HanLP splits (原来如此, 看热闹) and artefacts of the
  greedy split.
- **Lookup cost:** about 0.2 µs.

### Traditional/Simplified

- Your term dictionaries mostly list both scripts: 说话 and 說話 each hit 10 dictionaries.
  乾淨 and 裡面 miss one or two dictionaries.
- Danmei has no 應該 at all.
- Bilibili ranks 應該 9,560th, from comments written in Traditional, while 应该 itself
  ranks much higher.

## Design

### On-disk format (version 1)

Each dictionary lives in `app_dir()/dictionaries/<id>/`.

- `<id>` is a random 64-bit hex number assigned at the first import and kept for good.
  Order, on/off and other settings hang on it.
- It is not derived from the title or revision: titles can change with an update
  (CC-CEDICT's is "CC-CEDICT [2026-11-17]") and contain `/`, `[` and CJK.
- Yomitan identifies dictionaries by title; hanstat does not.

| File | Contents |
|---|---|
| `meta.json` | Format version; title, revision, author, attribution, description; `sourceLanguage`; kinds (terms and/or frequency); counts; `styles.css` text; tag bank; source zip path, size and mtime; import date; longest headword in characters; update data: `isUpdatable`, `indexUrl`, `downloadUrl`, last check time and result, remote revision |
| `zdict` | The trained zstd dictionary |
| `gloss.bin` | Compressed glossaries, each distinct one once |
| `gloss.idx` | Sorted `(hash u64, offset u64, len u32)` |
| `terms.bin` | Records: expression, reading, definition tags, rules, term tags (u16-length strings), score i32, glossary hash u64 |
| `keys.idx` | Sorted `(xxh64 of headword, term offset)`. Readings are not indexed: a reading is pinyin, and the reader never looks one up. |
| `keys.bloom` | Blocked Bloom filter of the headword hashes, about 10 bits per key, read into RAM |
| `freq.bin`, `freq.idx` | Records: expression, reading ("" = any), value i32, display |
| `media.bin`, `media.idx` | Images, by path within the zip |

**Glossary key.** Two xxh64 seeds, i.e. 128 bits, so duplicates need no byte comparison.
Lookups still check the headword's bytes, to catch hash collisions in `keys.idx`.

**Manifest.** `app_dir()/dictionaries.json` lists the order and on/off state of each
dictionary.

**Atomic import.** An import writes to `dictionaries/.tmp-<id>` and renames it when it is
complete. Startup deletes leftover `.tmp-*` folders.

**Format changes.** If a release changes the format version, the manager marks the
dictionary out of date and re-imports it.
- An updatable dictionary is re-imported from its `downloadUrl`, through the update path
  below. That also brings it to the latest revision.
- Any other dictionary is re-imported from its recorded zip, if that zip still exists.
- Zips are not copied into the app folder (another 590 MB for the current set).

### Import

Same pipeline as the prototype, with these changes:

- **Thread pool.** Use a dedicated rayon pool with one fewer thread than the CPU has, so
  the UI and reader stay smooth during an import.
- **Training.** The 2 MB sample is drawn from bank 1. Single-threaded first; see
  Dependencies.
- **Bank types.** Read `term_meta` banks (frequencies; pitch and IPA are skipped and
  counted), `tag_bank`s, `styles.css` and media. Media is copied on a separate thread.
- **Memory.** Process one bank at a time per worker; banks never exceed 27 MB. The channel
  is bounded at 32. Expected peak about 0.6 GB on Wikipedia.
- **Progress and cancel.** Progress is banks done over total, through an atomic.
  Cancelling deletes the `.tmp-<id>` folder.
- **Errors.** A malformed bank fails the import and names the file.
- **Skipped data.** Report rows that were skipped, by reason.
- **Budget:** all 17 dictionaries in 6 s or less of wall time on this machine, Wikipedia
  2.0 s or less, peak private memory 1 GB or less.

### Lookup

**The worker thread.** A `Dictionaries` worker thread owns every mapped store. The UI
sends `Lookup { generation, scan }` and receives results tagged with that generation;
results from an older generation are dropped. Owning the stores in one thread also means
a dictionary can be unmapped before it is deleted or re-imported. Windows cannot delete a
mapped file, so the manager sends `Unload(id)` and waits for the acknowledgement.

**The scan.** For each prefix length from `min(16, longest headword in any enabled
dictionary)` down to 1:
- For each script variant (the text as it is, then `fold_t2s` of it if different):
  - For each enabled store, check its Bloom filter, then `keys.idx`, then the expression
    bytes.
- Hits carry the length, dictionary order, term offset and glossary hash. Nothing is
  decompressed at this stage.

**Grouping.** Group hits by (headword, reading), longest match first. Within that, sort by
dictionary order, then by the user's chosen frequency dictionary, then by score. This is
Yomitan's grouped mode. Glossaries are decompressed only when the popup lays them out.

**Frequencies.** Look up the headword as it is, then `fold_t2s` of it. Show a rank from the
folded form as such, e.g. "Danmei 412 (简)".

**Cold-cache defence.**
- The Bloom filter and the per-dictionary longest-headword cap keep misses, which are most
  of the 16 prefixes, from touching the mapped files at all.
- When the reader opens, a background thread loads the Bloom filters and the manifests.
- If a measured cold lookup is still over budget, add an in-RAM "fence" array: every
  256th key of `keys.idx`. Each hit then costs at most one page fault.

**Cache.** An LRU of the last 64 scan results, so moving the pointer back and forth within
a word costs nothing.

**Budget:**
- Warm: median 0.3 ms or less and p99 1 ms or less for all 17 dictionaries and both
  script variants.
- Cold, first lookup after opening the reader: 20 ms or less.
- Count page faults with `GetProcessMemoryInfo` to check the cold case.

### Hit testing in the reader

- **Chapter text.** `tree.rs` adds `chars: Vec<char>` to `Chapter`. It holds the text in
  layout order, with `'\0'` at block boundaries and hard breaks. Each `Atom` in
  `layout.rs` gets `src: u32`, its index into `chars`.
- **Line records.** `line()` already has every atom's x position (`xs`) and records link
  rectangles per line. It also records `HitLine { top, bottom, xs, widths, src }` per line,
  with the unit index. `ChapterLayout::page_hits(page)` returns them.
- **Ruby.** Annotations are not hit; their base characters are.
- **Pointer to character.** Binary-search the line by y, then the character by x. The
  result is a `chars` index.
- **Scan string.** Take characters from that index up to the cap. Stop at `'\0'`, at
  whitespace, and at Unicode punctuation except `·` and `—` inside names.
- **Highlight.** Mark the matched length (which may run across a line break) with a
  translucent fill, drawn before the page's text items.
- **Budget:** hit testing plus building the scan string, 20 µs or less. That is binary
  search over at most about 40 lines per page.

### Trigger and interaction

- **Default trigger: hover with Shift held**, as in Yomitan.
  - A plain click already turns the page: the left 30% goes back, the rest forward
    (`Reader::ui`). Plain hover would open popups whenever the mouse rests on the page.
  - Settings: Shift / Ctrl / Alt / no key; a no-key mode waits about 150 ms before
    looking up.
- **While the popup is open:**
  - It stays open while the pointer is on the word or the popup.
  - Esc closes it, not the reader. A click outside closes it, as with the existing
    `popup_was_open` handling.
  - The wheel scrolls the popup and never turns the page.
  - Shift-hover over text inside the popup opens a child popup, at most 3 deep.
  - A cross-reference link inside the popup looks up its target in a child popup.

### Popup rendering

The pipeline, built once per glossary and cached by (dictionary, glossary hash, popup
width):

1. **Glossary JSON to HTML (`sc.rs`).** Strings and `{type: text}` become escaped text.
   Structured content becomes elements from a whitelist:
   - `data` keys become `data-sc-<key>` attributes.
   - `style` objects become inline CSS.
   - `lang`, `title` and `open` are kept.
   - `href` of the form `?query=…` becomes a lookup link.
   - `img` points into `media.bin`.
   - Each entry is wrapped in `<div data-dict="N">`.
2. **Parse** with `scraper`. Measure it; if parsing is over budget, build the
   `ego_tree` directly.
3. **Cascade.** Split `tree::build` into an EPUB front end and a generic
   `build_doc(doc, sheets)`, so the popup uses the reader's existing cascade (`compute`,
   `style::apply`). Each dictionary's `styles.css` is parsed into a `RuleSet` once, when
   the dictionary is loaded. A small popup user-agent sheet sets `details`, `summary`,
   `ol`/`li` numbering, `rt` and the headword styles.
4. **Layout.** Use `layout::layout` with `content_w` set to the popup's inner width and
   `page_h = f32::INFINITY`, giving one long page. Extensions it needs, also usable by
   books:
   - **`inline-block` and `inline-flex`:** an atomic box. Its inline content is laid out
     to a mini line, and the box is drawn with background, border, radius and padding.
     This is what draws the pills.
   - **`border-radius`:** on `Item::Fill` and on borders.
   - **`opacity`:** multiplies colour alpha at cascade time.
   - **`color-mix(in srgb, currentColor N%, transparent)`:** resolved at cascade time.
   - **`display: flex` / `flex-wrap`:** approximated as an inline flow.
   - **`details` / `summary`:** collapsed by default, unless `open` is set. A collapsed
     element shows its summary with ▸; a click toggles it and lays out only that entry
     again.
   - **Links:** `Link.target` becomes an enum: `Chapter(u32)`, `Toggle(node)`,
     `Lookup(String)`.
   - **`::before` with a string `content`:** last, and only if the user's dictionaries
     need it to read correctly.
5. **Popup frame.**
   - An `egui::Area` (foreground) placed below the word, or above it if there is no room.
   - Width about 26 em of the reader's text size; height at most 60% of the window.
   - Contains a `ScrollArea::show_viewport`. It uses the reader's installed font families,
     so the popup's CJK text matches the page, and the Solarized palette.
6. **Header per headword.** The headword large, then the reading(s), then frequency
   badges, then the dictionary names. Each dictionary's glossaries sit under its own
   collapsible bar, open for the first three dictionaries in the user's order.
7. **Laziness.** Lay out only the entries in the viewport plus one screen ahead.
   Decompress only those, so about 45 KB of JSON never has to be parsed all at once.
8. **Budget:**
   - Hover to popup on screen: within the next frame, 16 ms or less, for the cases
     measured above.
   - First screen of entries: 4 ms or less to build and lay out.
   - Scrolling: 2 ms or less per newly shown entry.
   - An open, idle popup must not trigger continuous repaints.

### Dictionary manager

- Opened from the reader's top bar ("Dictionaries"), as an `egui::Window`.
- **Rows:** on/off; up/down to reorder; title; kind (terms / frequency); entries; size on
  disk; revision; update status and button (see Updates); delete (confirmed, through
  `Unload` first).
- **Re-importing the same dictionary.** If the zip has the same `indexUrl`, `downloadUrl`
  or title as an installed one, offer "Replace". Replacing keeps the old id, so order and
  settings stay. Yomitan instead refuses a duplicate title.
- **Import:** a button opening an `rfd` multi-file dialog, plus zips dropped on the window.
  Each zip shows progress, an error if it fails, and a cancel button.
- **Choices:**
  - Which frequency list sorts the readings: "none" or one list.
  - Whether ranks are shown.
- Settings are stored in `reader.json`, next to the existing reader settings.

### Updates

**What dictionaries carry.** A Yomitan `index.json` can have `isUpdatable: true`, an
`indexUrl` (a small JSON index of the latest release) and a `downloadUrl` (its zip).

In `dicts/` on 2026-11-18:
- 15 of the 17 dictionaries are updatable. Danmei and Bilibili have no URLs.
- Every URL is https on GitHub: `releases/latest/download/…` or
  `raw.githubusercontent.com`.
- Remote indexes are 0.5–1.5 KB and took 0.3–0.9 s each, mostly GitHub's release redirect.
- One update was available at the time: CC-CEDICT 2026-11-16 → 2026-11-17. Its title
  changes with every release.
- Every remote `downloadUrl` equalled the stored one.

**How Yomitan does it** (checked in yomidevs/yomitan at ce1b1bf, 2026-11-12):
- **Importing.** `dictionary-importer.js` keeps `indexUrl` and `downloadUrl` only when
  `isUpdatable` is true and both are valid URLs. Otherwise the import fails.
- **Checking.** A "Check for updates" button runs `checkForUpdate()` for every dictionary
  at once (`dictionary-controller.js`). Each fetches `indexUrl`, validates it against the
  index schema, and calls `compareRevisions(current, latest)`.
- **Comparing** (`dictionary-data-util.js`). If both revisions are dot-separated integers
  with the same number of parts, they are compared part by part as numbers. Otherwise it
  is a plain string `<`. That covers this set's formats: `2026-11-16`,
  `2026-11-13T14:02:01Z`, `2026.11.16.3`, `2025.12.13.9.1`.
- **Download URL.** The remote index's `downloadUrl` wins over the stored one.
- **Applying.** It deletes the old dictionary first, then imports from the URL. A failed
  or interrupted download leaves the user with no dictionary. hanstat must not copy this.

**hanstat's design:**

- **Identity and settings.** An update keeps the dictionary's `<id>`, so its order, on/off
  state and frequency-sort choice stay. Its title, revision, description and update data
  are taken from the new `index.json`. A new title is fine.
- **Checking:**
  - The "Check for updates" button in the manager fetches every `indexUrl` in parallel, at
    most 8 at a time, 10 s timeout each, through `ureq` (already used by
    `src/update.rs`) with the app's existing `USER_AGENT`.
  - It runs on a background thread. Each row shows "Up to date", "2026-11-16 →
    2026-11-17", or "Check failed: …"; one failure never blocks the others.
  - The remote JSON is checked for a string `revision` and an https `downloadUrl` if one
    is present. Anything else counts as a failed check.
- **Automatic checks.** When the reader opens, at most once a day, behind a "Check for
  dictionary updates" setting (on by default). It is off when `HANSTAT_NO_UPDATE` is set,
  as for app updates. The result is a badge on the Dictionaries button
  ("1 update"); nothing is downloaded without the user. "Install dictionary updates
  automatically" is a second setting, off by default, because downloads can be large:
  Wikipedia's zip is 295 MB.
- **Comparing revisions.** Use Yomitan's rule, with one fix: when both revisions are
  dotted integers of different lengths, compare them as numbers with the missing parts
  read as 0. Yomitan falls back to a string compare there, which misses
  `2026.9.1` → `2026.10.1.1`. Equal or older remote revisions are no update.
- **Downloading:**
  - Stream to `dictionaries/.download-<id>.zip` with a 64 KB buffer. Never in memory: the
    app updater reads its own zip into memory, but that is a few MB, not 295.
  - Progress comes from `Content-Length`. Cancelling deletes the partial file.
  - https only; redirects allowed (GitHub releases redirect to its object storage).
  - Before downloading, check free space: zip size + 3× zip size (the store measured
    about 2.5× the zip) + 1 GB margin. If it doesn't fit, refuse with a message that says
    how much is needed. The old version stays in place until the swap, so both exist
    briefly.
- **Checking the download.** Yomitan zips carry no checksum. Instead:
  - The zip must open and have an `index.json`.
  - Its revision should equal the one in the remote index. If it doesn't, still install
    it, and show the revision actually installed.
  - Bank sizes are capped, using the sizes in the zip's directory: 256 MB per bank and
    16 GB in total, against zip bombs.
  - Media goes into `media.bin`, never into the file system, so its paths can't escape the
    folder.
- **Importing** uses the normal importer and pool, into `.tmp-<id>`. The old version keeps
  answering lookups during the download and the import.
- **Swap**, rolled back if any step fails, as `src/update.rs` does for the exe:
  1. The worker unloads `<id>`.
  2. Rename `<id>` to `.old-<id>`.
  3. Rename `.tmp-<id>` to `<id>`.
  4. The worker loads `<id>`.
  5. Delete `.old-<id>` and the downloaded zip.

  Lookups that arrive during the swap wait in the worker's queue for its few
  milliseconds.
- **Startup recovery.** Delete leftover `.download-*` and `.tmp-*` folders. If `<id>` is
  missing but `.old-<id>` exists, rename it back.
- **Update all.** At most 2 downloads at a time, imports one at a time (an import already
  uses all cores but one).
- **Dictionaries without update URLs** (Danmei, Bilibili): "Replace from file…" takes a
  newer zip and keeps the id.
- **Privacy.** Checks contact the sites the dictionary authors chose; the manager says so
  next to the button. Requests carry the generic app user agent and nothing about the
  user.
- **Budget:**
  - Checking all 15 takes 2 s or less of wall time (sequentially they would take about
    9 s).
  - Never on the UI thread.
  - Download memory stays at the buffer size.
  - The swap leaves lookups waiting 50 ms or less.
  - Importing an update meets the normal import budget.

## Phases

Each phase ends with its tests green and its budget measured and written into this file.

1. **Refactors, no behaviour change.**
   - `tree::build` → `build_doc`.
   - The `Link` target enum.
   - `Atom.src`, `Chapter.chars` and `HitLine`.
   - Check: `render_pages` gives pixel-identical PNGs for 3 test EPUBs (Simplified,
     Traditional, ruby), and the existing tests pass.
2. **Engine.**
   - `src/dict` with import, store, lookup and freq.
   - `hanstat-cli dict import|lookup|bench`.
   - Unit tests:
     - xxh64 against the reference vectors.
     - All frequency shapes.
     - `sc.rs` escaping and attributes.
     - Round trip of a synthetic zip written by the test.
     - Bloom filter has no false negatives.
     - The Windows empty-file mapping case.
   - Ignored integration test over `dicts/`: entry counts per dictionary equal the counts
     above, and the 30-word agreement list matches.
   - Benches against the import and lookup budgets.
3. **End to end with plain text.**
   - Hit testing, highlight, worker, cache, and Shift-hover.
   - A popup that shows glossaries as plain text, so latency is measured on the real path
     early.
   - `drive_ui` gets pointer-move and modifier events and writes popup PNGs.
4. **Structured-content rendering**, as above, with the layout extensions. Review PNG
   snapshots of CC-CEDICT, 现代汉语词典, 重編國語辭典, 汉语成语源流大辞典 (details) and
   Wikipedia entries.
5. **Dictionary manager**, persistence, format-version handling and deletion.
6. **Updates.**
   - Checking, comparing, downloading, the swap, and recovery.
   - Unit tests:
     - The revision rule, on a table built from the formats above, plus equal, older,
       and different-length dotted revisions.
     - Remote index validation: no `downloadUrl` means the stored one is used; a
       non-https URL is rejected; a changed title is accepted.
     - The disk-space refusal.
     - A failure injected at each step of the swap must leave the old version working.
     - Startup recovery from leftover `.old-*`, `.tmp-*` and `.download-*` folders.
   - An offline end-to-end test: a `std::net::TcpListener` in the test serves an index
     and a small zip over plain HTTP. The https-only rule is switched off for the test.
   - Ignored network test:
     - Check all 15 real `indexUrl`s against the 2 s budget.
     - Update CC-CEDICT (7.7 MB) from an older local revision.
     - Delete the outputs afterwards.
7. **Polish.**
   - Frequency badges and the Traditional→Simplified fallback.
   - Trigger settings, child popups, media images.
   - Keyboard: Esc, and arrow keys to scroll the popup.
8. **Release.**
   - CHANGELOG, What's new, README section.
   - Note the one-time Library re-analysis caused by the dependency change.

## Test safety (lesson of 2026-11-18)

A stage profiler held Wikipedia's whole 3.4 GB of JSON in memory as a JSON tree. Windows
grew the page file to 37 GB and filled the disk.

- Never hold a whole dictionary in memory; work one bank at a time.
- Run every import test under a watchdog that kills it above 4 GB of private memory or
  below 10 GB free on C:.
- Write test outputs to scratch and delete them afterwards.
- Never write next to the zips in `dicts/`. Hard links are fine; delete only the links.
- GUI tests follow the rules in the project memory: `LOCALAPPDATA` pointed at a scratch
  copy, and windows that close themselves.

## Risks

- **Large entries.** A single character can have 10 long 汉语大词典 entries. Lazy layout
  and collapsed dictionary bars are the defence; measure 不 and 行 in phase 4.
- **Cold reads.** Lookups over 1.5 GB of mapped files read from disk once the OS has
  evicted them. Measure in phase 2 after other work has filled memory; Bloom filter and
  fences are the defence.
- **Parsing cost.** scraper/html5ever on long glossaries; measure in phase 4.
- **CSS written for browsers.** The dictionaries' CSS is written for Yomitan's browser
  renderer, so the approximations above will not be pixel-faithful. Accept that, as long
  as the result reads well. Badge colours like `#111` backgrounds were chosen to work on
  both of Yomitan's themes and read fine on Solarized Light.
- **Deleting a mapped dictionary on Windows.** Only the worker thread may hold the
  mappings (see Lookup). The update swap depends on this too.
- **Unusual revisions.** A dictionary author could publish revisions that don't sort,
  such as `v2` after `v10`, which compares as older. Comparing the way Yomitan does keeps
  hanstat in step with what the authors test against; "Replace from file…" covers the
  rest.
- **Hosts.** If a host such as GitHub rate-limits or changes its redirects, the checks
  fail visibly per dictionary and nothing is lost.

## Settled with the user (2026-11-18)

These were open questions; the user chose the recommended option for each.

1. **Trigger.** Hover with Shift held, by default. Ctrl, Alt, and no key with a short
   delay are the settings' alternatives.
2. **Grouping.** By headword across dictionaries, Yomitan's default: one group per
   (headword, reading), with each dictionary's glossaries in a collapsible bar under it.
3. **Source zips.** Not kept. Their path is recorded; updatable dictionaries re-download
   after a format change.
4. **Dictionary updates.** Checked automatically at most once a day when the reader opens
   ("Check for dictionary updates", on by default), and installed only when the user
   clicks. Automatic install is a setting, off by default.

## Results (implemented 2026-11-18, release 2026.11.18.03)

Measured on the same machine; warm OS cache unless said.

| Budget | Target | Measured |
|---|---|---|
| Import all 17 (`hanstat-cli dict import`) | ≤ 6 s | 5.34–5.36 s (3 runs) |
| Wikipedia import | ≤ 2.0 s | 1.69–1.78 s |
| Import peak private memory | ≤ 1 GB | 0.55 GB |
| Lookup, 17 dictionaries, both scripts, all hits decompressed | median ≤ 0.3 ms, p99 ≤ 1 ms | median 0.10 ms, p99 0.30–0.38 ms |
| First lookup after opening | ≤ 20 ms cold | 1.5 ms warm; cold not measured (needs a reboot or admin rights to flush the file cache) |
| Popup on screen | same frame | 60/60 and 51/51 hovers (Simplified, Traditional book), child popups too |
| Whole frame with lookup and popup | ≤ 16 ms | median 2.3–2.4 ms, max 9.4–12.7 ms |
| Popup entries built and laid out | ≤ 4 ms first screen | median 1.75–1.85 ms; all dictionary bars open 3.4–5.8 ms |
| Check 15 update URLs | ≤ 2 s | 1.1–1.5 s |
| Swap during an update | ≤ 50 ms | 3–8 ms |
| Real update of CC-CEDICT (download and import) | | 1.1 s |

Per popup entry: HTML 40 µs, parse 55 µs, styling 72 µs, layout 370 µs. Book pages
render byte-identical to before (60 pages of 3 books, after phases 1 and 4).

**Deviations from the plan:**
- **Dictionary training.** It took 80–120 ms single-threaded, over the 0.1 s rule, so
  `zstd-sys` `experimental`/`zstdmt` are used, giving 45–75 ms including reading bank 1.
- **Free disk space.** The update download's check needed one more `windows-sys` feature,
  `Win32_Storage_FileSystem`. On macOS it is not checked: no `libc` dependency there.
- **Term records** carry their place in the dictionary (bank << 32 | row): banks are
  written in whatever order they finish, so ties were ordered differently from run to run.
- **Readings** group with spaces and apostrophes ignored (`huàshé tiānzú` =
  `huàshétiānzú`), so a word isn't listed twice across dictionaries.
- **Frequencies:** a Traditional headword shows the better of its own and its Simplified
  form's rank. Lists made from Simplified text rank 時 136,904th and 时 near the top.
- **Inline boxes** that contain an image stay inline (the image shows; the pill doesn't).
- **Jobs** run one at a time, downloads included; the plan allowed 2 downloads at once.
- **The fence index was not needed** (warm lookups are well under budget); still
  unmeasured cold.

**Bugs the tests found, fixed:**
- **Crash recovery inside the import** deleted the update's own download.
- **`busy()` was racy:** the window counted a finished job whose result wasn't taken yet as
  idle.
- **A reload cleared a lookup in flight:** its "loaded" reply dropped a lookup asked just
  after an import.
- **egui hides an area it hasn't seen** for one frame (sizing pass): the popup areas are
  warmed once.
- **Bottom-anchored popups jumped:** egui placed them from last frame's size, a frame off.
  Popups above their word now have a fixed height and are placed by their top.

**Tests:**
- Unit: `cargo test` (dictionary engine, revision rule, swap/rollback/recovery, offline
  update end to end over a local HTTP server, HTML conversion, tree and layout hit testing).
- Ignored, with real data:
  - `dict::tests::real_dictionaries`.
  - `dict::jobs::tests::real_update` (network).
  - `reader::preview::{dictionary_popup, popup_words, dictionaries_window}`, with
    `LOCALAPPDATA` pointed at a scratch copy.
