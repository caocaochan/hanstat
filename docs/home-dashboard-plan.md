# Home dashboard: plan

Written 2026-11-24 and implemented the same day (uncommitted at first), in `src/home.rs`.

**Where the implementation differs from the plan below:**

- **Activity (user's changes, same day):** first one month, then "really small squares",
  and then the **whole year**. The page now reuses `charts::calendar` (weeks across,
  Monday on top), as in Library Stats.
  - It is held to about 11-point cells (`YEAR_W`) and dims the days still to come.
  - Its title line gives the year's reading time and how many days had reading.
- **Totals:** a row of their own below the calendar, titled "All time". The four tiles
  sit in one row when each can be at least 180 pt wide, and two by two otherwise.
- **Header:** a greeting ("Good evening", by the local hour) over the date, with
  "📖 N books in your Library ›" at the right. The streak moved to the Today column,
  under the ring.
- **Today:** the ring shows today's minutes in its middle and fills towards the goal.
  - The goal can be set or changed from Home ("Set a goal…" / "Change goal…"), the
    same way as in the reader's Stats panel (`set_goal` from today on).
- **Shelf:** shows as many covers as fit (at most 8), with no sideways scrolling.
  - Under each cover: the progress bar, then the title (two lines at most), then "42% ·
    3 d ago". "<1%" is shown for a book just started.
  - The count beside "Recently read" leaves out the hero.
- **Covers:** cut to a paperback's proportions: 184×262 pt for the hero, 120×171 pt on
  the shelf.
  - Large covers are resized with Lanczos3, never enlarged. Small covers keep their old
    `thumbnail` path.
  - A book with no cover gets a tile in an accent picked by a hash of its id.
- **Book details:** one background thread at a time (`HomeView::request`), the hero
  first. For each book it gives:
  - the chapter lengths (`reader::chapter_text_len`);
  - the table of contents, for the chapter name;
  - the large cover, made once from `extract::load` when the book has a small cover.
  These are kept in memory only, and forgotten when a book is removed or re-keyed
  (DuChinese).
- **Known words:** counted from `known.merged` (Anki + lists). With no book opened yet,
  Home builds `merged` once (`update_merged`).
- **Leaving Home:** Home also shows whenever no book is open or on its way
  (`App::on_home`). `open()` and multi-file opens leave Home.
  - The Ctrl+1…6 tab shortcuts leave Home; Ctrl+PageUp/PageDown do nothing on Home.
  - Ctrl+H (`Action::Home`) goes back to Home.

Today the window opens on an empty "Drop .epub or .txt files here, or click Open…" page
(`main.rs`, the `self.report.is_none()` branch of the central panel). This plan replaces
that page with **Home**: an airy dashboard built around the books you are reading. Home
uses the app's own Catppuccin chrome (`palette.rs`), not another reader's look.

## The user's choices

- **Sections:** Continue reading (the hero), Recently read shelf, Today & streak,
  Activity and Totals.
  - Not wanted: Up next on the reading path, Series to continue, Recently added and
    DuChinese.
- **Cover click:** opens the book in the reader where you left it. A small "Overview"
  link appears on hover and opens the analysis instead.
- **Navigation:** the app starts on Home. A 🏠 Home toolbar button and a shortcut go
  back to it. While Home is shown, the book-only toolbar items are hidden.

## Layout

The content sits in one centred column no wider than 1,120 pt, with the rest of the
window as margin. Spacing comes from a fixed scale: 8 / 16 / 24 / 40 pt.

- **Sections:** separated by 40 pt of space, with no boxes around them.
- **Cards:** only the small figure tiles are cards (`palette::card`, `base` fill, no
  stroke).
- **Breakpoint:** below about 900 pt the two-column rows stack into one column.

```
 Tuesday, 24 November                                        🔥 12-day streak
 ───────────────────────────────────────────────────────────────────────────
 ┌────────┐  死亡万花筒                                  ╭──────╮  Today
 │        │  Chapter 41 · 63% · about 4 h 10 min left   │ 38/45│  38 of 45 min
 │ cover  │  ████████████████████░░░░░░░░░░░░           ╰──────╯  12,400 chars
 │180×270 │  [ ▶ Continue reading ]   Overview                    Best streak 21
 └────────┘
 Recently read  8                                                      See all ›
 ┌────┐   ┌────┐   ┌────┐   ┌────┐   ┌────┐   ┌────┐
 │    │   │    │   │    │   │    │   │    │   │    │     120×180 covers,
 └────┘   └────┘   └────┘   └────┘   └────┘   └────┘     24 pt apart
 title    title    title    title    title    title
 ▬▬▬▬░░   ▬▬░░░░   ▬▬▬▬▬▬   ...                          thin progress bar
 Activity                                 Totals
 ▢▢▢▢▢▢▢▢▢▢▢▢▢▢▢▢  (16 weeks)              ┌──────────┐ ┌──────────┐
 ▢▢▢▢▢▢▢▢▢▢▢▢▢▢▢▢                         │ 14 books │ │ 212 h    │
 ...                                      │ finished │ │ read     │
                                          ├──────────┤ ├──────────┤
                                          │ 9.8 M    │ │ 7,071    │
                                          │ chars    │ │ words    │
                                          └──────────┘ └──────────┘
```

### Header line

The weekday and date on the left, in weak 14 pt type. On the right, the current reading
streak, shown in peach once there is one. Nothing else goes here: no title or logo.

### Continue reading (the hero)

This is the last Library EPUB read in the reader (`App::last_read`).

- **Cover:** 180×270 pt, rounded 10 pt, with a soft shadow.
- **Title:** 26 pt semibold.
- **Meta line:** the chapter, the % through the book, and the time left at your measured
  speed: (1 − %) × `han_total` ÷ `App::cpm()`.
- **Progress bar:** 6 pt tall, mauve on `surface0`.
- **Buttons:** a filled mauve **Continue reading** button and a plain **Overview** link.

When nothing has been read yet but the Library has books, the hero shows the most
recently analysed book as **Start reading**.

### Today & streak

This sits to the right of the hero on wide windows. Data: `reading_stats::Summary`
(`goal`, `read_streak`, `goal_streak`, today's `Day`).

- **Ring:** a 96 pt progress ring in peach, filled by today's progress towards the goal
  (`Goal::progress`).
- **Beside the ring:** minutes and characters read today, and the best streak.
- **No goal set:** the ring is replaced by plain figures for today and a "Set a goal"
  link, which opens the place where goals are set now.

### Recently read shelf

Up to 8 books, most recent `last_day` first (`Summary.books`), without the hero book.

- **Covers:** 120×180 pt, each with a title of up to two lines and a thin progress bar
  underneath.
- **Hover:** the cover lifts slightly and the Overview link appears.
- **Overflow:** when the books don't fit the width, the row scrolls sideways.
- **See all ›** opens Library › Stats › Reading.

### Activity

A 16-week calendar heatmap that reuses `charts::calendar`, so it looks the same as
Library Stats. Days on which the goal was met carry the same mark as there. Hovering a
day gives its minutes and characters.

I picked the heatmap over 7-day bars because it shows streaks at a glance. Swapping it
for bars is a small change.

### Totals

Four tiles in a 2×2 grid: books finished, hours read, characters read, and known words.
The known-words figure is the same union of Anki and lists that the Overview's "Known ="
line counts. Each tile has one line of weak sub-text, such as "+3 this month" or the
known-characters count.

### Empty Library (first run)

The page becomes a welcome page with three parts:

- a large dashed drop zone ("Drop .epub or .txt files here") with an **Open…** button;
- three numbered steps: open a book → choose known words (Anki, HSK/TOCFL) → read;
- the drop-zone text only where dropping works (`drop_works()`).

## Navigation and toolbar

- **New state:** `App.home: bool`. It is true at startup, unless `--tab` is given.
  Opening or dropping a book, picking one in the Library, or toggling the Library sets it
  to false. 🏠 Home sets it to true.
- **Toolbar on Home:**
  - Hidden: Segmenter, forced words, Encoding, and Export's book items. Export keeps
    "Library as image".
  - Kept: Open, Anki, Known, Library and Reader.
  - Off Home: everything returns.
- **🏠 Home button:** the first item after Open. It is a selectable label, like Library.
  🏠 Home and 📖 Library act as two views: picking one leaves the other.
- **Shortcut:** a new `shortcuts::Action::Home`. The default key will be chosen against
  the existing ones when this is implemented (Ctrl+H, if free).
- **Closing the reader:** returns to the view it was opened from. That already happens,
  because the reader covers the window without changing the view underneath.

## Data that must be added

### Larger covers

The saved covers are 120×168 px (`history::COVER_MAX`). At 180×270 pt on a 150–200 % DPI
screen they would be visibly blurry.

- **What:** add `covers/large/<id>.png`, at most 480×720 px.
- **New books:** written at analysis time, next to the small cover (`main.rs` around the
  `save_cover` call), from `Book.cover`.
- **Existing books:** made on demand by a background thread, and only for the books that
  Home shows (at most 9). The thread runs `extract::epub::load` on the Library's copy of
  the file, which already finds the cover the way readers do.
- **Fingerprints:** `src/extract` itself is not edited, so the Library fingerprint is
  unchanged.
- **Fallbacks:** TXT books and books without a cover get a drawn tile: the title on a
  Catppuccin accent chosen from the id's hash.
- **Removal:** removing a book from the Library also deletes its large cover
  (`History::forget`).

### % through the book

`reading_stats::left_at` gives the spine and offset. Turning that into a percentage needs
each chapter's `reader::chapter_text_len`, which means opening the EPUB.

- **When:** the same background thread computes it once per book.
- **Cache:** kept in memory, keyed by (`id`, `LeftAt.saved`), so it is redone only after
  the book has been read again.
- **Interim:** the figure is left blank until it has been computed; no estimate is shown.
  (Characters read ÷ `han_total` is wrong for books entered by jumping.)

### Summary

`Summary` is cached per `reading_stats::revision()`, as in `library_reading.rs`. Home
reuses that pattern and doesn't rebuild the summary every frame.

## Files

- **`src/home.rs` (new):** the page, its layout and its widgets (the ring, the cover with
  its shadow and hover, the shelf). Added to build.rs `NOT_ANALYSIS`.
- **`src/main.rs`:** the `home` state, the toolbar, the central-panel branch, the
  shortcut, the large-cover write, and the background job's polling.
- **`src/history.rs`:** `large_cover_path`, and saving and deleting large covers.
- **`src/shortcuts.rs`:** `Action::Home`.
- **`src/palette.rs`:** any new shared helpers, such as `shadow()` and an accent picked
  by hash.
- **No new dependencies:** no change to Cargo.toml, so the fingerprint is unchanged.

## Phases

1. **Home state and welcome page:** the `home` state, navigation, the slim toolbar, the
   shortcut and the empty-Library welcome page.
2. **Hero and shelf:** the hero and the shelf, with large covers and the % through each
   book from the background job.
3. **The rest:** Today & streak, Activity and Totals; the responsive stacking.
4. **Polish and checks:**
   - screenshots in Mocha and Latte at about 1,280 and 2,560 pt wide, through a scratch
     Library, as described in the GUI-testing notes;
   - unit tests for the time-left and %-through maths and for the breakpoint.

## Risks

- **Large covers for existing books:** each one means parsing a whole EPUB (about 1 s for
  a long novel). That happens off the UI thread and at most 9 times; until a large cover
  exists, the small one is shown scaled up.
- **Shadows in egui:** egui draws shadows as blurred rect meshes (`epaint::Shadow`). A
  large blur on many covers costs some tessellation. If that shows up in frame times, the
  shelf will use a smaller blur.
