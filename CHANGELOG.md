# Changelog

## [2026.11.28.03] - 2026-11-28

### Changed
- **Smoother, faster blur for DuChinese sentence translations.** Replaced the multi-pass text blur with a cached Gaussian-feathered cloud texture drawn per word chunk, removing visual striping artifacts and cutting rendering cost while keeping the same click-to-reveal behaviour across all light and dark themes.

## [2026.11.28.02] - 2026-11-28

### Changed
- **DuChinese sentence translations are blurred until clicked.** In books imported from DuChinese with **Sentence translations** ticked, each English translation paragraph now appears blurred by default in the Reader so it doesn't spoil the Chinese while you read. Click a blurred translation to reveal it (and click it again to re-blur it; dragging across revealed text still selects and copies normally). Works for both newly imported and previously imported DuChinese stories.

## [2026.11.28.01] - 2026-11-28

### Added
- **Horizontal and vertical text direction in the quote PNG exporter.** The **Export quote** dialog now includes a **Text** direction switcher (**Horizontal** / **Vertical**), defaulting to the Reader's current reading direction until overridden and remembered between exports. Vertical mode sets quotes in traditional top-to-bottom, right-to-left columns using the chosen font's vertical punctuation forms (with upright characters and rotated fallback forms matching the Reader), pairing Square and Horizontal cards with a vertical left colophon and Vertical cards with a centered bookmark colophon.

### Changed
- **Balanced, orphan-free line and column wrapping on quote cards.** Both horizontal and vertical quote layouts now use an optimal line/column breaker that honours CJK kinsoku and Latin word boundaries, keeps lines non-increasing in length (no row or column longer than the one before it), and avoids single-character orphans and punctuation right at the start/top of a wrapped line.

## [2026.11.28.00] - 2026-11-28

### Added
- **Choose theme and fonts in the quote PNG exporter.** The **Export quote** dialog now lets you pick a **Theme** independently of the Reader's theme (defaulting to **Same as pages**) and choose separate **Quote** and **Title** fonts from your installed CJK fonts or the bundled LXGW WenKai (defaulting to **Same as reader** and **LXGW WenKai**, with a **Same as quote** option for the title). Your choices are remembered between exports and previewed with the same offscreen renderer used to save the PNG.

### Fixed
- **Opening Export as PNG… no longer corrupts Reader text.** Previously, opening the **Export quote** dialog replaced egui's font definitions on the live window context, wiping the font atlas and scrambling cached page glyphs behind the dialog. The live preview now renders offscreen without touching the Reader's font atlas.

## [2026.11.27.06] - 2026-11-27

### Added
- **Page width and height in the Reader's Aa menu.** The **Aa** panel now includes **Page width** and **Page height** sliders (with **Reset** back to **Auto**). The default **Auto** behaviour is unchanged, adapting automatically to the window, text size and reading direction, while custom widths and heights clamp to the window and work in single-page, two-page spread and vertical modes.
- **Export selected text as a quote PNG.** Right-clicking selected text in the Reader now offers **Export as PNG…**, opening a live-preview dialog styled in the current Reader theme's colours. Choose between **Square** (1024 × 1024 px), **Horizontal** (1024 × 640 px) and **Vertical** (640 × 1024 px) layouts; the card includes the quote, date, book title (automatically read from the book) and author (read from EPUB `<dc:creator>` metadata, and editable in the dialog).

## [2026.11.27.05] - 2026-11-27

### Added
- **Book cover menu on Home.** Hovering over a book cover on Home (both the **Continue reading** hero cover and every **Recently read** cover) shows a **…** button in its top-right corner with quick actions: open its **Overview**, **Mark read…** (or edit its read mark), **Remove from recently read** (hides it from Home while keeping your saved place and reading stats; opening it again in the Reader brings it back), **Reset reading data…** (clears its reading time, characters and saved page after confirmation), or **Delete from Library** (with the status bar's Undo).

## [2026.11.27.04] - 2026-11-27

### Changed
- **Faster, smoother UI across the Reader, Library, DuChinese and Read aloud.**
  - Spinners during background work (analysis, downloads, Anki loading and writing-style ratings) now animate at 30 FPS instead of running uncapped without vsync, cutting GPU/CPU usage and avoiding contention with local models.
  - The Reader no longer shapes the entire Table of Contents on every frame while the **Contents** menu is closed, virtualizes long TOC lists, caches dictionary popup layouts before building HTML, caches dictionary image indexes, synthesizes TXT chapter markup lazily, skips redundant font-atlas rebuilds when fonts haven't changed, and keeps `goal-sound` decoding and dictionary deletion off the UI thread.
  - **Library › Books**, **Library › Stats**, **Reading path**, **Mark read**, **DuChinese** and **Changelog** now cache per-frame sorting, series grouping, title widths, read-book vocabulary counts, running-total label placement and timezone lookups, decode book and DuChinese covers off the UI thread with bounded texture caches, and run **Save as image…** layout and cover loading in the background.
  - Marking words known (**A** / **+**) updates your merged known words incrementally instead of rebuilding every word list from scratch.
  - **Read aloud** and **Export audiobook** skip full-book billing work in Local mode, compute Azure book estimates off the UI thread, virtualize chapter lists, skip empty audio caches when scanning audiobook coverage, cache cloned voice references and server registrations, and move preview/voice-designer audio and file operations off the UI thread.
  - Startup memory-maps read-only system fonts instead of copying them into heap RAM, and EPUB extraction, segmentation, word discovery, Anki coverage and the Reading path planner do less redundant work and parallelize more passes.

### Fixed
- **Broken images in the Reader no longer spin at 20 FPS while idle.** Failed images stop requesting repaints, while zoomed images repaint as soon as their sharper background decode finishes.
- **Subscript (`<sub>`) text in the Reader is now positioned below the baseline**, and errors opening Anki's card browser from the dictionary popup are now shown to the user.

## [2026.11.27.03] - 2026-11-27

### Added
- **Prompt before replacing a book with the same title.** Opening or dragging in a file whose title matches a different book already in the Library now asks whether to **Replace** it, **Keep both** or **Cancel** (**Skip** when opening several files at once, with an option to apply the choice to the remaining matches). Replacing updates the Library copy, cover and statistics once the new file finishes analysing, while keeping your reading stats, where you left off, read status, custom columns and reading-path placement.

## [2026.11.27.02] - 2026-11-27

### Added
- **Every column sorts in Characters, Words, Sections and Suggested words.** **Characters** and **Words** now also sort by **Count**, **%**, **Cumulative %**, and (for Characters) **Code point**. **Sections** now sorts by every column — included (**✓**), **#**, **Section**, **Kind**, **Chars**, **Unique**, **New**, **Words**, **Unique w.** and **New w.** — keeping excluded sections (`–`) at the bottom when sorting by **New** or **New w.** **Suggested words** now sorts by every column — ticked (**✓**), **Word**, **Status**, **Occurrences**, **Wrong**, **Dependency**, **Boundary freedom** and **Note** — and your sort choices are remembered between runs.

## [2026.11.27.01] - 2026-11-27

### Changed
- **Upright numbers and Latin letters in vertical mode.** Digits and Latin letters now stack upright top to bottom along the column instead of rotating sideways, with half-em word spaces and vertical forms for ASCII brackets.

## [2026.11.27.00] - 2026-11-27

### Added
- **Vertical reading mode.** The Reader's **Aa** panel has a **Text direction** setting (**Horizontal** / **Vertical**). Choosing **Vertical** lays the book out in columns running top to bottom and right to left on a single landscape page, with vertical punctuation forms (and rotated Latin letters and numbers), ruby annotations beside the column, and right-to-left page turning (**←** or clicking left of the page goes forward; **→** or clicking right goes back). Text selection, search, read-aloud highlighting and the dictionary popup (placed beside the column) all work in vertical mode, and switching back to **Horizontal** restores your one- or two-page choice.

## [2026.11.26.06] - 2026-11-26

### Added
- **Reset all reading stats.** **Settings › Reading stats** has **Reset all reading stats…**. It first shows how many books, hours and characters will go, then removes every book's reading time, characters, days and sessions, and with them the streaks and the speed measured in the Reader. Where you left each book, your daily goals, these settings and the books marked read are kept. A copy of the stats as they were is saved first in hanstat's folder as `reading-stats-before-reset-<date>.sqlite`. If a book is open in the Reader, its timing starts again from the reset.

### Changed
- **New icons.** The emoji and symbols used as icons are now Phosphor icons everywhere: the toolbar and status bar, Home, the Library, the Overview, Settings, DuChinese, the Reader and its dictionary popup, and the saved Overview image. They look the same on Windows, macOS and Linux, and the Settings gear in the status bar is larger.

### Fixed
- **Writing style can be reached in Settings.** With **Keyboard shortcuts** open, the Settings window grew taller than the screen, and only the shortcuts list scrolled. The whole window now scrolls, and the shortcuts list no longer has a scroll box of its own.
- **Letters glued to Chinese characters no longer count as words.** When the segmenter joined Latin letters or digits to Chinese characters (Myself嗯, 地雷x1, 0点), the whole thing was counted as one Chinese word. Such tokens are now cut where the script changes, so only the Chinese part counts. Dictionary words that mix scripts, such as T恤, AA制, B超 and 4S店, stay whole, and so does a word in your user dictionary (for example B站). Every book is analysed again once after updating. On four test novels this removed between 4 and 1,175 such non-words per book, and changed nothing else.

## [2026.11.26.05] - 2026-11-26

### Added
- **TXT books open in the Reader.** **📕 Read** now shows for TXT books too: on the Overview, in the Library, on Home and as the toolbar's **📕 Reader**. Each chapter found by its heading (第1章, 楔子, 番外 …) becomes its own pages, with the heading centred above it, the paragraphs indented, and every heading listed in **Contents**. The text is read with the same encoding detection and chapter splitting as the analysis, so the Reader counts the same characters as the book's statistics. Search, the dictionary popup, Anki cards, Read aloud, audiobook export and reading stats all work as with an EPUB. A very long chapter, or a book with no headings, is split into parts behind the scenes so pages still open quickly. The Reader always detects the encoding itself, even if you chose one by hand for the analysis, and **Learn while reading** does not yet follow your place in a TXT book.

## [2026.11.26.04] - 2026-11-26

### Fixed
- **Back from a search result returns to your reading place.** After **Back to where you were** (or Esc), the Reader showed your page but still remembered the search result's position, so resizing the window, changing a setting in **Aa**, or closing and reopening the book took you to the wrong place. It now returns to exactly where you were reading.

## [2026.11.26.03] - 2026-11-26

### Fixed
- **Search works with the hidden menu bar.** With **Menu bar: Hide until the pointer is at the top** on, the **Back to where you were** bar sat at the top of the window, so reaching for it brought the menu bar out over it. It now sits over the line under the page, and the search sidebar starts below the strip at the top, so neither is covered.

## [2026.11.26.02] - 2026-11-26

### Added
- **Anki cards get audio.** The **+** in the dictionary popup now adds the word and its sentence spoken by the voice chosen in **Read aloud** (Azure or the voice on your computer). In the card setup, `{audio}` is the word and the new `{sentence-audio}` is the sentence; Lapis's **SentenceAudio** field gets `{sentence-audio}` by itself. A sentence already read aloud uses the audio made then. The audio is made in the background, so the popup and the pages don't wait for it. With no voice set up the card is still added, without audio, and its ✔ turns yellow with the reason in its tooltip.
- **Mark words known from the dictionary popup.** Each entry has a ✓ beside the **+**: click it (or press **A** for the first entry) to mark the word known, and again to unmark it. Words that Anki or your word lists already count as known show a blue ✓ that names where they are known from.
- **Search the book with Ctrl+F.** A sidebar opens over the left of the pages with a search box and every match, each with its sentence and chapter. Traditional and Simplified characters find each other, and a match can run across a line break. Click a result (or press Enter for the next one) to open its page with the match highlighted. Your reading place, reading time and stats stay where they were: **Back to where you were** (or Esc) returns you there, and **Keep reading here** makes the result your reading place instead. The sidebar hides when you open a result; Ctrl+F or 🔍 brings it back with the same results.
- **Read aloud controls.** While reading aloud, the bar has buttons to go back a sentence, pause or resume, stop, and go forward a sentence (Ctrl+←, K and Ctrl+→), and a **playback speed** slider from 0.5× to 2× (double-click for normal speed). The speed works for every voice, changes at once on the sentence you're hearing, and needs no new audio. Azure's own speed setting is now called **Voice speed**: it makes the audio again.

## [2026.11.26.01] - 2026-11-26

### Added
- **The Reader's menu bar can hide.** In **Aa**, tick **Menu bar: Hide until the pointer is at the top**. The bar stays out of sight and appears over the pages when you move the pointer to the top of the window. It stays while one of its panels or windows (Aa, Stats, Contents, Read aloud, Dictionaries) is open, and shows for a few seconds when a new notice arrives. The pages are laid out for the whole window and don't move or re-flow when the bar shows or hides; while shown, it covers the top edge of the pages. Dragging a text selection up to the top doesn't bring it out.

## [2026.11.26.00] - 2026-11-26

### Fixed
- **Read aloud turns the page in the middle of a sentence.** When a sentence runs on to the next page, the page now turns as the voice reaches that page's first character, instead of after the whole sentence has been read. This works with both the Azure and the local voice. The timing is estimated from how far the voice is into the sentence's audio, so the turn can come slightly early or late if there is a long pause at a comma just before the page break.

## [2026.11.25.11] - 2026-11-25

### Changed
- **The Reader opens faster and stays responsive while laying out chapters.** Font discovery starts after the first app frame, dictionaries stay loaded when you switch books, and chapter layout can continue over several frames.
- **Reopening an EPUB's Overview is faster.** Once an EPUB has been opened, hanstat can reuse its extracted text while the file's contents match. Report construction and the Library's word-profile work run in the background.
- **Reading Path is faster for large libraries.** Candidate books and series are scored in parallel. The same inputs produce the same reading order.

## [2026.11.25.10] - 2026-11-25

### Fixed
- **Writing-style rating no longer crashes hanstat on computers with less memory.** The model's server kept a cache of every sample it had read, which grew past 9 GB within seconds and ran a 16 GB laptop out of memory; hanstat then closed, and did so again on every start. The cache is off now (it was never used again) and the server stays near 4 GB, with the same ratings. If the computer is short of free memory, rating waits for it (shown in the status bar) instead of starting, and stops if memory runs low while rating. If hanstat closes while the model is loaded, rating is **paused** at the next start until you click **Rate the books without a rating** in Settings (⚙), so a crash can't repeat on every start.

### Changed
- **The model fits your graphics card.** As many of its layers go on the card as its free memory holds, keeping 1 GB for hanstat and the desktop, and the rest run on the CPU: a card with 4 GB or more runs it all, as before; a smaller one (3 GB) now uses what it has instead of failing. Settings shows the split, e.g. “33 of 33 layers on NVIDIA GeForce RTX 5070”. The ratings are the same either way; only the speed differs.

## [2026.11.25.09] - 2026-11-25

### Fixed
- **Series are recognised in more titles, in the right order.** In the Reading path, “Thirty-Six Stratagems 1: …” to “6: …” are now one series in volume order (before, the “Six” of the name was taken as volume 6 for all of them, so they could be read in any order). “Journey to the West 3: Havoc in Heaven (Part 1)” and “Three Kingdoms 7: The Battle of Red Cliffs (Part One)” now join their series as parts of volume 3 and 7. A **Prologue** (also 序章, 楔子, 前传) comes before volume 1, and an **Epilogue** (also 尾声) after the volume it follows (“Three Kingdoms 3 Epilogue” between 3 and 4). A number word in a subtitle (“I’m a Cat 4: Three Cakes”) is no longer taken as a part.

## [2026.11.25.08] - 2026-11-25

### Added
- **Your own voice and message for the daily goal.** Under **Stats › Settings** in the reader, the goal sound has a **Voice** list: the built-in cheer, your designed voices and the local voice's speakers (once downloaded under Read aloud). Pick one, type what it should say (up to 20 characters, so it stays under about 5 seconds), and press **Make**: the voice says it once and it plays from then on. **▶ Play** lets you hear it. Until it is made, or after you change the voice or the message, the built-in cheer plays.

## [2026.11.25.07] - 2026-11-25

### Added
- **A voice cheers when you reach your daily goal.** Along with the glow, a voice says “宝贝！你好厉害哦！你完成今天的目标啦！” when today's goal is met while you read. If read aloud is playing, it pauses for the cheer and goes on after; pausing or stopping it yourself meanwhile leaves it so. Turn the sound off under **Stats › Settings** in the reader, apart from the glow.

## [2026.11.25.06] - 2026-11-25

### Added
- **A glow when you reach your daily goal.** When today's reading goal is met while you read, a rainbow glow runs round the edge of the window for 5 seconds. It shows once a day: not again for paging back under the goal and on, and not when you open a book with the goal already met. A minutes goal shows it as the time is up, without waiting for a page turn. Turn it off under **Stats › Settings** in the reader.
- **Listening can count toward the daily goal.** Settings (⚙) has a new **Daily reading goal** option, **Count listening (read aloud) as reading time**. With it on, time and characters while read aloud plays count toward the goal and its streak, in the reader, on Home and in Library Stats. Reading speed and days read still leave listening out. Off by default.

## [2026.11.25.05] - 2026-11-25

### Added
- **Popup theme.** The reader's **Aa** panel has a new setting under Theme: the dictionary popup can follow the pages' theme (**Same as pages**) or use any of the reader's themes, light or dark, whatever the pages use. It changes an open popup at once, child popups included. Everything in the popup takes the theme's colours: the headword, readings and glossaries, links and tags, frequency ranks, the Anki **+** button, the dictionaries' bars and the scroll bars. Black-and-white glyph images in a dictionary are shown light only in a dark popup. If you already used the reader, your popup stays Catppuccin Mocha, as it was, until you choose another; new installs start with Same as pages.

### Fixed
- **The popup's scroll bar no longer covers the Anki + buttons.** Entries now leave room at their right for the scroll bar, even when it widens under the pointer.

## [2026.11.25.04] - 2026-11-25

### Changed
- **Pages turn only from beside them.** A click on a page no longer turns it; click to the left of the pages to go back and to the right to go forward. Clicks on the text (links, read-aloud jumps, closing popups) work as before, and the keys and mouse wheel still turn pages.
- **The Today ring changes colour as you read.** On Home it starts red, turns orange and then yellow, and is green once the day's goal is met.

## [2026.11.25.03] - 2026-11-25

### Added
- **A Books strip under "Characters read over time".** Under the chart's dates, each book is a bar from the first to the last day you read it. Books read at the same time sit on rows of their own, so reading several at once shows at a glance; books read one after another share a row. Hovering a bar highlights that book on the curve (and hovering a book on the curve highlights its bar); clicking a bar opens the book. Books whose label didn't fit on the chart still get a bar.

## [2026.11.25.02] - 2026-11-25

### Added
- **Characters read over time.** Library Stats › Reading has a new chart: your running total of Chinese characters read, added up day by day, with each book labelled on the curve where you had read half of it. Hover over the curve for the total on a day and what you read that day; click a book's label (or its dot) to open the book. It follows the All time / Year / Month / Week picker, and its dates run by day, week or month depending on how much reading there is to show.
- **Start and finish dates in Mark read.** When marking a book read you can now pick when you read it (both optional, with the same calendar as the Library's date columns). The new chart spreads that book's characters evenly over those days, less whatever the reader already timed, so books read on paper or in another app count too. A start date alone runs to the day you marked it read; a finish date alone counts as one day. Only the chart uses these dates: time, speed and streaks stay what the reader measured.

## [2026.11.25.01] - 2026-11-25

### Added
- **Hover text for the reader's unlabelled parts.** The thin bar along the foot of the pages is your daily reading goal; hovering over it now shows how far you are (e.g. "12 of 30 min, 1,200 of 5,000 characters") and, with both a minutes and a characters goal, that it follows whichever is further behind. The page number explains that it counts pages in the chapter, and **Aa** says what its panel holds.
- **Hover text for Home's Today ring.** It explains that the number is minutes read today across all books and how much of today's goal the ring shows.

### Changed
- **The toolbar's buttons are in a new order.** Where you go comes first: 🏠 Home, 📖 Library, 📕 Reader, then 📂 Open…, then the book's settings as before (Segmenter, forced words, Anki, Encoding for TXT books, Known). 💾 Export now sits at the far right. In a narrow window the buttons wrap onto a second line and Export stays at the top right.

## [2026.11.25.00] - 2026-11-25

### Fixed
- **Large Anki collections open on Linux.** hanstat reads a private copy of the collection, and that copy went into the temp folder, which on many Linux systems (Fedora, Arch, Debian 13 and others) lives in RAM and holds at most half of it. A collection of several hundred MB could fail with "No space left on device" even with plenty of disk space. The copy now goes into hanstat's cache folder on disk (`~/.cache/hanstat/anki-copy` on Linux, hanstat's own folder on Windows and macOS).
- **Clearer message when there isn't room for the copy.** hanstat checks the free space first and says how much the copy needs and how much is free. A copy that fails partway now says it could not be copied, instead of that the collection could not be read.
- **Copies left behind are removed.** If hanstat closed unexpectedly, its copy of the collection stayed behind for up to a day and several could pile up. Now copies no running hanstat is using are removed (all but the one kept for the collection you use), as are copies left in the temp folder by earlier versions.

### Changed
- **The Anki collection is no longer copied in full each time.** hanstat keeps its copy between runs and, when the collection has changed, writes only the parts Anki changed; when it hasn't changed (no reviews since last time), the copy is used as it is. This uses as much disk space as the collection for as long as Anki statistics are on; turning them off removes the copy when hanstat next starts.
- **Known-word shares in the Library are refreshed once** after this update, from the saved word data (no new analysis).

## [2026.11.24.08] - 2026-11-24

### Added
- **Choose the Library's columns.** **☷ Columns…** on Library › Books lets you reorder the columns (drag, or ▲▼), hide the default ones, and add more: Progress, Series, Unique characters, Words for 98%, Known unique words, Unknown words, Known unique characters, Time read, Reading speed, First read, Last read, Finished, Segmenter, Format and Sections. **Reset to default** puts them back.
- **Your own columns.** Add columns of text, numbers, yes/no, a ★ rating or a date, and fill them in right in the table. A deleted column can be put back for a few seconds. The search box also finds books by your text columns, e.g. an Author column.
- **Filter by any column.** Every column has a filter (⏷ funnel by its name): a range, a list to tick, dates, or text. Active filters show above the table and are kept when hanstat closes. **Hide read** is now the Read column's filter ("Not read"), and if it was on, it is set for you.
- **Columns that don't fit go to further pages**, with ◀ ▶. The book, 📕 Read, ✔ and ✖ stay on every page.
- **Progress column.** How far through each book you are in the reader (100% once finished or marked read).
- **Series from the book's metadata.** EPUBs that name their series as calibre writes it (calibre:series, or EPUB 3 collections of type series) are grouped by that, with its volume number, on the reading path and in the Series column; other books still by their title.

### Changed
- **Each chapter's length is saved**, so Home shows how far you are without working it out again each time hanstat starts.
- **The Library is analysed again once after this update** (a new component for the date picker); it happens in the background as after other updates.

## [2026.11.24.07] - 2026-11-24

### Fixed
- **The Mark read window no longer freezes while you change its rule.** "With the N other books read: … more words met 10+ times across books" was worked out again for every step while dragging the % or the times, and with many books marked read each took a noticeable moment, so the window stuttered. It is now worked out in the background: the last figure stays shown, with a spinner, until the new one is ready.
- **Changing "times" on the Reading path no longer piles up work.** The words met across books were worked out again for every step of a drag, each in its own background task, and all of them ran to the end. Now one runs at a time, and once it's done the next is for the value you ended on.

## [2026.11.24.06] - 2026-11-24

### Fixed
- **The reading path no longer piles up work while the Library updates.** After an update, the Library analyses its books again in the background, and each book it finished started the reading path over from scratch, reading every book's word data again, while the earlier attempts kept going. With **Learn while reading** this could keep the path planning for minutes, slow the computer down and use enough memory for hanstat to close. Now only one plan runs at a time, a plan that is no longer needed stops at once, and while books are being analysed the path is planned again at most every 10 seconds (and right away once they're done). Switching between **Learn while reading** and **Study before each book** is quicker too.
- **Only the books that changed are read again.** When the reading path is planned again, books whose word data hasn't changed are reused instead of read from disk. The path is the same as before.
- **Learn while reading uses where words occur after visiting Library Stats.** Opening **Stats** before **Reading path** could leave the path estimating where every book's words occur ("N books use an estimate…") until hanstat was restarted.

## [2026.11.24.05] - 2026-11-24

### Added
- **Home: hanstat now opens on a start page** instead of an empty window. It shows the book you're reading with a large cover, its chapter, how far through you are, the time left at your reading speed and a **Continue reading** button. Next to it, **Today** has a ring that fills towards your daily goal, today's characters and your reading streak; the goal can be set or changed there (**Set a goal…**). Below are your **Recently read** books (click a cover to read it; **Overview** on the cover opens its figures), this year's reading as a calendar a small square a day, and your totals: books finished, time and characters read, and words known.
- **🏠 Home** in the toolbar (Ctrl+H) goes back to it from a book or the Library. On Home, the toolbar leaves out what only applies to an open book.
- **A welcome page** when the Library is empty: a place to drop books, and the three steps to get started.
- **Sharper covers.** Books get a larger cover for Home: new books when they are analysed, books already in the Library the first time Home shows them.

## [2026.11.24.04] - 2026-11-24

### Fixed
- **Chinese text in the reader takes the theme's colour with SimSun and MingLiU.** On a PC without Source Han Serif or Noto Serif, the reader's serif is SimSun (PMingLiU for Traditional), and its Chinese characters were drawn near-white on every theme, almost invisible on light pages; Latin letters and digits were fine. Fonts with built-in small-size bitmaps (SimSun, NSimSun, MingLiU, PMingLiU, MS Mincho, MS Gothic, Gulim, Batang) are now drawn from their outlines, in the theme's colour. Rare characters drawn from SimSun-ExtB anywhere in hanstat are fixed too. Other fonts are unchanged.

## [2026.11.24.03] - 2026-11-24

### Added
- **Keep series together** on **Library → Reading path**. Ticked, the books of a series are read one after another, volume 1, 2, 3…, with no other book between them, in both **Learn while reading** and **Study before each book** and whichever **Best order** you choose. The whole series goes where it is best on the path, counting what its earlier volumes teach for the later ones. A book you're reading still comes first, followed by the rest of its series. An order you set by hand is kept until you click **Use the planner's order**; a volume added since goes after the rest of its series. Unticked (the default) works as before: volumes are read in order, but other books can come between them.
- **hanstat-cli --plan --keep-series** does the same on the command line.

### Fixed
- **Frequency ranks no longer squeeze the popup.** With several frequency lists, a rank that didn't fit at the end of the popup's first line was squashed into the space left, one letter per line, and made the header so tall that the definitions were pushed out of sight. A rank that doesn't fit now starts the next line.

## [2026.11.24.02] - 2026-11-24

### Added
- **Learn while reading: a second Reading path.** **Library → Reading path** now starts in **Learn while reading**: nothing is studied before a book. Each book teaches the words it uses often (90% of those it uses 10+ times, the rule ✔ Mark read uses), and the books are ordered so that each starts as easy as it can. A book's **Beginning** is how much of its first few chapters (20,000 running words) you'd know, counting the words it teaches as you meet them; **Over the book** is the same for the whole book, so a long book that repeats its words is easier than its word count suggests. **Teaches** says how many words it teaches, and its hover how many unknown words you'd meet (per 1,000 running words). **💾** saves the words it teaches.
- **Words met across books count.** A word no single book teaches is learned once it occurs in two or more books, 10+ times in all (the same 10 as above). The reading path counts it, and so does ✔ Mark read: the Mark read window says how many more words a mark adds this way, Library Stats › Books read shows them as **Across books**, and they count as known everywhere. A book marked read keeps its word counts from then on (in `read` in the app folder), so analysing it again or removing it from the Library doesn't change them. Books marked read before this version count once their word data is copied, the first time it starts; a book already gone from the Library still counts its own words, but not towards words across books.
- **The book you're reading comes first.** With Learn while reading, a book you have read past its first 1% of in the reader (📖 37%) is first on the path, most recently read first, and its figures are for the rest of it. An order you set by hand is kept.
- **Best order** chooses what the path makes best. In Learn while reading: **Easiest beginning** (every book as easy to get into as it can be) or **Fewest unknown words** (over all the books, which can put a long book early when the words it teaches make the books after it easier). In Study before each book: **Fewest before any book** (as before) or **Fewest in all**, and with a fixed number of words, **Easiest hardest book** (as before) or **Easiest on average**.
- **hanstat-cli --plan --read** plans learning while reading (`--read-percent`, `--min-count`; defaults from the window); `--objective` is listed in the help. With `--json` each step has `known_at_start`, `known_over_book`, `unknown_met` and `teaches`.

### Changed
- **The Reading path starts in Learn while reading**, including for paths planned before. **Study before each book** is one click away, with your target, number of words and other settings as you left them.
- **Every Library book is analysed once more** in the background the first time this version starts, to keep where its words occur (by chapter) for **Beginning**. Saved analyses make this quick for most books; until a book is done, its figures use an even spread of its words ("an estimate" in the hover).
- **The ✔ Mark read rule and the Reading path's are one setting.** Changing the share or the number of times in either changes both.

## [2026.11.24.01] - 2026-11-24

### Removed
- **Japanese and Korean books are no longer supported.** hanstat is for Chinese books only: the Kiwi and Sudachi analysers, the JLPT, Kaishi 1.5k and Korean 1–6급 word lists, and the Japanese and Korean reading speeds are gone. Every book is analysed as Chinese.
- **The language badge** at the top right of the toolbar is gone, and with it the language choice in the Reading path and Library Stats. The Overview still names the book's script, and the Library's Language column still shows 简体中文 or 繁體中文.
- **Korean and Japanese books leave the Library** the first time this version starts. Their settings files (`anki-ko.json`, `known-ja.json` and so on) and downloaded analysers are left on disk, unused; delete them by hand if you like.
- **hanstat-cli** no longer takes `--lang` or `--kaishi`; `--level N` and `--hsk N` are the same option. TXT files can no longer be read as EUC-KR.

## [2026.11.24.00] - 2026-11-24

### Added
- **📕 Read on each Library book** opens it in the reader straight from the Library, where you left it (Chinese EPUBs, as on the Overview).
- **📕 Reader** next to **Library** goes back to the Library book you last read in the reader, where you left it.

### Changed
- **The Library keeps its own copy of each book.** A book you open or add is copied into the `books` folder in the app folder, and the Library, the reader and everything else read that copy, so the file you added it from can be moved, renamed or deleted. Each book has a folder there with the file under its own name; the same book added twice is stored once. Its reading stats, place, cover, read mark and writing-style rating are the copy's, unchanged.
- **Books already in the Library are copied over once**, in the background, the first time this version starts ("Copying books into the Library" in the status bar); the originals are not touched. A book whose file is gone, or has changed since it was analysed, stays as it was. A book that cannot be copied (a full disk) is read where it is and copied next time.
- **Removing a book from the Library deletes hanstat's copy** once it can no longer be put back; the file you added it from is not touched.
- **Opening a file you edited since adding it adds it as a new book**, instead of replacing the old one, since the Library's copy is the old file.

## [2026.11.23.04] - 2026-11-23

### Changed
- **Suggested words lists only your 5 newest forced words.** **… N more** shows the rest, and **Show fewer** hides them again. The **Forced words** window still lists them all.

## [2026.11.23.03] - 2026-11-23

### Added
- **DuChinese stories as Library books.** **Library › Add from DuChinese…** lists DuChinese's graded stories and courses, with their covers, levels and chapters. Filter by stories or courses and by level, search them, and hide the ones already imported.
- **How much of each story you'd know.** Each story shows the share of running words you know, worked out from its first chapter with your own segmenter and known words, the way the Library counts them. Sort by **Best fit for you** to see the easiest first. The first time, each first chapter is fetched once, which takes a few minutes for the whole list; after that it is instant. Imported stories show the figure for the whole book.
- **Import** fetches a story's chapters and saves it as an EPUB, which is analysed into the Library and opens in the reader like any other book. The chapters are titled with their own Chinese headings. Choose **Simplified** or **Traditional** (DuChinese's own Traditional text, not a conversion), and optionally **Sentence translations**: each paragraph followed by DuChinese's English in small print. Books go to `DuChinese books` in the app folder; the 📂 button chooses another folder.
- **Premium chapters.** **Sign in…** signs in to DuChinese. Your password goes to duchinese.net only and isn't saved. The session it hands back is kept encrypted for your Windows account (in the Keychain on macOS, the keyring on Linux). If you signed up with Apple or Google, you can paste the session from a browser signed in to duchinese.net instead. Signed out, a story with locked chapters is imported with its free ones and marked as such ("1 of 22 · 21 locked"). Signing in later with a subscription adds the rest.
- **New chapters are added as they come out.** Imported stories still being released are checked twice a day (and when the window opens), and new chapters are added to the book. **Check for new chapters** in the window, or **Update** on a story, asks right away, and also picks up chapters DuChinese has edited. Turn off **Add new chapters on their own** to check only by hand.
- **The Library row** of an imported story says where it stands: "DuChinese · 6 chapters", "4 of 10 · ongoing", and "2 new" for a few days after chapters arrive.
- **In the reader**, a story that gets new chapters while it is open says "1 new chapter is out" with **Reload**, which closes the book, adds them and opens it again where you were; otherwise they are added once you close it. On the last chapter of a story still coming out, it says how many more are announced.

### Changed
- **An updated story stays the same book.** Its file changes when chapters are added, but its reading stats, where you left it (in the same chapter), its read mark, its place on the reading path and its writing-style rating all carry over. A read mark keeps the words it counted when you marked the book: new chapters don't add to your known words.

## [2026.11.23.02] - 2026-11-23

### Added
- **Reading stats in the EPUB reader.**
  - The reader times your reading and counts the Chinese characters you page past. Paging back takes them off again, and jumps through the contents or links don't count.
  - It pauses on its own after 5 minutes without input, leaving that idle time out. It also pauses when the window is in the background or a menu is open. **P**, or a click on the session time, pauses and starts it by hand.
  - Under **Stats** in the reader's bar you can choose to start timing only by hand.
  - While **Read aloud** plays, the time counts as listening, kept apart from reading.
- **A line under the pages** shows the session's time, characters and speed, and the time left in the chapter and the book. You can hide it.
- **The Stats panel** in the reader shows this session, today, this book and all books, your streak, and a daily goal in minutes and/or characters. **Discard this session…** takes a session back out.
- **Paging past a book's last page** records it as finished and offers **Mark read**.
- **Library Stats › Reading.** Totals, speed, streaks and books finished, for all time or a year, month, week or day, with ‹ › to step through them.
  - The calendar shows minutes read each day, with goal days ringed and its own year ‹ ›.
  - Charts show time or characters per day or month, and speed per week or day.
  - The table of books read can be filtered the same way; right-click a title to remove its stats.
- **Read so far** on each book's Overview: time, characters, speed, start and finish dates, and the time left at your speed.

### Changed
- **Chinese reading times use your own speed.** Once you've read 10 minutes in the reader, reading times in the Library, on the Overview and in the reader use your speed over the last 30 days. To use a fixed speed instead, turn on **Settings (⚙) › Reading speed › Chinese › Override**.
- **Where you left each book** is kept in the reading-stats database (`reading-stats.sqlite`). Places saved before are still used.
- The first start after updating refreshes each Library row's known-word shares from its saved data. No book is analysed again.

### Fixed
- **Settings and known words could be lost.** If settings, the reading path, Anki, writing-style, known-word or reader settings files couldn't be read, hanstat used to start from defaults and overwrite the file at the next save. Such a file is now kept as `<name>.unreadable-<time>`, and hanstat says so.
- **Saved files survive a power cut.** Every file hanstat saves is now flushed to the disk before it replaces the old one. The settings, reading-path and reader settings files were written in place before.

## [2026.11.23.01] - 2026-11-23

### Added
- **Colour themes for the EPUB reader.** Choose one under **Aa → Theme**: six light themes (Solarized Light, Ecru, Paper, Catppuccin Latte, Rosé Pine Dawn, Gruvbox Light) and ten dark ones (Solarized Dark, Catppuccin Frappé, Macchiato and Mocha, Rosé Pine, Rosé Pine Moon, Gruvbox Dark, Nord, Tokyo Night, Black). The list shows each theme in its own colours. The theme colours the pages, the reader's bar and menus, and the Dictionaries, Anki and Read aloud windows, and is remembered for every book. Each theme uses its palette's own colours, unchanged.
- **Books stay readable on dark pages.** On a dark theme, text a book sets in a dark colour (dark grey, navy) is shown light, and light boxes behind text turn dark. Images are shown as they are.

### Changed
- **Solarized Light uses Solarized's own text colour.** Body text is Solarized's base00, a little lighter than the darker ink the reader used before.
- **Links take the theme's link colour**, in the dictionary popup too, instead of always Solarized blue.

## [2026.11.23.00] - 2026-11-23

### Changed
- **A new look.** The window uses the [Catppuccin](https://catppuccin.com) colours: Mocha on the dark theme and Latte on the light one, following the system's setting as before. Figures sit on raised cards, the current tab is underlined, the toolbar is more compact and fits on one line at the usual window size, and word lengths are shown as bars. On the light theme, coloured text (difficulty, Anki status, warnings) is darkened enough to read easily. Exported images use the same colours.
- **The window opens at three quarters of the screen, centred.** If you resize it, hanstat keeps that size for next time, and a window closed maximised opens maximised. Where it was on the screen is no longer kept: it always opens centred. The window no longer grows to fit the Overview.
- **Clearer type.** On Windows, interface text is set in Segoe UI with Microsoft YaHei, the pair Windows itself uses, and headings and figures in their semibold. On macOS and Linux, hanstat bundles Inter for Latin text and its semibold for headings (SIL Open Font License; see `LICENSE-Inter-font.txt`).

## [2026.11.21.00] - 2026-11-21

### Fixed
- **The local voice fits graphics cards with less memory.** Reading four pieces of text at once (the default) needs about 6.2 GB of graphics memory, so on a 4 GB card such as an RTX 3050 Laptop the voice stopped while loading with `ErrorOutOfDeviceMemory`. hanstat now checks how much memory the graphics card has and reads fewer pieces at once when four won't fit: one on 4 GB, three on 6 GB, four on 8 GB and more. If the card still runs out, for example because another program holds part of its memory, the voice starts again with one fewer, and the sentences that failed are read again. If even one doesn't fit, what doesn't fit goes in the computer's memory, which is slower, and the panel says so. If that fails too, the message says how much memory the voice needs and what to close.
- **See before downloading whether the card is big enough.** Under **Read aloud → This computer**, a note now shows the graphics card's name and memory when it is small for the local voice, and **Advanced → At once** shows how many pieces this card reads at once. The **Unload** tooltip gives the memory actually held.

## [2026.11.20.01] - 2026-11-20

### Added
- **Import and export designed voices.** In **Design a voice**, choose **Import voice…** beside **Kept voices**, or **Export…** next to a saved voice. A `.voice` file keeps the exact saved name, original recording, and transcript. Imported voices appear in Preview and Generate locally. Importing a voice you already have preserves its local name.

## [2026.11.20.00] - 2026-11-20

### Added
- **Select and copy EPUB text.** Drag across text, including across both visible pages, then choose **Copy** from the right-click menu or press Ctrl+C. Copied text keeps paragraph breaks and authored line breaks.
- **Play from here.** Right-click a sentence to start reading aloud from it with the current TTS settings. A click on ordinary text clears the selection while TTS is stopped. During playback, clicking text still jumps to its sentence.

## [2026.11.19.06] - 2026-11-19

### Added
- **Export audiobook.** Choose **Export audiobook…** beside **Read** on a Chinese EPUB’s Overview. Select chapters and save a chaptered M4B or Opus file with optional sentence-timed SRT subtitles. Both formats embed the EPUB cover when available, scaling its longest edge down to 2,048 pixels if needed.
- **Reuse saved speech.** The exporter picks the voice with the most complete selected chapters and generates any missing audio in that voice. With an empty cache, choose an Azure voice, a local preset or a saved local voice. **Open in Reader** uses the export voice for the session and plays saved audio with sentence highlighting, including offline playback.
- **FFmpeg setup.** The exporter uses FFmpeg on PATH when available. On Windows, the dialog can download a checksum-verified copy into hanstat’s app data. On macOS and Linux, put your installed FFmpeg on hanstat’s PATH.

## [2026.11.19.05] - 2026-11-19

### Added
- **Design a voice for reading aloud.** Under **Read aloud → This computer**, choose **Design a voice…** below **Generate locally…**, describe the voice and make a sample. Start with neutral narration or use sentences from the page shown. Play the sample, make another take, then keep it under a name. Kept voices are available in Preview and Generate locally, with controls to play their samples, rename them and confirm deletion.
- **Keep the same voice across books.** Qwen3-TTS VoiceDesign makes the sample once, and Qwen3-TTS Base clones it to read. The sample sets the voice's tone and pace. Its WAV hash identifies its saved audio, so another voice gets its own clips. Samples live in the user's `voices` folder and survive clearing Saved audio.
- **Download the voice models when asked.** The Design window's button downloads the pinned VoiceDesign and Base Q8_0 models, about 4.1 GB, plus any missing server and shared tokenizer. It greys out during downloading and after completion. The three model modes share one guarded server process, switching when needed. Designed voices buffer a few seconds at startup to keep short chapter headings flowing into the narration.

### Fixed
- **The local voice buttons sit directly together.** **Design a voice…** is immediately below **Generate locally…**, with the usual spacing between buttons.

## [2026.11.19.04] - 2026-11-19

### Changed
- **The voice on this computer reads on from one chunk to the next.** Each chunk of paragraphs used to be read as if the book started there, and it began only about 0.1 s after the last one ended, against about 0.7 s between sentences inside a chunk, so every new chunk sounded abrupt. Now the model first reads the end of the sentence before (up to 60 characters), so the chunk's first sentence comes out as a continuation; that part is cut away, keeping the pause after it (now 0.55–0.9 s where chunks meet). A speech recogniser heard nothing of the cut-away part in the test chapter. Making audio takes about 15% longer. Audio made before keeps its old joins: **Delete** this book's local audio and make it again to get the new ones.
- **Long chunks are never cut off.** The model's server stops after 163.8 s of audio unless told otherwise, which a slow voice (or an instruction to read slowly) could reach before the end of a long chunk, silently dropping its last sentences and putting the highlight in the wrong place. Each request now gets room for three times its expected length (within what the model can hold), and a take that still runs to the limit is made again; if that happens twice, those sentences are reported as failed instead of being saved wrong.
- **The Advanced settings explain themselves.** Hovering a setting's name or its slider now says what it changes in what you hear and when to change it: Temperature for a calmer or livelier narrator, Top-p for fewer odd readings, Repetition penalty for stutters, the Detail settings for a cleaner or more natural-sounding voice, Seed for another take, Chunk length for steadier reading, and At once for speed and graphics memory.

## [2026.11.19.03] - 2026-11-19

### Added
- **Read aloud with a voice on this computer.** **Read aloud → Read by** now offers **This computer** next to Azure: Qwen3-TTS 1.7B CustomVoice (Alibaba Qwen, Apache-2.0) runs on your GPU through qwentts.cpp's server (MIT). It is free, needs no key and works offline once downloaded. Everything else works as with Azure: the sentence heard is highlighted, the pages follow, a click reads from a word's sentence and the dictionary popup pauses it.
  - **Nothing is downloaded until you ask.** **Download (2.3 GB)** in the panel fetches the model (Q8_0) and the server, pinned and checked by SHA-256, into the models folder. The button greys out once pressed. The server is built for Windows and Linux (Vulkan: NVIDIA, AMD and Intel GPUs) and for Apple Silicon Macs (Metal).
  - **The model is loaded once.** It loads on first use, in a few seconds (the very first start also compiles the GPU's shaders and takes longer), and stays loaded until hanstat closes or you press **Unload**, which frees its GPU memory (about 4–5 GB with four requests at once).
  - **Voices.** Nine voices, each described in the panel: Vivian, Serena, Uncle Fu, Dylan (Beijing), Eric (speaks Sichuanese), Ryan, Aiden, Ono Anna and Sohee. **Instructions** take free text in any language for tone, pace or emotion, for example 用温柔平稳的语气，语速稍慢. The speed slider is Azure's only: the local voice's pace comes from the instructions.
  - **Preview** reads a few sentences from the page shown with the current settings, so you can hear them before making a chapter. It is not saved.
  - **Advanced** has every setting the model takes: temperature, top-p, top-k and repetition penalty, the same for the fine detail, a seed (another number, another take), the chunk length and how many requests the GPU works on at once (1–4). **Reset to defaults** keeps the voice and instructions.
  - **Steadier reading.** Whole paragraphs, up to 200 characters by default, are read in one request, so the voice keeps the same tone from sentence to sentence instead of starting afresh at each one. The audio is then cut into one clip per sentence by matching its pauses to the text's punctuation (a long pause at a comma stays at the comma), so the highlight follows the voice sentence by sentence and click-to-read still starts at a sentence. Checked with a speech recogniser on 死亡万花筒: every clip of its story chapters says its own sentence. Reading from a sentence not made yet starts with a short chunk, so the sound comes within a second or two.
  - **Generate locally…** (under **Ahead of time**) opens a window with the voices, instructions, preview and advanced settings, and the book's chapters by their table-of-contents names. Each chapter shows its sentences and about how long it lasts, with **All**, **None**, **This one** and **From this one on**. **Generate** makes the chosen chapters four requests at a time, about 20× faster than real time on an RTX 5070, skipping sentences already made, and switches reading aloud to the local voice.
  - **Saved audio.** Each sentence is saved as Ogg Opus (about 30 kbit/s, an hour of speech in about 14 MB), next to the book's Azure MP3s. The two never replace each other: switching back to Azure keeps the local audio, and **Saved audio** has separate rows and **Delete** buttons for each. Each voice, instruction and setting is saved separately.
  - Saved in `reader.json` under `tts` as `source` and `local`. Older versions ignore them.

Every book is analysed again once: the audio codec brought new libraries into the build, and saved analyses are kept per set of libraries.

## [2026.11.19.02] - 2026-11-19

### Added
- **Read aloud.** **Read aloud** in the reader's bar has the book read by an Azure AI Speech neural voice, sentence by sentence, using your own Azure Speech key and region (from the Speech resource's **Keys and Endpoint** page; the free F0 tier gives 500,000 characters a month, and Azure counts each Chinese character twice).
  - **Reading.** **Read from this page** starts at the page's first sentence. The sentence heard is highlighted, the pages turn with it (into the next chapter too), and **Pause**, **Resume** and **Stop** sit in the bar while it reads. While reading, a click on a word reads from its sentence, so you can go back to an earlier one; pages turned by hand stay where they are until the reading comes back to them.
  - **The dictionary popup** pauses the reading while it is open and resumes it when it closes (**Pause while the popup is open**, on by default).
  - **Voices.** Only Chinese neural voices are offered (Mandarin, Taiwanese Mandarin, Cantonese, Wu and the regional ones), from the region's own list. Simplified and Traditional books each keep their voice (Xiaoxiao and HsiaoChen by default). The speed goes from −50% to +100%.
  - **Pausing is smooth.** Pause, Stop, the popup and a click on another sentence fade the voice out over 60 ms (and resuming fades it in over 40 ms) instead of cutting it with a click.
  - **Saved audio.** Every sentence's audio is saved (24 kHz MP3) and reused, so a passage is paid for once, in one folder per book: `read-aloud\<title> [id]` in the cache folder (`%LOCALAPPDATA%\hanstat` on Windows, `~/.cache/hanstat` on Linux). **Ahead of time** makes a chapter's or the whole book's audio now, showing how many characters Azure will bill at most; sentences already saved are skipped. The panel shows the book's saved audio and every book's, each with **Delete**. Each voice and speed is saved separately.
  - **The key is never stored in plain text.** On Windows it is encrypted for your Windows account (DPAPI), on macOS it goes in the Keychain, and on Linux it goes in the keyring (Secret Service: GNOME Keyring, KWallet). It is not in `reader.json`. If the keyring refuses it, the key is used until hanstat closes.
  - Azure's rate limits are respected: a 429 answer makes every request wait (Retry-After).
  - The Flatpak now asks for sound (`--socket=pulseaudio`) and the keyring (`--talk-name=org.freedesktop.secrets`).
  - Saved in `reader.json` as `tts`. Older versions ignore the key.

## [2026.11.19.01] - 2026-11-19

### Changed
- **CC-CEDICT's classifiers get their own line in the popup.** `CL:` and its measure words used to follow the senses on the same line (nervous; tense; scarce; CL:陣); they now start a line below them, also in the glossaries the popup's **+** sends to Anki.

## [2026.11.19.00] - 2026-11-19

### Added
- **Add Anki cards from the popup.** Each entry in the dictionary popup has a **+** at the right of its headword that adds a note for that word to Anki. It is greyed out when the word is already in Anki, and turns into **✔** once added (click it to see the note in Anki's browser). Notes go through AnkiConnect, as Yomitan's do: Anki must be running with the AnkiConnect add-on (2055492159) or one that answers the same way (AnkiConnect Extended, Tsunagi). hanstat never writes to the collection file itself.
  - **Setting up.** **Dictionaries → Anki cards → Set up…** (or the **+** before anything is chosen) picks the deck and note type, then the template of each field, with Yomitan's markers: `{expression}`, `{reading}`, `{main-definition}` (the first dictionary's definitions, also `{glossary-first}`), `{glossary}`, `{sentence}` (the book's sentence with the word in bold), `{cloze-prefix}`, `{cloze-body}`, `{cloze-suffix}`, `{document-title}` (the book's title), `{frequencies}` and `{frequency-harmonic-rank}`. `{audio}`, `{screenshot}` and `{popup-selection-text}` are accepted and left empty, so Yomitan templates work unchanged (HyperTTS can add the audio in Anki). Lapis fields get Yomitan's usual templates on their own: Expression `{expression}`, ExpressionReading `{reading}`, ExpressionAudio `{audio}`, SelectionText `{popup-selection-text}`, MainDefinition `{main-definition}`, Sentence `{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}`, Picture `{screenshot}`, Glossary `{glossary}`, Frequency `{frequencies}`, FreqSort `{frequency-harmonic-rank}`, MiscInfo `{document-title}`; other note types' fields are guessed from their names. Tags default to `hanstat`.
  - **Duplicates.** A word is already in Anki when a note of the same type has the same first field, as in Yomitan; **Notes of every type count** widens that to every note type. Anki itself refuses a duplicate, so a click can never add one twice.
  - **Speed.** The popup shows as fast as before: Anki is asked on a thread of its own after the popup is drawn, and the buttons update a moment later.
  - Glossaries keep their formatting; links to other words become plain text and dictionary images are left out. Words looked up inside a popup have no sentence.
  - Saved in `reader.json` as `anki`. Older versions ignore the key.

## [2026.11.18.06] - 2026-11-18

### Added
- **HSK 2.0 levels.** In Chinese mode the known-words window has an **HSK 2.0 vocabulary** section between HSK 3.0 and TOCFL: checkboxes for levels 1–6 and an **All up to** row. They can be ticked alongside or instead of HSK 3.0 and TOCFL, and the toolbar shows them as `HSK 2.0 1–4`.
  - **The list.** The 2012 revision of the HSK word lists (5,000 entries: 150/150/300/600/1,300/2,500 per level), which the HSK tested until the 2025 syllabus and the HSK Standard Course textbooks teach. Taken from hskhsk.com's lists (MIT) and checked against the official 2012 spreadsheet. Paired entries count as their words (虽然……但是…… gives 虽然 and 但是), and a homograph listed twice counts at its first level (得 is level 2, 得 děi level 4). This gives 4,995 words.
  - **Traditional books.** The list is in Simplified characters; with **Match Traditional ↔ Simplified** on (the default), it matches Traditional books too.
  - **Measured.** On 黄金台, HSK 2.0 1–6 covers 60.4% of running words, HSK 3.0 1–6 covers 71.6%, and both together cover 73.9% (HSK 2.0 1–4: 51.9%, HSK 3.0 1–4: 61.4%). The 2012 list leaves out frequent single-character words that the 2025 syllabus has (道, 中, 时, 事, 手, 可, 后), and forms such as 他们 and 这么.
  - **CLI.** `--hsk2 N` marks levels 1 to N. `--known` includes the HSK 2.0 levels ticked in the window.
  - Saved in `known.json` as `hsk2`. Older versions ignore the key.

## [2026.11.18.05] - 2026-11-18

### Fixed
- **A new word's popup starts at the top.** After scrolling down in the popup, the next word looked up opened scrolled down as far.

## [2026.11.18.04] - 2026-11-18

### Changed
- **Choose which dictionaries start collapsed.** A **Collapsed** checkbox beside each dictionary in **Dictionaries**: its entries start closed in the popup (click its bar there to open them), the others open. It used to be the first three open, whichever they were. The list now has column headings.
- **Resize the popup.** Drag the grip in its corner (the one away from the word). The size is kept when hanstat restarts; **Reset** in **Dictionaries** goes back to the default.
- **A click on a popup closes the popups opened from it.**

### Fixed
- **The popup no longer flickers between two words.** With Shift held on a word whose popup sits above it, the popup could switch back and forth between that word and one from an entry scrolled out of sight (the hidden part lay under the pointer). A popup now only looks up what is visibly under the pointer, on that popup.
- **A popup opened from another is never behind it.** After a click on a popup, the next popup opened from it could show underneath.

## [2026.11.18.03] - 2026-11-18

### Added
- **Look words up in the reader.** Hold Shift with the pointer on a word: its entries in your Yomitan dictionaries open beside it, and the word is highlighted on the page. The longest word starting there comes first, then shorter ones. Traditional text also finds entries that dictionaries list only in Simplified.
  - The popup is dark (Catppuccin Mocha) and set in sans-serif. Entries look as their dictionaries style them: headword badges, numbered senses, examples, pictures. Short senses share a line (说话: *to speak; to say; to talk; …*). Each dictionary's entries sit under a bar that opens and closes (the first three are open); examples behind **▸** open with a click.
  - Hold Shift over a word in the popup, or click a cross-reference, for that word's own popup beside it, up to three deep. Esc closes the topmost popup; a click on the page or a page turn closes them all; ↑ and ↓ scroll it.
  - Frequency lists show each word's rank (说话: Danmei 194, Bilibili 563). A Traditional word shows its Simplified form's rank when that is the more frequent, marked **(简)**: lists made from Simplified text rank the Traditional spelling far down.
  - A lookup in 17 dictionaries (8.9 million entries) takes about 0.1 ms, and the popup shows in the same frame as the pointer reaches the word.
- **CC-CEDICT comes by itself.** With no dictionary installed, opening the reader downloads [CC-CEDICT](https://github.com/MarvNC/cc-cedict-yomitan) (8 MB, under a second), once: delete it and it stays deleted.
- **Dictionaries**, in the reader's bar: import Yomitan dictionaries (.zip; or drop them on the reader). The 17 tried, 590 MB of zips including Wikipedia's 4 million titles, import in 5.4 seconds. Each is kept in hanstat's own files (`dictionaries` in the app folder), about 2.5 times its zip's size. Turn dictionaries on and off, put them in order, delete them; choose the key (Shift, Ctrl, Alt, or none: rest the pointer on the word) and the frequency list that orders entries of the same length.
- **Dictionary updates.** Most dictionaries say where new revisions are published. Those are checked once a day when the reader opens, or with **Check for updates**; **Update to …** downloads and installs the new revision while the old one keeps working until the new one is ready. A check asks only each dictionary's own site. Installing what is found without asking is off unless you turn it on; `HANSTAT_NO_UPDATE` turns checks off.
- `hanstat-cli dict import|list|lookup|bench|check` does the same from the command line.

  Every book is analysed again once: the dictionaries brought new libraries into the build, and saved analyses are kept per set of libraries.

## [2026.11.18.02] - 2026-11-18

### Fixed
- **The reader's font lists open.** Clicking the Serif or Sans-serif box in **Aa** still closed the panel in 2026.11.18.01: egui keeps one open popup per window, so the list replaced the panel. The panel now stays open while you choose, until you click **Aa** again, press Esc or click the page.
- **Esc closes the open menu or panel, not the reader.** With **Aa** or **☰ Contents** open, Esc closed the reader too.

### Changed
- **Contents numbers its entries when the book repeats names.** Some books give every chapter the same title (偷偷藏不住: 偷偷, 偷偷, …; 难哄, 折月亮), so the entries read **1. 偷偷**, **2. 偷偷**, …, numbered at each level; the page heading shows the same. Each entry also shows its length in characters (Chinese characters, letters and digits; no punctuation), and the menu opens at the chapter you are in, no wider than its entries.

## [2026.11.18.01] - 2026-11-18

### Added
- **Read the book.** **📕 Read** beside a Chinese EPUB's title on the Overview opens it in a reader that fills the window, with the book's own layout: its headings, indents, spacing, rules, colours, links and pictures, cover and illustration pages, and ruby (notes above the text). Pages are laid out by hanstat itself for Chinese: lines are justified to a grid of whole characters, never start with closing punctuation (，。」…) or end with opening punctuation (「《“), and never split —— or ……. Books made for vertical reading (most Readmoo ones) are shown horizontally, with their spacing turned to match.
  - **One page or two** side by side (when the window is wide enough), switched in the reader's bar. Turn pages with the arrow keys, Page Up/Down, Space, the mouse wheel, or a click (the left third goes back); Home and End go to the start and end of the chapter, **☰ Contents** to any chapter, Esc back to the statistics. The book reopens where you left it.
  - **Aa** sets the serif and sans-serif fonts, chosen from those installed that have Chinese (or the bundled LXGW WenKai), which one text without a font of its own uses, the text size, the least line spacing and the margins. It stays open while you choose, until you click **Aa** again, press Esc or click the page.
  - The page is Solarized Light. Chapters are prepared in the background as the book opens, and pictures are loaded at the size they are shown.

  Nothing is analysed again.

## [2026.11.18.00] - 2026-11-18

### Changed
- **Library titles are shown in full when there is room.** The Book column of the Library's Books and Reading path tabs is as wide as the longest title, where the window leaves room for it; titles were cut at 300 points (140 with writing style on) however wide the window. It is never narrower than 140 points, and stays the same width while you search. It can no longer be resized by dragging.
- **A book's image lines up its two last cards.** The Vocabulary coverage chart is as tall as the Words to know table beside it.

## [2026.11.17.16] - 2026-11-17

### Added
- **Save the Library as an image.** 💾 Export → **Library as image (PNG)…** draws one long picture, 1920 pixels wide: the whole library at a glance (books, characters, words, reading time, books read, the longest book), then each language with its size, vocabulary and how much of it you know, its books by difficulty, the vocabulary coverage and length-against-difficulty charts, and a card for every book with its cover, script, difficulty, read mark and figures. The Export menu is there with no book open too, for this.
- **Save a book's Overview as an image.** **🖼 Save as image…** at the top right of the Overview (or 💾 Export → **Book as image (PNG)…**) draws the book's cover and title, its difficulty on a scale of the bands, the text's figures, what you know of it, its coverage curve and the words needed for each share of the text.

  Both follow the window's light or dark theme and are saved in the background. Book titles are set in LXGW WenKai, bundled with hanstat (SIL Open Font License; see `LICENSE-LXGWWenKai-font.txt`).
- **What's new.** The first start of a newer version shows the release notes since the version you had, once. Clicking the version number at the bottom right now has **Changelog**, every release's notes.

  This release only records the version you have: its notes show from the next update on.

  Nothing is analysed again.

## [2026.11.17.15] - 2026-11-17

### Changed
- **Chinese books count far fewer non-words.** A segmenter that misplaces a boundary invents a "word" there: 他妈凭什么 became 他/妈凭/什么, 木柯跑 glued a name to a verb, 时此刻 is what was left of 此时此刻. Each occurs once or twice, but together they were about a third of a novel's distinct words. Now a token that no dictionary knows and that the book itself produces fewer than 5 times is split again into dictionary words and the book's own frequent words (its names and terms): 妈凭 → 妈/凭, 木柯跑 → 木柯/跑. On 我在无限游戏里封神 (LTP) the distinct words went from 44,224 to 30,196, and names get back the occurrences that were glued to a neighbour (白柳 18,502 → 19,760). Forced words and known words are never split, and neither is a rare name the text itself gives away: a string that recurs, whose characters occur mostly inside it (衢 of 白衢), and that isn't always next to the same character (三鹦 of 杜三鹦 is).
- **Suggested words** lists names the segmenter still gets wrong in as few as 1% of their occurrences (5% before). Most names are now counted right without forcing them, as the glued forms (骆闻舟面, 戚容道) are split again; those, up to 99% right, are no longer suggested.
- **Idioms count as one word.** The segmenters cut idioms apart (若/有所思, 一/头/雾/水) or glued a name to them (白柳若/有所思, 张傀声/嘶力竭); now each of about 60,000 idioms is one word wherever it occurs, in either script: 若有所思 is counted 122 times in that novel instead of 17. A forced word still wins over an idiom.

  The dictionaries are bundled as headword lists only (`data/chengyu.txt`, `data/extra-words.txt`; see `data/Headwords-LICENSE.txt`), built by `examples/headwords.rs`.

  Every book is segmented again once, as the counts changed: seconds a book with LTP, minutes with CKIP.

## [2026.11.17.14] - 2026-11-17

### Changed
- **The Library analyses its books again only when their counts can change.** It used to also do so when:
  - **a Korean or Japanese word list was imported or changed**: lists only join the segmenter's tokens in Chinese books, so those books' counts never changed;
  - **a release changed the known-word lists' code or the bundled HSK, TOCFL, Korean, JLPT or Kaishi lists**: which words you know already updates the rows from their saved word data;
  - **a release changed how the Library is saved or shown**: 2026.11.17.9 analysed every book again for making one date function public;
  - **a release changed the Library's search**, which also threw away the saved analyses, so books opened slowly once again.

  A release that changes how known-word shares are worked out now updates the rows from their saved word data (milliseconds a book) instead of analysing every book again.

  This release itself analyses the Library once more in the background.

### Fixed
- **2026.11.17.12 analysed every Library book again for nothing.** Its settings and keyboard-shortcut code counted as part of the analysis, so the Library's rows (and the saved analyses) looked out of date although every count was the same. They no longer count.
- **ⓘ explains, hovering doesn't repeat it.** The Overview's **Running words you know**, **Difficulty for you** and **Writing style** tiles showed their whole explanation on hover as well as on ⓘ; now only ⓘ does. The Library's search box likewise shows a short hint, with the explanation on its ⓘ.

## [2026.11.17.12] - 2026-11-17

### Added
- **Keyboard shortcuts.** **Ctrl+O** opens books, **Ctrl+L** shows or hides the Library, **Ctrl+F** goes to the search box: the Characters or Words tab's filter (from the other tabs, the Words tab's), or the Library's search in the Library or with no book open, with its text selected so that typing replaces it. **Ctrl+1**–**Ctrl+6** show the book's tabs (also from the Library), **Ctrl+Page Down** and **Ctrl+Page Up** the next and previous tab (the Library's while it is shown). **Ctrl+K** opens the Known words window, **Ctrl+Shift+K** Anki, **Ctrl+D** Forced words, **Ctrl+,** Settings, and **Esc** closes the window in front. In the Words tab, **Ctrl+C** copies the selected word. Buttons, tabs and search boxes show their shortcut in their hover text. Keys without Ctrl or Alt (Esc, A) do nothing while you type in a text box: there Esc first leaves the box.
- **Change the shortcuts** in ⚙ → **Keyboard shortcuts**: click one and press the new keys (Esc keeps the old ones), ✖ for none, ⟲ for the default, or **Reset all**. Keys another action had are taken from it, with a note saying so; keys that move between or through things (Tab, the arrows, Page Up/Down, Home, End, Enter, Space) or edit text (Ctrl+A, Ctrl+Z, Ctrl+Y) are refused. The Words tab's **A** (mark known) can be changed too. Only the shortcuts you change are saved, in `settings.json`, so the others follow new defaults; a new default never takes keys you chose.

### Fixed
- **The status bar no longer grows to fill the window.** In 2026.11.17.11 it could slide up over the content when the pointer passed over it, and stay there: its layout could take the window's whole height, so its separators and centred items grew with every redraw. It is one line high again, and takes a second or third line only when what it shows doesn't fit (in a narrow window, while books are analysed).

## [2026.11.17.11] - 2026-11-17

### Changed
- **Known words update the Library in seconds.** A Library row now records apart what changes a book's counts (hanstat's analysis, the user dictionary, the word lists given to the segmenter, Match Traditional ↔ Simplified) and what only changes how much of them you know (the known words, **Leave out words that occur only once**). When only the second changed, as after marking words known or a book read, the row is computed again from the book's saved word data, which now also hold its character counts, instead of opening, reading and searching the book for names again. Measured on a Library of 81 novels: about 16 ms a book on one core, against 0.4–1 s a book on all of them before. A book whose file has moved is updated too.
- **Less work while you click.**
  - The known words (Anki plus levels, lists, marked words and read books) are put together again only when one of them changes; before, also on every ticked section and whenever word discovery finished, and each time the Library's rows, its statistics and the reading path were worked out again for nothing. Japanese word-list entries are read with Sudachi once per session instead of every time.
  - **Library → Stats** are counted only while that tab is open.
  - The Library, the books' word data and the known words are saved in the background, a moment after the last change (marking word after word known writes them once), instead of while the window is drawn. Whether each Library book's file is still there is checked in the background every 5 seconds instead of at every frame, so a book on a sleeping drive or a network share no longer stalls the window.
  - Ticking sections looks for suggested words once, 300 ms after the last tick, and stops a search already running; jieba's dictionary is loaded once.
  - The Characters and Words tables filter and sort only when the filter, the sort or the book change, not at every frame.
- **Opening another book stops the analysis still running**, instead of letting it finish unseen. **Changing the segmenter or a TXT's encoding while a book is analysed** analyses it again with the new choice; before, the new choice was shown but the book was analysed with the old one.
- **Settings window.** **⚙** in the status bar opens a window with the settings (Counting, Reading speed, Writing style) instead of a menu that closed at a click elsewhere.
- **The window remembers** its size and place, the tab, the Library's tab, sort and **Hide read**, and the Characters and Words tables' sorts (in `window.ron` next to `settings.json`). It grows to fit the Overview only until it has a size of its own: on the first start, and until you resize it.
- **Colours on the light theme.** Status, difficulty, warning and error colours were made for the dark theme and had a contrast of about 2:1 on the light one (Young 1.9, Moderate 1.8, warnings 2.0). On the light theme they now have darker versions with a contrast of at least 4.5:1; the dark theme is unchanged.
- **ⓘ explains.** The Overview's **Running words you know**, **Difficulty for you** and **Writing style** tiles, the Library's search and the Suggested words' columns have an **ⓘ** that shows their explanation when clicked; before, it was only in a tooltip.
- **The status bar** shows the open book's file name (the full path on hover) and takes a second line when what it shows doesn't fit on one.
- **Releases are numbered with two digits per day**: 2026.11.18.00, .01, … GitHub lists a day's releases by tag name as text, which put 2026.11.17.10 between .2 and .1. Earlier versions update to them as usual.

### Added
- **Sortable Characters and Words tables.** Click **Rank**, **Char** or **Status** (Characters) or **Rank**, **Word**, **Status** or **Length** (Words) to sort; again to reverse. Ties stay in frequency order. The **Word** column can be dragged wider, and long words are cut off at its edge instead of running into the next column.
- **Undo removing a book.** ✖ in the Library takes the book out at once, and **↩ Undo** in the status bar puts it back for 10 seconds; only then are its cover and word data deleted. The ✖ is set a little apart from **✔**.
- **Errors can be dismissed.** Each error above the content has a **✖**, and a new error replaces only an earlier one of the same kind (a failed save no longer hides a failed analysis). The Library's warning line has a ✖ too.

### Fixed
- The hover texts of the Library's search box and of **Rate the books without a rating** had runs of spaces in the middle.

On the first start of this version every Library row is analysed again once, so that its word data hold the character counts, and each book is segmented once more (the analysis code changed): seconds a book with LTP, minutes with CKIP. After that, changes to the known words take the fast way.

## [2026.11.17.10] - 2026-11-17

### Changed
- **Traditional and Simplified characters count as one.** With **Match Traditional ↔ Simplified** on (the default), 說 and 说 are one character wherever characters are counted, as words already were. Before, a Library with books in both scripts counted every such pair twice, in the characters known and in the total, so **Characters known** and **Distinct characters** in Library Stats were too high. A book's Overview, Characters tab and sections count the same way: a book that mixes scripts no longer has two of a character, and pairs such as 髮/發 (both 发) are one. Each character is still listed as the book writes it, the more frequent form where it uses both, and the Characters tab's filter finds it in either script (说 finds 說). Turning the option off counts characters as written again. Words in a book are still counted as written.
- **Traditional books are rated for writing style in Simplified.** The model's examples and its typical rating are Simplified, and it rated Traditional text lower as it was than converted: by 0.19 on 16 Taiwanese editions (61 of 64 samples rose), while Simplified novels converted to Traditional did not move. A Traditional book's samples are now converted character by character before rating; text that mixes scripts is left as it is. Every book is rated again once.

### Fixed
- **llama.cpp no longer outlives hanstat.** If hanstat was closed while the model loaded, crashed or was ended from Task Manager, `llama-server` kept running and holding GPU memory (three left over held 7 GB of a 12 GB card and slowed the next rating run fifteenfold). On Windows it now runs in a job that ends with hanstat, on Linux it is killed when hanstat ends, and closing hanstat while the model loads stops it at once. macOS still relies on hanstat closing normally.

On the first start of this version every Library row is analysed again once.

## [2026.11.17.9] - 2026-11-17

### Added
- **Mark a book read.** The Library's new **Read** column has a **✔** button for each book: of its words that occur at least N times (10 by default), the most frequent P% (90% by default) count as known. The window shows how many words that is, how many of them you didn't know yet, the most frequent of those, and their share of the book's running words; the last P and N are kept in `plan.json`. A read book's ✔ turns green, with when it was marked on hover; click it to change the share or **Unmark read**. The words are saved as they were with the language's known words (`known.json`, `-ko`, `-ja`), so analysing the book again doesn't change them and unmarking takes back exactly those (a word another source marks known stays known). Names count too, which helps the next volume of a series.
  - **Not given to the segmenter.** A read book's words don't join pieces the segmenter split. Feeding thousands of a book's tokens back would carry its segmentation errors (LTP's glued names such as 蒋舸没) to other books, and every mark would invalidate the analysis cache of every Chinese book. Marking a book changes only the known words: the Library's rows are recounted from the saved analyses, about a minute for 81 books.
  - **Reading path.** A read book leaves the path, with a line under it saying how many did. Each step has **✔** to mark it read, beside **✖** (left out without being read).
  - **Library Stats.** Read books stay in every figure. The per-language table has a **Read** column, and a new **Books read** section shows how many, their characters and reading time, the words they count as known (and how many were new), and each read book.
  - **Hide read** next to the Library's search box leaves them out of the list. The **📚** window lists the books read, each removable with ✖ (also those no longer in the Library). The known-words label counts them ("HSK 1–5 + 1 book read").
  - Works for the books of a language not in use too: its known words are changed in its file.

### Changed
- **The README is short:** what hanstat does, how to install it, a page on using it, the command line, where its data is and the licences, with a new screenshot. The details of every feature are in this changelog.
- **Writing style is shown as a percentage of your Library**, in the Overview, the Library and the reading path. The model rates on a scale of 1 (plain) to 9 (near classical Chinese), but books use only a narrow band of it (5.1 to 6.6 in a test library of 70 novels), so the ratings all read alike. Now a rating is placed on a logistic curve fitted to the rated Chinese books of the Library: their median shows as 50%, their 5th and 95th percentiles as 5% and 95%, so whatever band a library's books occupy, they spread over the whole range (in the test library, 0.9% to 97%). Nothing is fixed to one library's numbers; one rated book shows as 50%. The percentage moves as books are rated; the word after it (plain, typical, literary, very literary) is the model's own judgment and doesn't. Hovering a rating shows the model's own number, which is what is kept (`style.json` is unchanged) and what counts toward the difficulty, with the **Weight** in ⚙ per point of it as before.
- **Words marked with + no longer change how books are segmented.** Since 2026.11.17.1 they joined pieces the segmenter split (想/到 → 想到), so each + or − in a Chinese book meant segmenting every Chinese book in the Library again (minutes a book with CKIP). They are the segmenter's own tokens, so joining them elsewhere mostly turned two known tokens into one known word. Now a mark only recounts the books, from the saved analyses. Imported word lists still join split words, as their words come from outside the book (一口气 split as 一/口气 would otherwise never match). To keep a name whole, apply it from Suggested words or type it into **Force a word**, which also splits it off a word it was glued to. If you have words marked with +, the Chinese books are segmented once more after updating.

### Fixed
- The Library's **Difficulty** column sorts: by the share of running words known as counted for the difficulty (the writing style included, while rated), hardest first, books with nothing known last.
- The Library's sortable column headings look clickable: they are underlined under the pointer, which turns into a hand, and their hover text says a click sorts and a second reverses.

## [2026.11.17.8] - 2026-11-17

### Added
- **The Library keeps itself up to date.** Each row records what it was computed from: hanstat's analysis, the user dictionary, the known words (Anki and word lists) and the settings for its language. When any of them changes, the rows affected are analysed again in the background, 5 seconds after the last change, so marking a run of words known costs one update. A change while it runs stops it and it starts afresh after the pause. Rows keep their place and their date; a book removed meanwhile is not brought back. The status bar shows the progress; **Stop** holds it until something changes again. Books of a language not used since hanstat started wait until it is, as do all rows while Anki is still being read. On the first start of this version every row is updated once. Marking Chinese words known changes how books are segmented, so each such update segments the Chinese books again: seconds a book with LTP, minutes with CKIP.
- **⟳ Recalculate library** at the right of the Library's header analyses every book again at once.
- **Sections are kept per book.** The sections you include are saved with the book's Library row and restored when it is opened again; background updates use them too, with the segmenter the book was last analysed with (both were the defaults before).
- **Default reading speeds** in ⚙ (Chinese, Japanese, Korean), saved in `settings.json`, for the Library's and Library Stats' reading times and each book's Overview. An Overview's speed can still be changed for that book alone, with **↺** to go back; opening another book starts from the default again.
- **Force a word by typing it.** The Suggested words tab, and a new **📖 forced words** window from the status bar (for every language), have a **Force a word** box (several words can be separated by spaces) and the list of forced words, each removable with ✖.
- **Forced words are saved** between runs, per language, in `dictionary.txt` (`dictionary-ko.txt`, `dictionary-ja.txt`) next to `settings.json`, in the format `hanstat-cli --dict` reads.

### Changed
- The Library's header no longer has a line of explanation under the tabs; their hover text remains.
- **Suggested words finds names the segmenter glues to the next word.** LTP often keeps a name with a rare character together with the word after it (蒋舸没, 蒋舸问, 蒋舸站), and such a name was never suggested: only a split counted as an error. Now a string inside a longer token that is no dictionary word counts as wrong too (the column is now **Wrong**). The main characters of 默读 (骆闻舟) and 天官赐福 (谢怜) were missing for this reason and are now recommended, with about 19 more names in each of three test novels.
- Suggested words checks CC-CEDICT besides jieba's dictionary for general words, so words like 靠谱, 抛尸, 副市长 are no longer pre-ticked (1–3 fewer non-names per test novel, no name lost).
- The status bar's **✖** that removed all forced words is gone; **Remove all** is next to the list.

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
