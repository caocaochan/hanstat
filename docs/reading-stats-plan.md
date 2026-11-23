# Reading stats for the EPUB reader: plan

Written 2026-11-23 from scratch (the 2026-11-19 plan was discarded). All four phases
were implemented the same day, uncommitted at first.

**Where the implementation differs from the plan below:**

- **Tracker settings:** stored in `reading-stats.json` (`settings`), not `reader.json`,
  because Library Stats needs the day start too.
- **Weekly speed:** drawn as columns. `charts::scatter` has a log x axis and a percent y
  axis, so it doesn't fit.
- **Undo:** no turn-by-turn undo list. "Discard this session…" (with a confirmation)
  covers mistakes.
- **End of the book:** pressing Next on the last page again counts nothing. A test caught
  an early double count.
- **Reading speed (user's change, same day):** the Chinese reading times everywhere use the
  speed measured in the reader (`App::cpm`, from `reading_stats::measured_now`). Settings
  (⚙) has an **Override** toggle that uses `cpm` instead (`Settings::cpm_by_hand`), and
  `cpm` also stands in until a speed is measured. The "use measured speed" buttons are
  gone.
- **Period filter (user's request, same day):** Library Stats › Reading has All time
  (the default) and Year, Month, Week (Monday to Sunday) and Day, with ‹ › to step one
  unit and a "This …" button to jump back to the current one.
  - The tiles, the calendar (other days dimmed), the charts and the book table follow the
    period.
  - The charts show days for a month or week and months for a year; a single day has no
    charts.
  - The choice isn't saved.
- **Storage (user's request, same day):** the stats live in SQLite, `reading-stats.sqlite`
  (rusqlite, already a dependency).
  - WAL mode, `synchronous=FULL`, one transaction per save, written on a background
    thread; only the books that changed are rewritten.
  - `PRAGMA user_version` guards the format: a database from a newer build, or one that
    can't be read, is never written to, and Library Stats says why.
  - A `reading-stats.json` from earlier builds is imported once when there is no
    database, and is left in place.
  - The calendar has its own year ‹ › in every view.
- **Files:**
  - `src/reading_stats.rs`: data, local time, goals, streaks, summary;
  - `src/reader/tracker.rs`: the state machine;
  - `src/reader/stats_ui.rs`: the line, the panel and the hooks;
  - `src/library_reading.rs`: Library Stats › Reading and the book's rows;
  - `charts::calendar`.
  - Both new top-level files are in `NOT_ANALYSIS`.
- **Checks:**
  - unit tests;
  - the ignored `reading_stats_tracker` test, which drives the real reader with a hand-set
    clock;
  - the ignored `reading_han_counts` corpus test: layout equals tree on every chapter of 9
    books, and the reader total is within 0.12% of the Library's;
  - the stats' share of a frame: about 1.2 µs.

The reader records reading time and characters per book and per local day, in its own
file. The figures appear in four places: a live line in the reader's page margin, a stats
panel in the reader, a Reading section in Library Stats, and a few rows in each book's
detail view. Extras: daily goals with streaks, read-aloud time kept apart, Mark read when
a book is finished, and a button that sets `cpm` from the measured speed.

No new dependencies and no change to Cargo.toml, so no Library re-analyses.

## Choices (user, 2026-11-23)

| Question | Choice |
|---|---|
| Start mode | A setting: automatic or by hand. **P** toggles in either mode. |
| What counts | Han characters only, so it matches `han_total` and `cpm`. |
| Going back | Net position: turning back subtracts, turning forward adds again. Jumps don't count. |
| Idle limit | 5 min. The idle stretch is removed. |
| Day starts | 04:00 local time. |
| Surfaces | Live line, stats panel, Library Stats › Reading, per-book rows in the Library. |
| Extras | Daily goal and streaks, listening apart, finish → Mark read, measured speed → `cpm`. |
| Export | None. The file is plain JSON and can be backed up by copying it. |

The feature set is the usual one for these trackers (ttsu documents its tracker and
statistics in its README; Hoshi lists reading statistics and credits ttsu). The look is
not taken from them. Every surface is designed for hanstat (see Design).

## What the code has now (checked 2026-11-23)

- **Position:** `(spine, offset)`. `offset` is in `Chapter::text_len` units, which counts
  letters, digits and Chinese characters together (`tree.rs:96`). A page starts at its
  first layout unit's `Unit::text` (`layout.rs:83`, `page_offset` at `layout.rs:149`).
- **Moves:**
  - `next()` and `prev()` turn pages, and cross into the neighbouring chapter through
    `go_chapter`.
  - `follow()` (links) and the contents menu jump through `go_chapter` too.
  - `follow_reading()` turns pages while read aloud plays.
  - A resize or font change lays the chapter out again without calling any of these, and
    keeps `offset`.
  - `go_chapter` is shared by turns and jumps, so it cannot tell them apart. The *call
    site* has to say which it is (see Tracker).
- **Saving:** `saver::write` writes off the UI thread, to a temporary name that is then
  renamed into place. `App::on_exit` saves and flushes. Reader settings are in
  `reader.json` (`mod.rs:110`).
- **Chrome:**
  - The reader draws a running head and a folio, "N / M", in the page margins in
    `theme.muted` (`mod.rs:1106`).
  - Reader colour themes are in `reader/theme.rs` (`page`, `bar`, `text`, `subtle`,
    `muted`, `accent`…).
  - The app's own chrome is Catppuccin (`palette.rs`: `card()`, `ink::*` for text).
  - Library Stats uses `Tile`/`tile_grid`/`card`. `charts.rs` has `columns`, `scatter`,
    `stacked_bar` and `compact`.
- **App wiring:** while the reader is open, `App::ui` calls `r.ui()` and returns early
  (`main.rs:2939–2947`), so nothing else is drawn. `ReadDialog` is in `read_books.rs`.
  The book detail view shows "Reading time … at N chars/min" (`main.rs:4947`).
- **Dates:** `history::date()` is UTC only, and there is no chrono crate.
- **Platform crates:**
  - `windows-sys` has no `Win32_System_Time` feature.
  - `libc` is a dependency on Linux only.
- **Fingerprint:** `src/reader` is already in build.rs `NOT_ANALYSIS`. A new
  `src/reading_stats.rs` must be added there in the same commit.

## Design

- **Fits hanstat, copies no one:**
  - Inside the reader it uses the current reader theme's own colours (`muted` and
    `subtle` for figures, `accent` for the goal bar), unchanged (see the "theme colours
    untouched" rule).
  - In the Library it uses Catppuccin chrome: `card`, `Tile` and `palette::ink` for text.
- **The reader is for reading:**
  - The live line shares the bottom margin with the folio. It is the same small size
    and colour, so it never moves or shrinks the page area.
  - No animation and no bright colours while the tracker runs.
  - The live line can be hidden in the panel.
- **State at a glance:** the line starts with the session time when running. When
  paused it shows "paused" plus the reason (idle, window inactive, menu open, by hand).
  Clicking the time toggles the tracker, as P does.
- **Every number has hover text** saying what it is and how it was counted.
- **Empty states say what to do,** never "0", "NaN" or "0 chars/min":
  - "Speed after a minute of reading";
  - "No reading recorded yet — open a book in the reader".
- **Performance budget:**
  - The tracker's per-frame work is O(1): a few comparisons and additions.
  - The file is serialised at most every 30 s and written by `saver`.
  - Library Stats builds its aggregates (per day, streaks, speeds) once per file
    revision, not every frame.
  - The calendar is one painter pass of at most 371 rects.
  - Check: reader frame times unchanged within noise, release A/B, medians over 3 runs.

## Han counts (`tree.rs`, `layout.rs`)

- `Chapter` gets `han_len: u32`: the Han characters in the chapter, counted with
  `chars::is_han` where `text_len` is counted.
- `Unit` gets `han: u32`: the Han characters before the unit, next to `text`. So the Han
  count at any page start is exact, not a ratio.
- `ChapterLayout::han_at(offset)` finds the unit with `text <= offset` (as
  `page_of_offset` does) and returns its `han`.
- The book's Han total is the sum of `han_len` once every chapter is built.
- The reader parses the EPUB with its own tree, while the Library counts from
  `src/extract`, so the two totals may differ slightly (hidden elements, navigation
  pages). The corpus check measures the gap. Progress and "time left" use the reader's
  own total.

## Tracker (`src/reader/tracker.rs`)

A pure state machine. It takes `now` and move events, so tests drive it with a fake
clock.

- **States:**
  - `Running`;
  - `Paused(reason)`, where the reason is `Manual`, `Idle`, `Unfocused` or `Menu`.
  - Only `Manual` needs P to leave it. The others resume on their own when the cause ends.
  - Start mode "by hand" opens a book in `Paused(Manual)`.
- **Auto-pause:**
  - the window loses focus or is minimised (`i.focused`, viewport info);
  - while the contents menu, the Aa panel, the Dictionaries window or the stats panel is
    open;
  - after 5 min with no input. The time back to the last input is removed.
  - Input means keys, pointer movement, clicks, scrolling and popup lookups.
  - While read aloud plays, the idle rule is off (see Listening).
- **Moves:** each call site tells the tracker what kind of move it is.
  - `Turn`: `next`, `prev`, and `follow_reading`, which counts as listening.
  - `Jump`: contents, links, the place restored on open, and a chapter picked in the
    panel.
  - The new place is known only once the target chapter is laid out (`Target::Last` on a
    backward chapter cross). The tracker keeps the pending kind until the reader reports
    the settled `(spine, offset)`.
- **Characters (net position):**
  - A `Turn` while running credits the signed Han count between the old and new place.
    Across chapters this is the tail of the old chapter + `han_len` of any chapters in
    between + the head of the new one.
  - A `Jump`, a turn while paused, or a relayout moves the base without counting.
  - A day's or session's total never goes below 0. A back turn takes from today's count
    down to 0, and the rest becomes the book's `debt`, which forward turns pay off before
    they count again. Paging back first thing in the morning therefore doesn't eat into
    today's goal, and re-reading isn't counted twice.
  - Next on the book's last page credits the rest of the book and sets `finished`.
- **Time:** while running, the time from the last state change is added on each pause,
  each turn and each 30 s tick. Nothing accumulates per frame.
- **Day split:** the day starts at 04:00 local. A span crossing it is split between the
  two days. Local day = `floor((t + utc_offset(t) − 4h) / 86400)`.
- **Speed:** chars ÷ reading minutes. It is shown only after ≥ 1 min of reading and ≥ 1
  credited turn.

## Listening

While read aloud plays (Azure or the local voice), time and turns go to `listen_secs`
and `listen_chars`. They are shown separately and never enter reading speed, goals, or
the `cpm` estimate. Audiobook export is not tracked.

## Local time (`src/reading_stats.rs`)

`utc_offset(t) -> i32`, by OS functions declared by hand, so no Cargo.toml change:

- Windows: `SystemTimeToTzSpecificLocalTime` from kernel32, via a hand-written
  `extern "system"` with `#[repr(C)] SYSTEMTIME`;
- Linux: `libc::localtime_r` and `tm_gmtoff`;
- macOS: a hand-written `extern "C" localtime_r` with a `#[repr(C)] tm` that includes
  `tm_gmtoff`.

The result is cached per hour of `t`.

## Data (`src/reading_stats.rs`, shared by the reader and the Library)

`app_dir()/reading-stats.sqlite` (see the note at the top; this was JSON at first):

```rust
struct ReadingStats {
    version: u32,
    books: HashMap<String, BookStats>,   // key: the reader's book_id (Library id or path)
    goals: Vec<Goal>,                    // dated: a day is judged by the goal then in force
}
struct BookStats {
    title: String,
    han_total: u32,                      // the reader's count
    first_read: Option<u64>, finished: Option<u64>,
    debt: u32,
    days: BTreeMap<i32, Day>,            // local day number
    sessions: Vec<Session>,
}
struct Day { read_secs: f64, read_chars: u32, listen_secs: f64, listen_chars: u32 }
struct Session { start: u64, end: u64, read_secs: f64, read_chars: u32,
                 listen_secs: f64, listen_chars: u32 }
struct Goal { from_day: i32, minutes: u32, chars: u32 }   // 0 = off
```

- **Tracker settings** go in `reader.json`, next to the other reader settings: start
  mode, idle limit, day start, and whether the live line shows. The history file holds
  history only.
- **Writes:** through `saver` every 30 s while running, on every pause, and on close. A
  crash loses at most 30 s.
- **Removal:** removing a book from the Library keeps its stats.
- **Size:** a few sessions a day for years is a few hundred KB.
- **Revision:** a process-wide counter increments on each write, so Library Stats knows
  when to rebuild its cache.

## Surfaces

1. **Live line** (reader bottom margin, either side of the folio; in a two-page spread,
   one half under each page):
   - left: state or session time · characters · chars/min;
   - right: time left in the chapter · time left in the book;
   - today's goal as a hairline in `accent` along the margin, when a goal is set.
   - "Time left" uses this book's measured speed once it has ≥ 10 min of reading, then
     the overall measured speed, then `cpm`.
2. **Stats panel** (a top-bar button next to Aa, drawn like the Aa panel):
   - the tracker state and a start/pause button;
   - session, today, this book and all time (time, characters, speed, listening);
   - this book's first read and finish dates, days read and progress;
   - today's goal progress and the streaks;
   - "Discard this session";
   - "Use measured speed: N chars/min (now M)";
   - tracker settings and the goal.
3. **Library Stats › Reading** (its own section, Catppuccin cards):
   - tiles: total reading time, characters, average speed, current and longest streak,
     books finished, listening time;
   - a calendar, `charts::calendar` (new): minutes per day over the year, goal-met days
     marked, hover for the day's details, a year switch;
   - the last 30/90 days of time and characters (`charts::columns`);
   - weekly speed (`charts::scatter`);
   - a per-book table: time, characters, speed, first read, finished, progress. A row's
     context menu offers "Remove this book's reading stats".
   - The measured-speed button again.
4. **Book detail** (Library, under "Reading time"):
   - "Read so far: 3h 20m, 61,200 chars (34%), 305 chars/min";
   - first read and finish dates;
   - "≈ 6h 05m left at your speed".
   - Hidden for a book with no reading data.

## Goals, streaks, finishing

- **Daily goal:** minutes, characters or both. 0 turns one off; with both set, a day
  needs both. A change applies from that day on. Past days keep the goal they had.
- **Streaks:**
  - days read (≥ 1 min of reading);
  - days the goal was met.
  - Today does not break a streak until it is over.
- **Finish:**
  - Next on the last page sets `finished` and, for a Library book, raises
    `Reader::finished` (a book id).
  - App takes it in the reader branch of `App::ui`, after `r.ui()` and before the early
    return, and opens `ReadDialog` there. Phase 4 first checks that the dialog's window
    draws over the reader.
  - Books opened from outside the Library record the date with no dialog.

## Phases

Each phase ends with its tests passing and the performance check done.

1. **Core:**
   - Han counts in `Chapter` and `Unit`, local time, the data file and the tracker (move
     kinds, auto-pause, day split, debt, listening);
   - the live line, P, the start-mode setting;
   - `reading_stats.rs` added to `NOT_ANALYSIS`.
   - Listening is in from the start, so early data is not skewed.
2. **Reader panel:** the stats panel, discard session, time left, the measured-speed
   button.
3. **Library:** the book detail rows; Library Stats › Reading with tiles,
   `charts::calendar`, columns, scatter and the per-book table, aggregates cached by
   revision.
4. **Goals and finishing:** goals, streaks, the goal hairline, Mark read on finish.

## Testing

- **Tracker unit tests (fake clock, fake positions):**
  - forward and back turns, debt paid off, jumps and relayouts not counted;
  - a backward chapter cross resolved after layout;
  - idle removal, focus loss, menus;
  - the 04:00 split and listening kept apart;
  - the end of the book, discard session.
- **Data tests:**
  - round-trip unchanged;
  - an older file without the new fields still loads;
  - local days across a DST change;
  - streaks with goal changes.
- **Corpus check (ignored test):** the reader's summed `han_len` against the Library's
  `han_total` on the test books. Report the gap, and require the sum of page-by-page
  credits to equal `han_len`.
- **GUI (ignored `drive_ui` test in `preview.rs`, as the other GUI tests):**
  - a scratch `LOCALAPPDATA` with 3–4 short books and `HANSTAT_NO_UPDATE=1`;
  - turn pages with fake time, then check the live line and the written file;
  - check the calendar and the book detail from PNGs.
- **Never touch the real `reading-stats.json` in tests:** use the scratch app dir.

## Not included

- Export or sync (ttsu has GDrive/OneDrive).
- Hand-editing days.
- Per-hour speed.
- "Freeze position". Debt covers re-reading.
