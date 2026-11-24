# Plan: Reading path "Learn while reading" mode

Status: implemented 2026-11-24, released in 2026.11.24.02. See
"Implementation notes" at the end for where it differs from this plan.

## Goal

The Reading path gets a second mode, chosen at the top of the tab:

- **Learn while reading** (the default): no study before a book. Reading a book
  teaches you its frequent words, and books are ordered so that each one is as
  easy as possible when you start it.
- **Study before each book**: today's mode, unchanged.

Three things make the new mode match real reading:

- Words met across several books count. ✔ Mark read records them too.
- The opening of each book is computed from where its words actually occur.
- The book you're reading now stays first, credited with what you've read of it.

Someone asked for this on Discord:

> most of us don't prestudy … a reading path where I'm learning while reading.
> So still ordered from easier to harder, and assuming I learn x percent of words
> from each book I read. But taking into account unique words vs length instead
> of just unique words

## Why the current mode doesn't fit

In study mode a step costs the words you study to reach 95% of a book. On the
Library (see Evidence), that count rises with book length (Spearman +0.47),
because long books have more distinct words and a longer tail. Under the Peak
objective, long books therefore drift later.

A learner who doesn't pre-study experiences something different:

- the **density** of unknown words, which sets how often they look something up;
- density that **falls as the book goes on**, because its frequent words become known;
- a long book that teaches **more** for the next books.

Length counts against a book only in study mode's cost.

The existing "Words read N+ times are learned" option can't express any of
this. It only adds words *after* a book. The step cost is still the study count,
or, with a fixed number, the coverage at the first page.

## The model

### What reading teaches

Let `M` = `plan.json`'s `read_min_count` (default 10) and `P` = `read_percent`
(default 90). These are the settings the Mark-read dialog already remembers.
Reading teaches words in two ways.

**In one book.** Of the book's words that occur ≥ `M` times in it, the `P`%
most frequent are learned. This is `known::read_selection` over the profile's
words, exactly as ✔ Mark read applies it today (names and already-known words
included, so the bottom `(100−P)`% is the same cut). The path then drops what's
already known and the names.

**Across books (new, Mark read too).** A word not learned in one book counts as
known once:

- it has occurred in **two or more** books you've read, and
- its meetings across them add up to **≥ `M`**.

The two-book condition keeps the `P`% hedge meaningful. Without it, a word in
a book's bottom 10% (met ≥ `M` times there) would be learned from that one book
anyway. It is also where the research supports learning best: meetings spread
over time.

Both the path and Mark read use these rules, so:

- The path predicts exactly what Mark read will record. After Mark read, the
  path recomputes from real knowledge, so the model never drifts from the
  app's state.
- There's one place to change the rule. Changing it in the path changes the
  Mark-read default too, and the other way round. The control's tooltip says so.
- "Learn x percent of words" in the Discord message is read as "x% of the words
  met at least M times", not "x% of all unknown words". A flat x% of unique
  unknown words would mostly be words that appear once or twice, which nobody
  learns from one meeting.

Why 10 meetings is a sensible default (recalled, not re-checked for this plan):
incidental-acquisition research puts useful meaning recall at around 10+
encounters, with wide variance. A reader with the popup dictionary looks words
up, which helps retention.

### What you meet while reading a book

Each word `w` that is unknown when the path reaches the book has:

- `e`: meetings in earlier books (read ones, and ones earlier on the path);
- `n`: occurrences in this book.

It is learned in this book if either:

- it is in the book's selection, after `M − e` more meetings (`M` if `e` = 0); or
- `e > 0` and `e + n ≥ M`, after `M − e` more meetings.

Until then, every meeting is an unknown word met (a lookup). Otherwise all `n`
meetings are lookups.

- **Over the book** = `1 − lookups / T`, where `T` = running words without names.
- **At the start** = the same over the first `W` = 20,000 running words,
  counted at the **actual positions** of the meetings (see Word positions).
  For a pinned book it is the 20,000 words from where you are. A book shorter
  than `W` is all opening.

With even spread, coverage could only rise through a book, so the opening was
the hardest stretch by construction. With real positions it is usually the
hardest, because new names and terms keep coming. "At the start" means what it
says, not "the hardest stretch".

### Word positions

The profile gets a sidecar with, for each word, the running-word positions of
its **first 32 occurrences**. It also holds a table of sections (`href`, first
running word, length in running words and in characters) for mapping reader
places.

32 covers any `M` ≤ 32 exactly. Past the 32nd occurrence, the rest are assumed
evenly spread to the end of the book, as are all occurrences of a word whose
sidecar is missing.

Measured on the 85 profiles in the Library, from their counts:

| | Median per book | Largest book |
|---|---|---|
| Positions to store (Σ min(n, 32)) | 90,333 | 272,064 |
| Today's profile `.json.gz` | 91 KB | 204 KB |

As delta-varints that is about 2 bytes a position, so roughly 180 KB per book
before gzip and around 2–3× today's profile size. That's an estimate; the real
size is measured when implemented.

The sidecar is binary (`profiles/<id>.pos.gz`, flate2 is already a dependency),
not JSON. The Library and Overview load profiles constantly and must not parse
90k integers each time. Only the Reading path loads sidecars.

**Re-analysis.** Positions need the token stream, which the analysis cache
doesn't keep. So every book is segmented again once:

- A book with no sidecar counts as needing analysis in the Library's background
  update, like a book with no profile today. The path tab shows "N books use an
  estimate of where their words occur" with an **Analyse N** button, the
  existing backfill.
- Until then those books use even spread, so the path works during
  re-analysis.
- Traditional books on CKIP take minutes each. The status bar says so (see
  `docs/deferred-improvements.md`).

### The book you're reading now

A book is **started** if:

- the reader has a place for it (`reading_stats::left_at`),
- that place is past its first 1% of running words, and
- it isn't marked read or left out of the path.

The planner's order puts started books **first, most recently read first**.
The search orders the rest. A hand-set order is kept as set, but its started
books still get the figures below.

A started book's step works like this:

- **Read part.** The meetings before your place count. Words reaching their
  learning point there are learned, and the rest add to `e` for later books.
  This is the path's prediction only: `known.json` changes on Mark read, as
  today.
- **At the start**, **Over the book** and the lookups are for the part still to
  read.
- **Teaches** is the whole book. The hover says how many of those are learned
  already, at your place.
- The # column shows 📖 and how far you are, e.g. "📖 37%".

Mapping a place to a running word: the reader's `LeftAt { spine, offset }`
gives a section by its spine `href`, then a fraction (offset ÷ section length)
gives a running word inside it. Before relying on it, check two things: the
unit of `offset` (the reader's text vs the analysis section text), and that
every spine `href` the reader uses is in the analysis sections. Unmapped
places count as not started.

### Which order

- **Step difficulty** = coverage at the start, less the writing-style offset.
  The offset works the same way as under a fixed number.
- **Peak** (default) = the highest lowest coverage at the start, then the
  fewest lookups in all.
- **Total** = the fewest lookups in all, then the highest lowest coverage at the
  start.

  Every book is read, so this is the highest token-weighted average coverage.
  It **rewards long books**: they teach the most, so reading one earlier cuts
  lookups in every book after it.

The beam search, series order, left-out and read books work as today. Only the
step and its cheap cost change.

**Rejected: coverage over the whole book as the step difficulty.** It is what
"unique words vs length" suggests literally, but it hides a hard opening. Greedy
by whole-book coverage puts 我在无限游戏里封神 (1.47M running words) second on
the path. That book starts at 73.7% coverage and averages 92.0% only because it
teaches 5,534 words. It lowered the path's lowest coverage at the start from
82.3% to 80.7%, while saving 6% of lookups. A reader drops a book at its
opening, not at its average. The whole-book figure is still shown, and the
Total objective is the place where that trade can be made on purpose.

## Evidence (measured 2026-11-24, scratch script, nothing in the repo)

Setup:

- 66 Simplified books with profiles from the real Library (16 non-Simplified
  skipped, because the script doesn't fold T→S).
- Learner knows HSK 3.0 bands 1–5 (3,557 words), as in `plan_eval`.
- Rule 10× / 90%, `W` = 20,000.
- **Even spread, and meetings counted within one book only.** Positions and
  cross-book counting will change the figures, and `plan_eval` re-measures them.
- The script is a Python re-implementation (scratchpad `lwr_eval.py`). Hapax are
  kept and `skip_hapax` isn't applied.

Spearman correlation with book length (running words):

| Measure | ρ |
|---|---|
| Distinct words | +0.85 |
| Words to study for 95% (current mode's cost) | **+0.47** |
| Coverage at the first page | −0.26 |
| Coverage at the start (`W`, with learning) | −0.32 |
| Coverage over the whole book (with learning) | **+0.39** |
| Words learned from the book | +0.92 |
| Lookups per 1,000 running words | −0.39 |

Coverage at the start and at the first page rank books almost identically
(ρ +0.97) under even spread. Positions should separate them: books whose key
words come early get an easier start.

| Path, greedy | Position ~ length | Lowest at start | Lookups in all |
|---|---|---|---|
| By coverage at the first page | +0.27 | 82.3% | 1,458,783 |
| By coverage at the start (**proposed**) | +0.28 | 82.3% | 1,460,919 |
| By coverage over the whole book (rejected) | +0.10 | 80.7% | 1,368,130 |

Long books still come somewhat later because, in this Library, they really do
start harder. That looks like a genre effect: the long ones are infinite-flow
and xianxia novels.

## Changes

### Analysis and profiles (`src/analyze.rs`, `src/profile.rs`, `src/history.rs`)

- Where word counts are tallied, also record each word's first 32 positions
  and the section table. The result is written as `profiles/<id>.pos.gz` next to
  the profile, by the same saver (fsync, set-aside on unreadable).
- `profile::Positions { sections, words: Vec<Vec<u32>> }` is parallel to
  `Profile::words`. It has a format byte, and the profile's word count is
  checked on load (a mismatch is treated as missing).
- It is deleted with the profile.
- The Library's background update treats a missing sidecar as "needs analysis".

### Known words and Mark read (`src/known.rs`, `src/read_books.rs`)

- On marking a book read, copy its profile to `read/<id>.json.gz`. This freezes
  its counts like `ReadBook::words`, so re-analysis or removing the book from
  the Library doesn't change what it taught.
  - Books marked read before this change get a copy the first time, if their
    profile still exists. Otherwise they don't count towards meetings across
    books, and Library Stats › Books read says how many don't.
- `known::cross_book(read: &[(Vec<(String, u32)>, &ReadBook)], min_count, fold) -> Vec<String>`
  applies the rule from "What reading teaches", folding T/S like the rest of
  the known words.
- `known::Settings` gets `across: Vec<String>` (saved, skipped when empty). The
  App recomputes it when:
  - a book is marked or unmarked,
  - `read_min_count` changes,
  - the T/S matching setting changes.

  `words()` includes it. The label reads e.g. "3 books read (+412 across books)".
- The Mark-read dialog shows "+N words now met M+ times across the books you've
  read" before you confirm. Unmarking takes them back if they no longer qualify.

### `src/plan.rs`

- `Study::Read { percent: u8, min_count: u32 }` becomes the third variant.
  `Params::incidental` and `lookahead` are ignored under it, since nothing is
  studied.
- `Library::build` takes optional `Positions` per book. `Library` gets the
  initial meetings `e0`: one byte per vocabulary word, saturating, from the
  frozen read-book counts of words not known.
- `State` gets the meetings vector, cloned like `known`. At about 150k
  vocabulary words that's 150 KB per state, well within the beam's widths.
- `Key` gets a **peak part and a sum part per step** instead of one cost:
  `step_cost` returns `(peak, sum)`.
  - Target: `(words, words)`.
  - Fixed: `(−coverage·100, −coverage·100)`.
  - Read: `(−(start − offset)·100, lookups)`.

  `add_cost` takes the pair. Behaviour of the existing modes is unchanged, and
  their tests prove it.
- `Step` gets:
  - `start: f64`, `lookups: u64`, `learned: Vec<u32>`;
  - `from: Option<f64>` (where a started book is, as a share of it);
  - `learned_so_far: usize`.

  `reading` holds the coverage over the (remaining) book. `study` stays empty.
- `Library::step` and `Library::cheap` gain the Read arm.
  - The cost is O(Σ min(n, 32) over unknown words) with positions, a few times
    today's O(words). `width_for` counts that, so the search keeps its time
    budget and narrows the beam instead.
- `Library::optimise` takes the started books (`[(book, from)]`) and fixes them
  as the first steps.
- `Path` gets `lowest_start()` and `total_lookups()`.
- Module docs get a **Learning while reading** paragraph.

### `plan.json` (`plan::Settings`)

- New field `mode: Mode { Study, Read }`, defaulting to `Read`: a `plan.json`
  without it (every existing one) loads as Learn while reading. `fixed`,
  `target`, `learn`, `incidental*` and `lookahead` stay as they are, so
  switching back to Study restores the user's old settings.
- New field `objective: Objective`, default `Peak` (today's behaviour). It's
  saved as `"peak"`/`"total"` and shared by both modes.
- `params()` maps `Read` to `Study::Read { read_percent, read_min_count }` and
  passes the saved objective instead of `Objective::default()`.

### Reading path tab (`src/main.rs` `plan_view`, `start_plan`)

- `PlanInputs` gets:
  - the started books and their places (rounded to 0.1% so paging doesn't
    replan every page);
  - the across-book state;
  - which books have positions.

  `start_plan` loads sidecars and read-book copies in the background thread,
  as it loads profiles now.
- A mode row first: **○ Learn while reading ○ Study before each book**.
- An objective choice on the same row, worded per mode:
  - Read: **Easiest hardest start** (Peak) / **Fewest lookups in all** (Total).
  - Study with a target: **Smallest largest step** / **Fewest words in all**.
  - Study with a fixed number: **Highest lowest coverage** / **Highest average**.

  The tooltip explains the trade: Total can put a long book early when it
  saves lookups (or words) later.
- Study keeps today's controls.
- Read shows: "A book teaches **[90]%** of the words it uses **[10]+** times,
  and words met [10]+ times across books (as ✔ Mark read)". The incidental and
  lookahead controls are hidden.
- Summary line: the two figures in the chosen objective's order, e.g. "hardest
  start X%, Y unknown words met in all". The easiest-first comparison line stays.
- Columns in Read mode, keeping six figures so the layout code is shared:

  | Column | What it shows |
  |---|---|
  | # | 📖 37% on a started book |
  | Known now | |
  | When reached | first page, or your place |
  | At the start | coloured by difficulty, as "While reading" is now |
  | Over the book | remaining part on a started book |
  | Teaches | hover: lookups per 1,000 words, and learned so far on a started book |
  | From earlier | |

- 💾 saves the words the book will teach ("study now or mine while reading").
- ✔ and ✖ are unchanged.

### `hanstat-cli --plan`

- `--read` selects the mode. `--read-percent` and `--min-count` default to
  `plan.json`'s values.
- Started books are pinned as in the GUI. `--no-pin` turns it off.
- The JSON output gets:
  - per step: `mode`, `from`, `known_at_start`, `known_over_book`,
    `unknown_met` and `teaches`;
  - for the path: `lowest_start` and `total_unknown_met`.

### `examples/plan_eval.rs`

- Adds `Study::Read`: easiest-now vs greedy vs beam, and the every-order check
  on 7-book subsets.
- Compares, on the real Library:
  - positions vs even spread,
  - across-book counting on vs off.

### README and CHANGELOG

- The Reading path paragraph describes Learn while reading first (the default),
  then studying before each book.
- The Read-column paragraph adds the across-books rule.
- The changelog entry at release says:
  - the path defaults to Learn while reading, and Study before each book is
    one click away with the old settings kept;
  - every book is analysed once more for word positions;
  - Mark read now counts words met across books.

## Tests

Unit tests in `plan.rs`, `known.rs` and `profile.rs`:

1. **Repetition beats size.** A long book and a short one have the same
   density of unknown tokens, but the long one's unknown words repeat ≥10
   times. The long one has the higher whole-book coverage and teaches more.
2. **Carry-over.** A book that teaches the next one's frequent words is placed
   before it.
3. **Same as Mark read.** For each step in turn, marking the path's books read
   in its order gives exactly the known words the path predicted after that
   step. This covers both one-book learning (`read_selection` with names and
   known words in the data) and across-book learning.
4. **Across books.** With `M` = 10:
   - 6 + 5 meetings in two books → known;
   - 12 in one book's bottom 10% → not known;
   - 9 in one book → not known;
   - unmarking one of the two books → not known again;
   - Traditional and Simplified spellings sum when folding.
5. **Positions.** Use a hand-made token stream.
   - The sidecar round-trips.
   - "At the start" counts meetings exactly where they occur, so front-loaded
     key words give a higher start than back-loaded ones with the same counts.
   - A missing or mismatched sidecar falls back to even spread, which equals
     the formula used before positions.
   - Words with more than 32 occurrences are spread over the rest of the book.
6. **Started books.**
   - A started book comes first, most recent first.
   - Its read part's words count for later books.
   - Its figures cover the rest.
   - A place under 1% doesn't pin.
   - A hand-set order isn't moved.
7. **Existing modes unchanged.** The current tests pass untouched. `Path::key`
   for Target and Fixed is identical before and after the `Key` change.
8. **Beam vs brute force** for Read, as `beam_against_every_order` and
   `beam_with_series_against_every_valid_order` do for the other modes.
9. **Settings.** A `plan.json` without `mode` or `objective` loads as Read and
   Peak, with its other fields kept. A `known.json` without `across` loads
   unchanged.

Other checks:

- **Place mapping:** an ignored test on a real EPUB (`READER_EPUBS`). Open it in
  the reader at a few places, then check that each maps to the running word
  whose text is on that page.
- **GUI:** a scratch `LOCALAPPDATA` Library with a started book and two read
  books, with screenshots of both modes (see [[gui-testing]] in memory). Never
  touch the real `plan.json`, `known.json` or reading-stats database.

## Order of work

1. **Positions:** analysis, sidecar, background re-analysis and the fallback.
   Useful alone: the path can use them right away.
2. **Across books in Mark read:** read-book copies, `cross_book`, and the dialog.
   Independent of 1.
3. **Read mode in `plan.rs`**, the settings and the tab. It uses 1 and 2 and
   works without them (even spread, no meetings across books).
4. **Started books.**
5. CLI, `plan_eval` measurement, README and changelog.

## Deferred

- **`W` from reading speed.** The opening could be "the first ~2 hours" at the
  measured cpm instead of a fixed 20k words. Decided against for now (fixed
  20k). Revisit only if 20k feels wrong in use.

## Choices (user, 2026-11-24)

| Question | Choice |
|---|---|
| Default mode | **Learn while reading**, for everyone. Existing users switch on update unless they pick Study again; the changelog says so. |
| Opening window `W` | 20,000 running words, fixed, not shown in the UI. |
| Share the rule with Mark read | Yes (one setting, path = what gets recorded). |
| Objective in the UI | **Shown in the tab**, for both modes. |
| Meetings across books | **In the path and in Mark read.** Rule (planned, not asked): two or more read books, `M`+ meetings in all. |
| Word positions | **Now**: a sidecar with the first 32 positions per word; every book analysed once more. |
| The book being read | **Pinned first**, credited with the part read, figures for the rest. |

## Implementation notes (2026-11-24)

- **Positions are by section, not by occurrence.** The analysis cache
  (`analyze::Prepared::stats`) already holds each section's word counts, and
  they sum exactly to the profile's. `profile::Positions` therefore stores, per
  word, `(section, count)` for its first sections until 32 occurrences are
  covered, plus the section table (`href`, first running word, running
  words). Within a section, occurrences are taken as even.
  - **Why:** no new segmentation pass and no change to `analyze.rs` (which is
    in the analysis fingerprint). Re-analysis mostly reads the cache, and reader
    places map straight onto sections.
  - **Cost:** chapter granularity. A 40k-character chapter is one block.
- **Re-analysis trigger:** `profile.rs` is in build.rs `LIBRARY_SOURCES`, so
  every Library row is out of date once and the background update re-runs it
  (cache reads plus discovery). Books without a sidecar are queued even when
  up to date, and **Analyse now** on the path tab does the same.
- **Across books:** `known::across_books` and `known::Settings::across`.
  - `read_books::Across` keeps it current in the background, keyed on the marks,
    `read_min_count` and T/S matching.
  - Read-book copies live in `read/<id>.json.gz` (`profile::read_copy_path_in`).
  - With **skip hapax** on, the path doesn't count a book's hapax towards
    meetings, while Mark read does. They differ only in that corner.
- **Started books:** a place maps to a running word through the reader's own
  spine (`reader::epub::Epub`). The share of the chapter is the offset ÷ the
  reader's own text length for that file (`reader::chapter_text_len`). That
  share then falls in the sidecar's section with the same `href`.
  - The sidecar keeps no character counts. On 9 real EPUBs, the reader's
    chapter text is 9–15% shorter than the analysis section's paragraphs
    (whitespace and markup text are counted differently), so "offset ÷
    section characters" would place a reader 10–15% too early in a chapter.
  - The ignored test `reader_places_map_to_sections` (`READER_EPUBS`) checks
    every spine file with text on those 9 books: 575 chapters, each found as a
    section with identical Chinese text.
  - Places in books without a sidecar don't pin.
  - **Not done:** pinning in `hanstat-cli`, which has no access to the GUI
    crate's reader or reading-stats modules. `--no-pin` wasn't added.
- **`plan_eval`:** instead of switching across-book counting off (there's no
  parameter for it), it reports how many of the words the path teaches come
  only from meetings across books. It also compares positions with even spread.
- **Wording (user, 2026-11-24):** the tab says **Beginning** for "at the
  start". The objectives are **Easiest beginning** and **Fewest unknown
  words** (Learn while reading), **Fewest before any book** and **Fewest in
  all** (Study with a target), and **Easiest hardest book** and **Easiest on
  average** (Study with a fixed number). Each has a plain-language tooltip.
- **Not run:** the `plan_eval` measurements on the real Library. Its books get
  position sidecars only once the new build has run its background update
  there.
