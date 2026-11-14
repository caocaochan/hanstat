# hanstat

[![CI](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml/badge.svg)](https://github.com/caocaochan/hanstat/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/caocaochan/hanstat)](https://github.com/caocaochan/hanstat/releases/latest)

Character and word statistics for Chinese `.epub` and `.txt` files, with counting rules designed to be *exactly* right for characters and *transparently* model-dependent for words.

## Install

Download `hanstat-vX.Y.Z-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/caocaochan/hanstat/releases/latest), unzip, and run `hanstat.exe`. Open a file, or drag it onto the window. The exe isn't code-signed, so Windows SmartScreen may ask you to confirm the first run. You can check the zip against its `.sha256` file.

The segmentation models are **not bundled**, because their licences differ from this project's. The first time a model is needed, hanstat downloads it from its official Hugging Face repository:
- LTP: 32 MB.
- CKIP: 46 MB.

Each download is pinned to an exact commit and verified by SHA-256, then cached in `%LOCALAPPDATA%\hanstat\models` (override with `HANSTAT_MODEL_DIR`). Nothing else is ever sent or fetched. jieba works fully offline.

CJK glyphs come from Windows system fonts (YaHei, JhengHei, SimSun-ExtB/ExtG).

### Command line

```
hanstat.exe [file] [--tab overview|characters|words|sections|suggestions|notes]
hanstat-cli.exe file.epub [--ckip|--ltp|--jieba] [--dict names.txt] [--auto-dict] [--suggest N] [--json] [--top N] [--include-all] [--exclude i,j] [--chars-csv f] [--words-csv f]
```

### Building from source

Rust stable (edition 2024) with the MSVC toolchain:

```
cargo build --release
cargo test --release      # downloads the models on first run
```
## What it reports

- Chinese characters: total, unique, and once-only (hapax).
- Chinese words: total, unique, and once-only.
- Composition: punctuation, Latin/other letters, digits, symbols, private-use code points and decode errors.
- Non-Chinese words and numbers.
- Paragraphs, sentences, and average sentence, paragraph and word length.
- Word-length distribution.
- Coverage: how many of the most frequent characters or words you need to recognise 50/80/90/95/98/99/99.5% of the running text.
- Per-section breakdown, including how many characters and words each section introduces that haven't appeared earlier. Untick a section (cover, copyright page, author's notes) and every total re-merges instantly without re-segmenting.
- Traditional vs Simplified detection, based on OpenCC's script-exclusive characters.
- Reading-time estimate at an adjustable speed.
- Exports: character and word frequency CSV (UTF-8 BOM, so Excel opens it correctly), the full report as JSON, and copying the unique lists to the clipboard.

## Counting rules (why the numbers are right)

| Problem in typical tools | What hanstat does |
|---|---|
| Regex `[一-鿿]` misses Ext A–J, supplementary planes, 〇 | A Chinese character is Unicode **Script=Han ∧ letter**. This covers every ideograph block and excludes radicals and punctuation. |
| Latin letters, digits and spaces counted as "characters" | Counted separately; never mixed into Chinese counts. |
| NFKD/NFKC mangles text, or no normalisation at all | NFC normalisation. Compatibility ideographs are unified, variation selectors are dropped (葛󠄀 = 葛), Kangxi radicals that stand in for ideographs (PDF/OCR artefacts) are folded, and ZWSP, soft hyphen and BOM are removed. Every change is reported. |
| Full-width `？！` not treated as sentence ends | Sentences end at `。！？!?…`; runs such as `？！` or `……` count once. |
| EPUB: the TOC, `<title>` and pinyin ruby get counted | Only the spine is read, in order, each document once. `<head>`, scripts, styles, `<rt>`/`<rp>`, hidden elements and alt text are skipped. The EPUB3 nav document and hand-made "Contents" pages (detected by matching against the NCX/nav labels) are excluded by default. |
| `<span>` or a source line break splits a word | Inline tags never split text. Source newlines between CJK characters are removed (CSS Text 3 segment-break rule), as browsers do. Block elements and `<br>` *do* end a paragraph, so words never join across them. |
| TXT must be UTF-8 | BOM, then strict UTF-8, then BOM-less UTF-16 statistics, then chardetng (GBK is decoded as GB18030). The encoding can be overridden. Hard-wrapped files are detected and their lines rejoined. |
| Rare/anti-copy glyphs in the Private Use Area silently counted | Reported separately; not counted as Chinese. |

**Verification** (2026-11-14):
- On the 13 MB test novel, the Han total, unique count and non-space count match an independent .NET-rune implementation exactly: 3,543,589 / 4,028 / 4,150,685.
- `tests/fixtures.rs` builds an adversarial EPUB with every case above and asserts hand-counted results.
- 40+ real EPUBs, including ones in a `Broken` folder, all parse.

## Word segmentation: three models, chosen by script

Chinese has no spaces, so "words" come from a segmentation model, and no segmentation is "correct" in an absolute sense. Annotation standards (PKU, MSR, AS, CityU) disagree on boundaries such as 三个 vs 三/个, and a model's *standard* changes the unique-word count as much as its errors do. **Character counts are exact; word counts are estimates.**

### Accuracy (SIGHAN 2005 bakeoff gold test sets, span F1, measured with `examples/cws_eval.rs`)

| Test set | jieba | LTP perceptron | CKIP bert-tiny | CKIP bert-base* |
|---|---|---|---|---|
| PKU (Simplified, LTP's home standard) | 83.6 | **95.2** | 86.9 | 89.9 |
| MSR (Simplified, neutral) | 82.7 | **87.7** | 82.2 | 84.3 |
| AS (Traditional, CKIP's home corpus) | 81.9 | 87.2 | **97.9** | 98.1 |
| CityU (Traditional, neutral) | 81.1 | 87.9 | **90.1** | 92.0 |

\* Not shipped: bert-base runs at ~1.6k chars/s on CPU, which is about 30 minutes per novel. It would need GPU inference (a CUDA toolkit for candle, or ONNX + DirectML).

**Each model is best in its own script.** CKIP is fed Simplified text converted to Traditional, at the phrase level with OpenCC (以后→以後). That conversion is not what holds it back: it gains only +0.1 F1 over character-level conversion. The gap comes from the annotation standard and domain. So the default, **Auto**, uses LTP for Simplified and CKIP for Traditional. Any segmenter can be chosen explicitly.

### On the test novel (Simplified web novel, 3.5 M characters, 5900X)

| Segmenter | Total words | Unique words | Main character's name 温简言 kept whole | Time |
|---|---|---|---|---|
| jieba | 2,194,306 | 34,674 | 0% | <1 s |
| LTP (Auto for Simplified) | 2,282,894 | 45,365 | 48% | 2 s |
| CKIP bert-tiny | 2,289,986 | 46,989 | 67% | 169 s |

On fiction, CKIP keeps names together more often than on news-style benchmarks suggest. No gold-segmented novel exists to measure overall accuracy on fiction. **For names, the user dictionary is the reliable fix for every segmenter.** It takes one word per line. Each leftmost-longest match is forced to be exactly one token: boundaries are added at its edges and removed inside it.

All script conversion for the models is length-preserving and happens on a copy: Traditional→Simplified for LTP/jieba, Simplified→Traditional for CKIP. Counts and lists always keep the original characters.

Tools:
- `cargo run --release --example cws_eval -- <icwb2 gold dir> [--base models/ckip/bert-base-chinese-ws]` reproduces the table above.
- `cargo run --release --example segcompare -- book.txt` compares jieba and LTP on a book.

## Suggested words: finding names you haven't read yet

Names are the main source of segmentation errors. When a segmenter splits `温简言` into `温/简言`, unique-word counts fill up with fragments. You can't write a user dictionary for a book you haven't read, so hanstat finds the candidates itself after each analysis, in about 0.5 s on a 3.5 M-character novel. They appear in the **Suggested words** tab, and the CLI equivalents are `--suggest N` and `--auto-dict`.

The method is unsupervised new-word discovery (新词发现) over runs of Chinese characters (`src/discover.rs`). It scores n-grams of 2–6 characters on four signals:

1. **Frequency:** at least `max(5, han_total / 200k)` occurrences.
2. **Dependency:** the minimum, over split points, of f(w) / min(f(left), f(right)). This is the share of the rarer part's occurrences that fall inside the candidate, and it's about 1.0 for names. It works better than plain PMI, which under-scores a protagonist's name because the name itself dominates its characters' frequencies.
3. **Boundary freedom:** the entropy of the characters immediately left and right. Real words appear in many contexts; fragments like `简言` don't.
4. **Segmenter evidence:** how often the current segmenter kept the string whole. Only strings it often splits are listed.

**Pre-ticked (recommended)** candidates meet all of these:
- not in jieba's general dictionary;
- at most 4 characters;
- free boundaries on both sides;
- either the segmenter already keeps the string whole 3–95% of the time, or the evidence is very strong (dependency ≥ 0.8, entropy ≥ 2.5, and no particles such as 了/一/着 inside, which separates set phrases like `指了指` from names).

Nothing changes until you click Apply. The applied list can be exported as a user dictionary for the rest of a series.

| Book | Recommended (LTP) | Examples | Unique words before → after |
|---|---|---|---|
| Test novel (TXT) | 65 | 温简言 云碧蓝 巫烛 祁潜 兴旺酒店 锁魂坛 | 45,365 → 43,402 |
| 默读 | 40 | 骆闻舟 费承宇 郎乔 顾钊 承光公馆 燕城 | 27,188 → 26,597 |
| 天官赐福 | 56 | 谢怜 师青玄 裴茗 白无相 血雨探花 极乐坊 | 29,288 → 28,156 |

Typical false positives are common colloquial words or set phrases (`靠谱`, `面无表情`, `喃喃道`). Forcing them is harmless, and they're easy to untick.
## Licence

hanstat's own code is licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

Bundled data:
- OpenCC character and phrase tables (`data/`): Apache-2.0 ([licence](data/OpenCC-LICENSE.txt)).
- jieba dictionary, via the `jieba-rs` crate: MIT.

Models downloaded at runtime are *not* part of this repository or its releases. Their own licences apply:
- [LTP legacy](https://huggingface.co/LTP/legacy) (HIT-SCIR): free for universities, research institutes and individual researchers. Commercial use requires a paid licence from HIT-SCIR.
- [CKIP bert-tiny-chinese-ws](https://huggingface.co/ckiplab/bert-tiny-chinese-ws) (Academia Sinica CKIP Lab): GPL-3.0.