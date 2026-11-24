# Library columns: plan

Written 2026-11-24 and implemented the same day (uncommitted at first): `src/library_columns.rs`
(layout, values, filters, paging, sort keys), `src/library_books.rs` (the table, the Columns
window, filter popups, cells), `src/chapter_lengths.rs`, `src/book_series.rs`, and
`series::{from_opf, read_epub, of, named, group}`.

**Where the implementation differs from the plan below:**

- **Pager placement:** "☷ Columns…" is in the Library header row. The pager, "Sorted by …" and
  the filter chips are in a row of their own under it, shown only when there are filters or
  more than one page.
- **Filter button:** a funnel drawn by hand (not a `⏷` glyph, which the sort arrow already
  uses). It is outlined, and filled in the accent colour while a filter is set. Its
  accessible name is "Filter <column>".
- **Dependencies:** `jiff` is a direct dependency as well (`std` only), for the day type.
  egui_extras needs its `serde` feature next to `datepicker`, because eframe's
  `persistence` makes egui require serialisable widget state.
- **`src/extract`:** it is in `NOT_ANALYSIS`, but also in `LIBRARY_SOURCES`, so touching it
  still re-analyses every Library row.
- **Columns window:** Writing style shows "default · off in ⚙" while rating is off.
- **Series column:** only books that share a series with another book show one, as the
  reading path counts them.
- **Checked in the GUI** (scratch Library of 6 books, including the three 酒与枪 EPUBs with
  `calibre:series`):
  - pinned columns, 3 and 11 pages;
  - progress (2%, 5%, 100% for a book marked read) and the metadata series 酒与枪 · 1/2/3;
  - all five own types;
  - hide, ▲ to move, delete with Undo, a new column;
  - a Known filter with its chip, remembered after a restart;
  - search by Author.
- **Not checked in the GUI:**
  - drag-and-drop reordering (only ▲▼);
  - the date picker's calendar;
  - typing into a cell after a click;
  - the Reading path tab's series grouping (unit-tested only).


Library → Books today is one hard-coded `TableBuilder` in `App::history_view` (`src/main.rs`,
about lines 6014–6328). Three parallel lists must stay in step there: the `others` width
array, the `heads` vector (with Writing style spliced in at index 8) and the `row.col` calls
(Known and Difficulty are drawn by one `match`). This plan replaces them with one list of
columns that the user can reorder, hide, add to, sort and filter. Columns that don't fit
the window go to further pages.

## The user's choices

- **Column kinds:** both kinds.
  - **Stat columns:** picked from data hanstat already has for each book.
  - **Your own columns:** a name the user chooses, holding values they type for each book.
- **Default columns:** they can't be deleted, but each one can be **hidden** and shown
  again.
- **Order:** the order of every non-pinned column can be changed.
- **Paging:** the columns that don't fit go to page 2, 3, …. **Book** (cover and title)
  stays at the left of every page, and the **✔ Read** mark and **✖ remove** stay at the
  right.
  - The **📕 Read** (open in reader) button is pinned beside Book too.
- **Sorting and filtering:** every data column can be sorted and filtered. That includes
  stat columns, your own columns and the pinned Book and ✔ Read. 📕 Read and ✖ are only
  buttons, so they have no sort or filter.
- **Answers to the follow-up questions (2026-11-24):**
  - **Types for your own columns:** Text, Number, Yes/No, Rating ★ (1–5) and Date.
  - **Date entry:** egui_extras' `DatePickerButton`. This adds the `jiff` crate, so every
    Library is re-analysed once after the update (see *Pitfalls*).
  - **Filters:** remembered between runs.
  - **"Hide read":** the checkbox is **removed**. Its job moves to the ✔ Read column's
    filter.
  - **Progress:** a book marked ✔ Read shows **100%**, even when it was never read in the
    reader. The cell shows the percentage with a thin bar under it.
  - **Series metadata:** read **exactly as calibre does**. Collections of type `set`, or
    with no type, are not series.
  - **Deleting your own column:** done at once, with **Undo** for a few seconds, like
    removing a book.
  - **Search:** the search box also matches your own **Text** columns.
  - **Sorted column hidden or removed:** the sort falls back to Analysed, newest first.
  - **Reset to default:** your own columns are kept, hidden, at the end.

## Columns

One enum names every column, sortable or not. It extends the existing `HistCol`, so a
saved `history_sort` still reads (see *Pitfalls*).

| Column | Kind | Default | Sort key | Filter |
|---|---|---|---|---|
| Book | pinned left | shown | title (lowercased) | the existing 🔍 search box |
| 📕 Read | pinned left | shown | — | — |
| Language | default | shown | variant | choice |
| Characters | default | shown | `han_total` | range |
| Words | default | shown | `words_total` | range |
| Unique words | default | shown | `words_unique` | range |
| Known | default | shown | `known.running_words` | range (%) |
| Difficulty | default | shown | as now | choice (levels) |
| Writing style | default | shown while style is on | score | range (%) |
| Reading | default | shown | minutes | range (h) |
| Analysed | default | shown | time | age |
| ✔ Read | pinned right | shown | read time | choice (read / not read) + age |
| ✖ | pinned right | shown | — | — |
| Unique characters | stat | — | `han_unique` | range |
| Words for 98% | stat | — | `words_for_98` | range |
| Known unique words | stat | — | `known.unique_words` | range (%) |
| Unknown words | stat | — | `known.unknown_words` | range |
| Known unique characters | stat | — | `known.unique_chars` | range (%) |
| Segmenter | stat | — | label | choice |
| Format | stat | — | EPUB / TXT | choice |
| Sections | stat | — | included (of total) | range |
| Time read | stat | — | `reading_stats` totals `read_secs` | range (h) |
| Reading speed | stat | — | `BookStats::speed` | range (cpm) |
| First read / Last read / Finished | stat | — | times | age |
| Series | stat | — | series name, then volume (metadata first, else title: see *Series from book metadata*) | text |
| Progress | stat | — | share read, 0–100% | range (%) |
| *your own* | user | — | text or number | text / range |

- **Hidden Writing style:** it keeps its place in the order but is not drawn while the
  setting is off, as today.
- **Reading data:** the reading-stats columns read `reading_stats::with` once per frame
  into a `HashMap<id, …>`. This is the same pattern `read_dates()` uses now.
- **Progress** is worked out from saved chapter lengths: see *Reading progress* below.
- **Your own columns** have one of five types. An empty value sorts like the existing "–"
  (below every value when sorted descending), and every filter has a "No value" choice.

  | Type | Cell | Sort | Filter |
  |---|---|---|---|
  | Text | click to type; clipped with "…", the whole value on hover | A–Z, ignoring case | contains (search matcher) |
  | Number | click to type; must parse | numeric | range |
  | Yes/No | a checkbox, toggled with one click (no edit mode) | yes before no | choice (yes / no) |
  | Rating ★ | five stars; click one to set, click the same star again to clear | by stars | at least N stars |
  | Date | `DatePickerButton` showing `2026-11-24`, with a ✖ to clear | by date | age (last N days / more than N days ago) and from–to |

  - **Stored values:** `Value::{Text(String), Number(f64), Yes(bool), Stars(u8),
    Date(i32 days since 1970-01-01)}`. Dates are saved as a day number, not a
    `jiff::civil::Date`, so the file doesn't depend on jiff's serde format. They are
    converted at the picker.
  - **Changing a column's type:** not offered. Make a new column instead. Values would not
    convert cleanly.

## Reading progress

How far through a book you are is: the characters in the chapters before the place where
you left it, plus the offset into its chapter, divided by all the book's characters. Home
already does this (`home::through`, `src/home.rs:264`). The place is cheap to get
(`reading_stats::left_at`). What costs time is each chapter's length:
`reader::chapter_text_len` builds every chapter's tree. Home works the lengths out on a
background thread and keeps them in memory only, so they are worked out again each run,
and only for the books Home shows.

**Save the chapter lengths.** They depend only on the file and on how the reader counts
text, so they can be saved once per book.

- **File:** `chapter-lengths.json` in `models::app_dir()`, through `saver::load_json` /
  `saver::write`. Its shape is `{ "reader": "<fingerprint>", "books": { "<id>":
  [len, …] } }`.
  - It is a few hundred bytes per book.
  - It is a cache: if it is lost or unreadable, the lengths are just worked out again.
  - It is not put in `reading-stats.sqlite`. That database holds data that can't be
    remade, and a new table there would need a schema migration.
- **Kept valid:**
  - **The file:** books are keyed by their id (the file's SHA-256), so a changed file is a
    new id.
  - **How the reader counts:** build.rs gets `HANSTAT_READER_TEXT_FINGERPRINT`, a hash of
    the sources `tree::build` counts text with (`src/reader/tree.rs`, `markup.rs`,
    `css.rs`, `epub.rs`, `style.rs`; check the exact set when implementing). When the
    fingerprint differs, the whole file is dropped and the lengths are worked out again.
    Places are counted with the same code, so the two always agree.
- **Who fills it:** one shared module, `src/chapter_lengths.rs`. It loads the file at
  start and works out the missing books on one background thread, one book at a time, as
  Home does now.
  - **Order:** books on screen first, then every other EPUB book with a place.
    Progress-sort needs all of them, but these are only the books opened in the reader, so
    usually a few dozen at most.
  - It saves after each batch, not after each book.
  - **Home** asks it for lengths instead of working them out in `home::find`, which then
    only does the table of contents and the large cover.
  - The reading-path thread (`src/main.rs:6680`) can take lengths from it too. That is
    optional, since it only needs one chapter.
- **Lifecycle:**
  - **DuChinese rebuild:** the new file gets a new id and its own lengths, and the old
    id's lengths are dropped.
  - **Remove:** a removed book's lengths are dropped in `App::finish_removal`, beside
    `home_view.forget`.
- **The cell:**
  - Finished in the reader (`BookStats::finished`) or marked ✔ Read: **100%**, with no
    lengths needed. The hover says which ("Marked read 3 days ago" / "Finished in the
    reader").
  - A place but no lengths yet: weak "…".
  - Otherwise `42%`, with a thin progress bar under it (Home's `progress_bar`, moved
    somewhere both can use). Hover: "Chapter 12 of 40 · left 3 days ago".
  - Never opened in the reader, or a TXT book (unless marked read): "–". It sorts below
    every percentage, like the other empty values.
- **Order of work:** this part needs no other part of the plan. It can land first, and
  Home moving onto the cache is a small win on its own: no more working out lengths on
  every start.

## Series from book metadata (separate from the columns)

Today a book's series comes only from its title: `series::parse` in `src/series.rs`,
used by the reading path (`plan::Library::build`, `src/plan.rs:734`) to keep a series
together and in order. Many EPUBs say their series outright in their metadata, the way
calibre writes it, and that should come first.

**What calibre writes and reads** (checked in calibre's source:
`src/calibre/ebooks/metadata/opf3.py`, `read_series` / `create_series`; `opf2.py`, the
`series` / `series_index` fields):

- **EPUB 3:** `<meta property="belongs-to-collection" id="c01">三体</meta>`, refined by
  `<meta refines="#c01" property="collection-type">series</meta>` and
  `<meta refines="#c01" property="group-position">2</meta>`.
  - When writing, calibre removes the older `calibre:series` metas and writes only this
    form.
- **EPUB 2:** `<meta name="calibre:series" content="三体"/>` and
  `<meta name="calibre:series_index" content="2"/>`.
- **Precedence when reading:** calibre takes the first `belongs-to-collection` with
  non-empty text whose `collection-type` refinement is `series`. Only if there is none
  does it read `calibre:series` / `calibre:series_index`. A collection of another type
  (`set`, or no type) is not a series to calibre.
- **The index:** parsed as a float (`2.5` is valid). If it is missing or not a number,
  the index is 1, and the series still counts.
- We read the same way, so a book means what calibre shows for it.
  - Not verified: whether calibre writes `2` or `2.0`. It doesn't matter, since we parse a
    float either way.

**Design:**

- **Not read during analysis.** `src/extract` is part of the analysis and Library
  fingerprints, so touching it would re-analyse every user's Library. CKIP takes minutes
  per Traditional book.
  - Instead, a small OPF reader lives in `series.rs`, which is already in `NOT_ANALYSIS`:
    `series::from_opf(&roxmltree::Document) -> Option<(String, f64)>`, a pure function.
  - A loader opens the EPUB and finds the OPF through `META-INF/container.xml`. It should
    reuse what `reader::epub` already does for that, if it exposes it.
- **Saved:** `book-series.json` in the app folder, through the saver: `{ "version": 1,
  "books": { "<id>": { "name": "三体", "index": 2.0 } | null } }`.
  - `null` (no series in the metadata) is saved too, so a book is read only once.
  - Bumping `version` re-reads every book. Bump it whenever the reading rule changes.
  - It is a cache: if lost, it is read again. Reading one OPF takes a few milliseconds, so
    the whole Library fills on a background thread at start, plus each new book once it
    is analysed.
  - It is kept in step like chapter lengths: forgotten on remove, and a DuChinese rebuild
    is just a new id.
  - TXT books have no metadata and keep the title rule.
- **One function decides a book's series:** `series::of(title, meta) -> Volume`.
  - With metadata:
    - The key is the metadata name, normalised as titles are (`key_of`: Simplified
      folding, punctuation and spaces dropped, lowercased). A book named `三體` in its
      metadata and a book whose title gives `三体` are then one series.
    - The volume is `index × 10` in the existing tenths (`2.5` → `[25]`, `0` for a prequel
      → `[0]`, negative clamped to 0).
    - Any volume marker in the title is ignored.
  - Without metadata: `series::parse(title)`, as today.
  - A book with metadata always counts as numbered. So two books sharing a metadata series
    form a group even when neither title has a number (the `any(number.is_some())` rule in
    `plan.rs`).
- **Reading path:**
  - **Where series are set:** `Library::build` stops parsing titles itself. Series are set
    by a new `Library::set_series`, used the way `set_offsets` is (`Arc::make_mut`), so
    metadata arriving doesn't rebuild the word data.
  - **Replanning:** the plan inputs carry a hash of the series metadata, so a finished
    metadata read replans once, not on every frame.
  - **"Keep series together":** the option works as before; only its groups get better.
- **Library:**
  - The Series column shows `三体 · 2`. Its hover says where it came from: "from the
    book's metadata (calibre)" or "from the title".
  - Volumes show as `2`, `2.5`, `上/下` as parsed, and "extra" for 番外.
- **Not changed:** the DuChinese EPUB builder. Writing series metadata into those files
  would change their bytes, which re-keys every imported book. Their titles already parse.

## Data and where it lives

A new file, `library-columns.json` in `models::app_dir()`, loaded with `saver::load_json`
and written with `saver::write` (fsync + rename; an unreadable file is set aside, never
overwritten). It holds:

```rust
struct ColumnsFile {
    /// Every non-pinned column in display order, with whether it's shown.
    /// Stat and user columns appear here only while added.
    order: Vec<(HistCol, bool)>,
    /// User column definitions; ids are never reused.
    fields: Vec<Field>,            // { id: u32, name: String, kind: Text | Number }
    next_id: u32,
    /// Values by book id, then field id. Numbers stored as f64.
    values: BTreeMap<String, BTreeMap<u32, Value>>,
    /// Active filters, kept between runs like "Hide read" is.
    filters: BTreeMap<HistCol, Filter>,
}
```

- **Not in `history::Entry`:** `src/history/row.rs` is in build.rs `LIBRARY_SOURCES`, so
  changing what a row holds re-analyses every Library. Rows are also rewritten on every
  re-analysis.
- **Not in `UiState` (eframe storage) either:** the layout refers to user field ids, so it
  belongs with them. `UiState` keeps only `history_sort`.
- **Normalising on load:**
  - Unknown columns (from a newer build, or a deleted field) are dropped.
  - Defaults that are missing are added at their default position, shown.
  - Duplicates are removed.
  - Filters on columns that no longer exist are dropped.
- **Book lifecycle:**
  - **Rekey:** where `History::rekey(old, new)` is called (`src/duchinese/ui.rs:525`),
    values move from `old` to `new`.
  - **Remove:** values stay while removing can still be undone. They are dropped in
    `App::finish_removal`, next to `self.covers.remove(id)`.
  - Nothing is pruned on load: an unreadable history.json must not wipe values.
- **Saving:** on commit of an edit (Enter or focus lost), on a layout or filter change, and
  on exit. Not every frame.

## UI

### "Columns…" button

The button sits in the Library header row, Books tab only, after "Hide read":
`☷ Columns…`. It opens an `egui::Window` (like the read dialog) holding:

- Greyed, fixed lines at the top and bottom: *Book, 📕 Read — always first* and *✔ Read,
  ✖ — always last*.
- One row per column, in order. Each row has:
  - a drag handle `☰` (`ui.dnd_drag_source` / `dnd_drop_zone`, both in egui 0.37 —
    verified in `egui-0.37.0/src/ui.rs:2767,2826`);
  - **▲ ▼** buttons as well. Dragging inside a scroll area is fiddly, and the buttons can
    be pressed through UI Automation in tests;
  - a checkbox: shown / hidden;
  - the name, with a weak tag: *default*, *stat* or *text*/*number*.
  - Stat columns have **✖ Remove** (they go back into the Add menu).
  - Your own columns have **✎ Rename** and **🗑 Delete**.
    - **Delete** takes effect at once and shows "Deleted 'Author' (values in 37 books) ·
      Undo" for the same few seconds as a removed book (`Removed`/`REMOVE_SECS`-style
      state).
    - While Undo is offered, the column and its values are kept aside in memory, not
      saved. The deletion is written when the time runs out, when another column is
      deleted, or when the app exits.
  - Defaults have neither, only the checkbox.
- **＋ Add column ▾** opens a menu:
  - the stat columns not yet added;
  - a separator;
  - **Your own…**, which asks for a name (required, unique among columns) and a type
    (Text / Number / Yes/No / Rating ★ / Date).
  - New columns go at the end, shown.
- **Reset to default** restores the default order and shows every default column. Stat
  columns are removed. Your own columns are kept, moved to the end, hidden, so no values
  are lost.

### Paging

`pack_pages` is a pure function, unit-tested. It takes the widths of the shown non-pinned
columns in order, and the room left beside the pinned columns. It packs columns into pages
greedily, keeping their order.

- **Book's width:**
  1. Book's floor = `clamp(longest title + cover, 140, 280)`.
  2. Room for the paged columns = available width − pinned widths − Book's floor − gaps −
     scroll bar.
  3. After packing, Book gets `min(longest title + cover, available − pinned − widest
     page)`. That width is the **same on every page**, so the title column doesn't jump
     when paging. `title_column_width` changes accordingly.
- **Too-wide column:** a column wider than the room on its own (very narrow window) gets a
  page to itself and is clipped.
- **Pager:** `◀ 1 / 3 ▶`, in the header row to the right of the Columns button. It is
  hidden when there is one page.
  - The page is kept as *the first column on it*, not as a number. After a resize the
    pager opens the page that now holds that column, so the columns you were looking at
    stay on screen.
  - The page is not saved between runs.
- **Sorted column on another page:** a weak "Sorted by Known ▼" appears beside the pager,
  so the order still makes sense.

### Sorting

Clicking a header still sorts, and clicking again reverses (`sort_header`, unchanged).

- **Direction of the first click:** numbers and dates start descending, text ascending, as
  today.
- **Sort key:** the key code becomes one `fn sort_key(col, entry) -> Key`, where
  `enum Key { Num(f64), Text(String) }` is compared with `total_cmp` / `cmp`. It is applied
  with `sort_by_cached_key`, so text is lowercased once per row, as Title already is.
- **Hidden or removed sort column:** if the column sorted by is hidden or removed, the sort
  falls back to Analysed, newest first.

### Filtering

- **Opening a filter:** each sortable header gets a small `⏷` button after its label. It
  is weak when no filter is set, and in the accent colour (`palette::accent()`) when one
  is. It opens a popup with that column's filter:
  - **range:** min and max (`DragValue`, either can be left empty), in the column's unit:
    %, hours, counts.
  - **choice:** a tick list of the values present in the Library (Language: Simplified /
    Traditional / Mixed; Difficulty levels; Format; Segmenter; ✔ Read: read / not read).
  - **age** (dates): "in the last N days" or "more than N days ago".
  - **text:** "contains", using the same matcher as the search box (`search::Query`: words
    in order, `*`/`?`, 三體 finds 三体).
  - every filter: a **"Books with no value"** tick for rows showing "–", on by default for
    choice filters and off once a range bound is set. Also a **Clear** button.
- **How filters combine:** all active filters must match (AND), together with the search
  box.
- **"Hide read" is removed.** The ✔ Read column's filter ("Not read") does its job.
  - **Migration:** a saved `library_hide_read: true` in `UiState` becomes that filter on
    the first start of the new build. The field is then no longer written. serde ignores
    unknown fields, so older and newer `UiState`s still read.
  - The "Every book is marked read." empty message goes. The general empty message covers
    it.
- **Chips:** under the header row, one chip per active filter, e.g. `Known ≥ 90% ✖`,
  `Language: Traditional ✖`, followed by **Clear filters**.
  - Chips show filters on hidden columns or on other pages too, so a filter is never
    invisible.
  - Clicking a chip opens its popup.
- **Empty result:** "No book matches the search and filters", with a **Clear filters**
  button.
- **Headers:** the extra `⏷` needs ~14 pt. Columns now 80–90 pt wide (Reading, Language)
  grow to ~100 pt so "Characters ⏷" etc. aren't clipped. Check this in a screenshot.

### Search

The search box also matches the text of your own **Text** columns, as well as the title
and file name, with the same matcher.

- **Combining:** a book matches when every search word is found in at least one of those
  fields. So `三体 刘慈欣` finds a book whose title holds 三体 and whose Author column holds
  刘慈欣.
- **Wildcards:** a word with `*`/`?` must match a whole field.
- `book_matches` takes the book's Text values, and `LIBRARY_SEARCH_HOVER` says so.
- Numbers, dates, ratings and yes/no are left to the column filters.

### Editing your own columns

- **Editing a cell:** click the cell, which is a weak "–" placeholder when empty. It turns
  into a `TextEdit` the column's width.
  - Enter or focus lost saves; Esc cancels.
  - A Number cell that doesn't parse shows the warn colour and is not saved.
- **Display:** long text is clipped with "…", and the whole value is shown on hover.
- **Only one cell edits at a time** (`App::library_edit: Option<(id, field)>`). While it
  is open, typed keys must not reach the Library's shortcuts. Check
  `shortcuts::Action::Filter`'s handling, as the search box does.

## Refactor first (no visible change)

1. Add `struct LibColumn` / `fn width(col)`, `fn header(col) -> (label, hover)` and
   `fn cell(ui, col, row ctx)`, and draw the table by looping over a column list. The list
   is the current fixed order. Split the Known/Difficulty `match`. Before/after PrintWindow
   screenshots should be identical.
2. Move sorting to `sort_key` / `Key`.

Then: the columns file and editor, paging, stat columns, your own columns, filters. Each
step can be screenshot and tested on its own.

## Files

- `src/library_columns.rs` (new): `ColumnsFile`, load/normalise/save, `pack_pages`,
  `Filter` + `Filter::matches`, `Key`, value rekey/forget. Pure logic, unit-tested.
  **Add it to build.rs `NOT_ANALYSIS`**, or every Library is re-analysed.
- `src/chapter_lengths.rs` (new): the saved lengths, the background worker, `through`
  (moved from `home.rs`). Also goes in `NOT_ANALYSIS`.
- `src/home.rs`: take lengths from `chapter_lengths`; `find` keeps the table of contents
  and the large cover.
- `build.rs`: `HANSTAT_READER_TEXT_FINGERPRINT`.
- `src/series.rs`: `from_opf`, `of`, the saved `book-series.json` and its background
  reader. It is already in `NOT_ANALYSIS`.
- `src/plan.rs`: `Library::set_series`; `build` no longer parses titles. Plan inputs gain
  the series hash.
- `src/main.rs`:
  - `history_view`: table loop, pager, chips, Columns window, filter popups, cell editing;
  - `HistCol`: new variants;
  - `title_column_width`: Book width rule;
  - `finish_removal`: forget values;
  - `App`: the loaded `ColumnsFile`, page anchor, open editor/popup state.
- `src/duchinese/ui.rs`: rekey values beside `history.rekey`.
- `Cargo.toml`: `egui_extras = { version = "0.37.0", features = ["datepicker"] }`. This
  pulls in `jiff` (checked in `egui_extras-0.37.0/Cargo.toml`: `datepicker = ["jiff"]`;
  `DatePickerButton::new(&mut jiff::civil::Date)`). It is the only new dependency.

## Tests

- **Unit tests** (`library_columns.rs`):
  - `pack_pages`: everything fits, exact fit, one column too wide, no columns.
  - Normalising: an old/empty file gives the defaults in today's order; unknown and
    duplicate columns are dropped; a missing default is re-added; a filter on a deleted
    field is dropped.
  - JSON round trip.
  - `Filter::matches` for each kind, including "no value".
  - Sort keys: empty values in both directions; text ignoring case.
  - Rekey and forget of values.
  - Each of the five value types: JSON round trip, sort order, filter (including Date's
    day-number ↔ `jiff::civil::Date` conversion around 1970 and leap days).
  - Delete with Undo: values come back on Undo, and are gone from the saved file after it
    runs out.
  - Search: a word found only in a Text column matches; two words split between title and
    a Text column match; a wildcard word must match a whole field.
  - `library_hide_read: true` in an old `UiState` becomes the ✔ Read = not read filter.
- **Unit tests** (`chapter_lengths.rs`):
  - `through` (its Home test moves with it).
  - A file with another reader fingerprint is dropped.
  - A missing or unreadable file starts empty.
  - Forget.
  - One test on a real EPUB from the test corpus: the saved lengths equal
    `chapter_text_len` for each chapter, and the progress at a known place matches what
    the reader's own `progress()` (`src/reader/mod.rs:677`) gives there.
- **Unit tests** (`series.rs`), each with a small hand-written OPF:
  - EPUB 3 collection form;
  - EPUB 2 `calibre:` form;
  - both forms present, where the EPUB 3 form wins;
  - a collection with `collection-type` `set` or no type, ignored, falling back to
    `calibre:series`;
  - a missing or bad index, which gives 1;
  - `2.5` → `[25]`;
  - a metadata name and a title-parsed key joining across 三體/三体;
  - a title marker ignored when metadata is present.
  - Also: one real calibre-tagged EPUB from the test corpus, if one has the tags (check
    with a quick scan of the OPFs first).
- **`history_sort` compatibility:** a `UiState` saved by the current build (`"Analysed"`,
  `true`) still deserialises after `HistCol` gains variants.
- **GUI** (scratch `LOCALAPPDATA` with 3–4 books, `writing_style: false`,
  `HANSTAT_NO_UPDATE=1`, as in the GUI-testing notes):
  - window narrowed until there are 2–3 pages, PrintWindow shots of each page;
  - pager and Columns window driven through UI Automation (`▲`, `▼`, checkboxes,
    "＋ Add column");
  - a filter chip shot;
  - a text cell edited and the window restarted to confirm it was saved.
  - Run `cargo fmt --check`, `cargo clippy --release --all-targets --locked -- -D warnings`
    and `cargo test --release --locked` before pushing.

## Pitfalls

- **The date picker re-analyses every Library once.** Cargo.toml and Cargo.lock are
  hashed into `HANSTAT_ANALYSIS_FINGERPRINT`, so adding `jiff` changes it. Every user's
  books are analysed again after updating (minutes per Traditional book with CKIP).
  - The user accepted this.
  - Ship the date picker in the same release as any other change that already changes
    that fingerprint, if one is coming.
  - Say it in the release notes.
  - Don't test it against the real Library (scratch `LOCALAPPDATA`, as in the
    GUI-testing notes).

- **`UiState` is all-or-nothing:** a field that fails to deserialise loses the whole
  struct (tab, window size). Only *add* variants to `HistCol`, never rename existing ones.
  The user field variant is `HistCol::Field(u32)` (a newtype variant serialises as
  `{"Field": 3}`, which older builds just fail on and default).
  - **Downgrade:** an older build reading a `history_sort` of `Field(3)` falls back to
    defaults for the whole `UiState`. That is acceptable but worth knowing.
- **Rows aren't cached:** the table rebuilds and sorts every frame today. Filters and
  reading-stats lookups add per-row work. Fine for hundreds of books; if it shows, cache
  rows with a `RowsKey`-style key as the Words/Characters tables do.
- **Popups inside `TableBuilder` header cells:** these must be anchored to the `⏷`
  response (`egui::Popup::from_response`) so they aren't clipped by the header row.
- **CRLF:** check `src/main.rs` line endings before scripted edits (LF as of 2026-11-24).

## Later (not in this plan)

- Per-column width choice.
- Hashing only the analysis crates' versions into `HANSTAT_ANALYSIS_FINGERPRINT`, not the
  whole Cargo.lock. Then a UI-only dependency like `jiff` wouldn't re-analyse Libraries.
- Setting or correcting a book's series by hand in hanstat. This would override both the
  metadata and the title.
- A keyboard shortcut for the pager (Ctrl+PageUp/PageDown are taken by tabs).
