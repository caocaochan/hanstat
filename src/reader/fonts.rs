//! The installed fonts that can show Chinese, for the reader's serif and
//! sans-serif choices, and their installation into egui.
//!
//! Fonts are found by reading the tables of every font file in the system's
//! font folders (`name` for the family, `OS/2` for weight and style, `cmap`
//! for Chinese coverage): a few small reads a file, done once in the
//! background. Chosen faces are memory-mapped, not read.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug)]
struct Face {
    /// `None`: the bundled LXGW WenKai.
    path: Option<PathBuf>,
    index: u32,
    weight: u16,
    italic: bool,
}

#[derive(Clone, Debug)]
pub(super) struct FamilyInfo {
    /// The English (or first) family name: what settings store.
    pub(super) name: String,
    /// The Chinese name, if the font has one that differs.
    local: Option<String>,
    faces: Vec<Face>,
}

impl FamilyInfo {
    pub(super) fn label(&self) -> String {
        match &self.local {
            Some(l) => format!("{} · {l}", self.name),
            None => self.name.clone(),
        }
    }

    /// The upright face closest to `weight`.
    fn face(&self, weight: u16) -> Option<&Face> {
        self.faces
            .iter()
            .filter(|f| !f.italic)
            .min_by_key(|f| (f.weight as i32 - weight as i32).abs())
            .or(self.faces.first())
    }

    fn has_bold(&self) -> bool {
        self.faces.iter().any(|f| !f.italic && f.weight >= 600)
    }
}

pub(super) const BUNDLED: &str = "LXGW WenKai";

/// The fonts found, once the background scan is done.
pub(super) fn families() -> Option<Arc<Vec<FamilyInfo>>> {
    static SCAN: OnceLock<Mutex<Option<Arc<Vec<FamilyInfo>>>>> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    let slot = SCAN.get_or_init(|| Mutex::new(None));
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            let found = scan();
            *SCAN.get().unwrap().lock().unwrap() = Some(Arc::new(found));
        });
    });
    slot.lock().unwrap().clone()
}

/// Start the scan without waiting for it.
pub(super) fn prefetch() {
    let _ = families();
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Some(w) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(w).join("Fonts"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(l)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(h).join("Library/Fonts"));
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        for d in [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/run/host/fonts",
            "/run/host/user-fonts",
        ] {
            dirs.push(PathBuf::from(d));
        }
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&h).join(".local/share/fonts"));
            dirs.push(PathBuf::from(&h).join(".fonts"));
        }
        if let Some(d) = std::env::var_os("HANSTAT_FONT_DIR") {
            dirs.push(PathBuf::from(d));
        } else if let Ok(exe) = std::env::current_exe()
            && let Some(p) = exe.parent().and_then(Path::parent)
        {
            dirs.push(p.join("share/hanstat/fonts"));
        }
    }
    dirs
}

fn scan() -> Vec<FamilyInfo> {
    let mut files = Vec::new();
    for d in font_dirs() {
        collect(&d, &mut files, 0);
    }
    let mut by_name: BTreeMap<String, FamilyInfo> = BTreeMap::new();
    for path in files {
        for (names, face) in read_faces(&path).unwrap_or_default() {
            let e = by_name
                .entry(names.0.to_lowercase())
                .or_insert_with(|| FamilyInfo {
                    name: names.0.clone(),
                    local: None,
                    faces: Vec::new(),
                });
            if e.local.is_none() {
                e.local = names.1.filter(|l| *l != e.name);
            }
            e.faces.push(face);
        }
    }
    let mut out: Vec<FamilyInfo> = by_name.into_values().collect();
    out.push(FamilyInfo {
        name: BUNDLED.into(),
        local: Some("霞鹜文楷 (bundled)".into()),
        faces: vec![Face {
            path: None,
            index: 0,
            weight: 500,
            italic: false,
        }],
    });
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                collect(&p, out, depth + 1);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
            matches!(
                x.to_ascii_lowercase().as_str(),
                "ttf" | "ttc" | "otf" | "otc"
            )
        }) {
            out.push(p);
        }
    }
}

fn be16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn read_at(f: &mut File, off: u64, len: usize) -> Option<Vec<u8>> {
    if len > 16 << 20 {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut v = vec![0; len];
    f.read_exact(&mut v).ok()?;
    Some(v)
}

/// ((family, Chinese name), face) for each Chinese-capable face of a font file.
/// (English family, Chinese family).
type Names = (String, Option<String>);

fn read_faces(path: &Path) -> Option<Vec<(Names, Face)>> {
    let mut f = File::open(path).ok()?;
    let head = read_at(&mut f, 0, 12)?;
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let n = be32(&head, 8)?.min(64) as usize;
        let tbl = read_at(&mut f, 12, n * 4)?;
        (0..n).filter_map(|i| be32(&tbl, i * 4)).collect()
    } else {
        vec![0]
    };
    let mut out = Vec::new();
    for (index, off) in offsets.into_iter().enumerate() {
        if let Some(x) = read_face(&mut f, off as u64) {
            out.push((
                x.0,
                Face {
                    path: Some(path.to_path_buf()),
                    index: index as u32,
                    weight: x.1,
                    italic: x.2,
                },
            ));
        }
    }
    Some(out)
}

fn read_face(f: &mut File, off: u64) -> Option<(Names, u16, bool)> {
    let dir = read_at(f, off, 12)?;
    let n = be16(&dir, 4)? as usize;
    let recs = read_at(f, off + 12, n * 16)?;
    let mut tables = std::collections::HashMap::new();
    for i in 0..n {
        let r = &recs[i * 16..i * 16 + 16];
        tables.insert(&r[0..4], (be32(r, 8)? as u64, be32(r, 12)? as usize));
    }
    // Outlines egui can draw.
    if !tables.contains_key(&b"glyf"[..]) && !tables.contains_key(&b"CFF "[..]) {
        return None;
    }
    let (weight, italic) = match tables.get(&b"OS/2"[..]) {
        Some(&(o, l)) => {
            let t = read_at(f, o, l.min(100))?;
            let weight = be16(&t, 4).unwrap_or(400);
            let fs_sel = be16(&t, 62).unwrap_or(0);
            // CJK Unified Ideographs (bit 59) or the GB2312 / Big5 code pages.
            let ur2 = be32(&t, 46).unwrap_or(0);
            let cp1 = if be16(&t, 0).unwrap_or(0) >= 1 {
                be32(&t, 78).unwrap_or(0)
            } else {
                0
            };
            let cjk_hint = ur2 & (1 << 27) != 0 || cp1 & ((1 << 18) | (1 << 20)) != 0;
            if !cjk_hint {
                return None;
            }
            (weight, fs_sel & 1 != 0)
        }
        None => (400, false),
    };
    let &(co, cl) = tables.get(&b"cmap"[..])?;
    let cmap = read_at(f, co, cl)?;
    if !['我', '的', '国']
        .iter()
        .all(|c| cmap_has(&cmap, *c as u32))
    {
        return None;
    }
    let &(no, nl) = tables.get(&b"name"[..])?;
    let name = read_at(f, no, nl)?;
    let names = family_names(&name)?;
    if names.0.starts_with('@') || names.0.starts_with('.') {
        return None;
    }
    Some((names, weight, italic))
}

fn cmap_has(cmap: &[u8], c: u32) -> bool {
    let Some(n) = be16(cmap, 2) else {
        return false;
    };
    let mut best: Option<(u8, usize)> = None;
    for i in 0..n as usize {
        let r = 4 + i * 8;
        let (Some(p), Some(e), Some(o)) = (be16(cmap, r), be16(cmap, r + 2), be32(cmap, r + 4))
        else {
            continue;
        };
        let rank = match (p, e) {
            (3, 10) | (0, 4) | (0, 6) => 2,
            (3, 1) | (0, 3) => 1,
            _ => continue,
        };
        if best.is_none_or(|(b, _)| rank > b) {
            best = Some((rank, o as usize));
        }
    }
    let Some((_, o)) = best else {
        return false;
    };
    match be16(cmap, o) {
        Some(12) => {
            let groups = be32(cmap, o + 12).unwrap_or(0) as usize;
            let (mut lo, mut hi) = (0usize, groups);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = o + 16 + mid * 12;
                let (Some(s), Some(e)) = (be32(cmap, g), be32(cmap, g + 4)) else {
                    return false;
                };
                if c < s {
                    hi = mid;
                } else if c > e {
                    lo = mid + 1;
                } else {
                    return true;
                }
            }
            false
        }
        Some(4) => {
            if c > 0xFFFF {
                return false;
            }
            let seg2 = be16(cmap, o + 6).unwrap_or(0) as usize;
            let ends = o + 14;
            let starts = ends + seg2 + 2;
            let deltas = starts + seg2;
            let ranges = deltas + seg2;
            for s in 0..seg2 / 2 {
                let (Some(end), Some(start)) =
                    (be16(cmap, ends + s * 2), be16(cmap, starts + s * 2))
                else {
                    return false;
                };
                if (c as u16) > end {
                    continue;
                }
                if (c as u16) < start {
                    return false;
                }
                let delta = be16(cmap, deltas + s * 2).unwrap_or(0);
                let ro = be16(cmap, ranges + s * 2).unwrap_or(0) as usize;
                let gid = if ro == 0 {
                    (c as u16).wrapping_add(delta)
                } else {
                    let at = ranges + s * 2 + ro + (c as usize - start as usize) * 2;
                    match be16(cmap, at) {
                        Some(0) | None => 0,
                        Some(g) => g.wrapping_add(delta),
                    }
                };
                return gid != 0;
            }
            false
        }
        _ => false,
    }
}

/// (English family, Chinese family) from the `name` table, preferring the
/// typographic family (ID 16) over the legacy one (ID 1).
fn family_names(t: &[u8]) -> Option<Names> {
    let count = be16(t, 2)? as usize;
    let strings = be16(t, 4)? as usize;
    let mut en: Option<(u16, String)> = None;
    let mut zh: Option<(u16, String)> = None;
    let mut any: Option<String> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (Some(p), Some(_e), Some(lang), Some(id), Some(len), Some(off)) = (
            be16(t, r),
            be16(t, r + 2),
            be16(t, r + 4),
            be16(t, r + 6),
            be16(t, r + 8),
            be16(t, r + 10),
        ) else {
            continue;
        };
        if id != 1 && id != 16 {
            continue;
        }
        let Some(raw) = t.get(strings + off as usize..strings + off as usize + len as usize) else {
            continue;
        };
        let s = match p {
            0 | 3 => {
                let u: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16_lossy(&u)
            }
            1 => raw.iter().map(|&b| b as char).collect(),
            _ => continue,
        };
        let s = s.trim().to_owned();
        if s.is_empty() {
            continue;
        }
        // Prefer ID 16 to ID 1 (rank by id).
        let better = |cur: &Option<(u16, String)>| cur.as_ref().is_none_or(|(cid, _)| id > *cid);
        if p == 3 && lang == 0x0409 || p == 1 && lang == 0 {
            if better(&en) {
                en = Some((id, s.clone()));
            }
        } else if p == 3
            && matches!(lang, 0x0804 | 0x0404 | 0x0C04 | 0x1004 | 0x1404)
            && better(&zh)
        {
            zh = Some((id, s.clone()));
        }
        if any.is_none() {
            any = Some(s);
        }
    }
    let name = en.map(|x| x.1).or(any)?;
    Some((name, zh.map(|x| x.1)))
}

// ---------------------------------------------------------------------------
// egui families

static BASE: OnceLock<FontDefinitions> = OnceLock::new();

/// The app's font definitions (system CJK fallbacks), which the reader's
/// families are added to.
pub(crate) fn set_base(defs: FontDefinitions) {
    let _ = BASE.set(defs);
}

/// Unicode Vertical Presentation Forms checked in each installed font's `cmap`,
/// so vertical layout uses them only when the chosen primary font has the glyph
/// (falling back to 90° rotation or optical positioning otherwise).
const VERT_FORMS: &[char] = &[
    '︐', // U+FE10 vertical comma (，)
    '︑', // U+FE11 vertical ideographic comma (、)
    '︒', // U+FE12 vertical ideographic full stop (。)
    '︓', // U+FE13 vertical colon (：)
    '︔', // U+FE14 vertical semicolon (；)
    '︕', // U+FE15 vertical exclamation mark (！)
    '︖', // U+FE16 vertical question mark (？)
    '︙', // U+FE19 vertical horizontal ellipsis (…)
    '︰', // U+FE30 vertical two dot leader (‥)
    '︱', // U+FE31 vertical em dash (—)
    '︲', // U+FE32 vertical en dash (–)
    '︵', // U+FE35 vertical left parenthesis (（)
    '︶', // U+FE36 vertical right parenthesis (）)
    '︷', // U+FE37 vertical left curly bracket (｛)
    '︸', // U+FE38 vertical right curly bracket (｝)
    '︹', // U+FE39 vertical left tortoise shell bracket (〔)
    '︺', // U+FE3A vertical right tortoise shell bracket (〕)
    '︻', // U+FE3B vertical left black lenticular bracket (【)
    '︼', // U+FE3C vertical right black lenticular bracket (】)
    '︽', // U+FE3D vertical left double angle bracket (《)
    '︾', // U+FE3E vertical right double angle bracket (》)
    '︿', // U+FE3F vertical left angle bracket (〈)
    '﹀', // U+FE40 vertical right angle bracket (〉)
    '﹁', // U+FE41 vertical left corner bracket (「, “)
    '﹂', // U+FE42 vertical right corner bracket (」, ”)
    '﹃', // U+FE43 vertical left white corner bracket (『, ‘)
    '﹄', // U+FE44 vertical right white corner bracket (』, ’)
    '﹇', // U+FE47 vertical left square bracket (［)
    '﹈', // U+FE48 vertical right square bracket (］)
];

/// Bitmask of [`VERT_FORMS`] present in a font face's `cmap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct VertForms(u32);

impl VertForms {
    /// Read vertical-form coverage from raw TTF/OTF/TTC font bytes.
    pub(super) fn from_font(font: &[u8], index: u32) -> VertForms {
        let face_off = if font.get(0..4) == Some(b"ttcf") {
            let n = be32(font, 8).unwrap_or(0);
            if index >= n {
                return VertForms(0);
            }
            be32(font, 12 + index as usize * 4).unwrap_or(0) as usize
        } else {
            0
        };
        let Some(n) = be16(font, face_off + 4) else {
            return VertForms(0);
        };
        let mut cmap = None;
        for i in 0..n as usize {
            let r = face_off + 12 + i * 16;
            if font.get(r..r + 4) == Some(b"cmap") {
                if let (Some(o), Some(l)) = (be32(font, r + 8), be32(font, r + 12)) {
                    cmap = font.get(o as usize..o as usize + l as usize);
                }
                break;
            }
        }
        let Some(cmap) = cmap else {
            return VertForms(0);
        };
        let mut mask = 0u32;
        for (i, &ch) in VERT_FORMS.iter().enumerate() {
            if cmap_has(cmap, ch as u32) {
                mask |= 1 << i;
            }
        }
        VertForms(mask)
    }

    pub(super) fn has(self, ch: char) -> bool {
        VERT_FORMS
            .iter()
            .position(|&c| c == ch)
            .is_some_and(|i| self.0 & (1 << i) != 0)
    }
}

/// Cached vertical-form coverage for the bundled LXGW WenKai font.
pub(super) fn wenkai_vert() -> VertForms {
    static VERT: OnceLock<VertForms> = OnceLock::new();
    *VERT.get_or_init(|| VertForms::from_font(crate::png_report::WENKAI, 0))
}

/// What the reader's text uses: an egui family for each of serif and
/// sans-serif, regular and bold.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Installed {
    pub(super) serif: FontFamily,
    pub(super) serif_bold: FontFamily,
    pub(super) sans: FontFamily,
    pub(super) sans_bold: FontFamily,
    /// The family has no bold face: bold is drawn twice, a little apart.
    pub(super) serif_faux_bold: bool,
    pub(super) sans_faux_bold: bool,
    /// Vertical presentation forms present in each primary face.
    pub(super) serif_vert: VertForms,
    pub(super) sans_vert: VertForms,
    /// The two choices this was made for.
    pub(super) key: (String, String),
}

/// Default choices, by what the system has.
pub(super) fn default_choice(fams: &[FamilyInfo], serif: bool, traditional: bool) -> String {
    let wanted: &[&str] = match (serif, traditional) {
        (true, false) => &[
            "Source Han Serif SC",
            "Noto Serif CJK SC",
            "Noto Serif SC",
            "Songti SC",
            "STSong",
            "SimSun",
        ],
        (true, true) => &[
            "Source Han Serif TC",
            "Noto Serif CJK TC",
            "Noto Serif TC",
            "Songti TC",
            "PMingLiU",
            "MingLiU",
        ],
        (false, false) => &[
            "Source Han Sans SC",
            "Noto Sans CJK SC",
            "Noto Sans SC",
            "Microsoft YaHei",
            "PingFang SC",
            "Heiti SC",
        ],
        (false, true) => &[
            "Source Han Sans TC",
            "Noto Sans CJK TC",
            "Noto Sans TC",
            "Microsoft JhengHei",
            "PingFang TC",
            "Heiti TC",
            "Microsoft YaHei",
        ],
    };
    wanted
        .iter()
        .find(|w| fams.iter().any(|f| f.name.eq_ignore_ascii_case(w)))
        .map(|w| w.to_string())
        .unwrap_or_else(|| {
            if serif {
                BUNDLED.to_owned()
            } else {
                String::new()
            }
        })
}

/// Load the regular upright face of family `name` from `fams` as a font
/// blob + collection face index, memory-mapped (with bitmap tables hidden).
pub(super) fn load_family_face(
    fams: &[FamilyInfo],
    name: &str,
) -> Option<(egui::epaint::text::Blob, u32)> {
    let fam = fams.iter().find(|f| f.name.eq_ignore_ascii_case(name))?;
    let face = fam.face(400)?;
    let b = blob(face)?;
    Some((b, face.index))
}

fn blob(face: &Face) -> Option<egui::epaint::text::Blob> {
    match &face.path {
        None => Some(Arc::new(crate::png_report::WENKAI) as egui::epaint::text::Blob),
        Some(p) => {
            let f = File::open(p).ok()?;
            // SAFETY: font files are not changed while the app runs; a
            // replaced file keeps the mapped inode on every platform.
            let m = unsafe { memmap2::Mmap::map(&f) }.ok()?;
            if !bitmap_tables(&m) {
                return Some(Arc::new(m) as egui::epaint::text::Blob);
            }
            // A private copy-on-write mapping: only the table directory's
            // page is copied, and the file is never written.
            // SAFETY: as above.
            let mut c = unsafe { memmap2::MmapOptions::new().map_copy(&f) }.ok()?;
            hide_bitmap_tables(&mut c);
            match c.make_read_only() {
                Ok(m) => Some(Arc::new(m) as egui::epaint::text::Blob),
                Err(_) => None,
            }
        }
    }
}

/// Embedded bitmap tables (`EBDT`, `EBLC`, `EBSC`): black-and-white glyph
/// bitmaps for small sizes, as in SimSun, MingLiU, MS Gothic and Gulim.
const BITMAP_TABLES: [&[u8; 4]; 3] = [b"EBDT", b"EBLC", b"EBSC"];

/// The table records (offset of each record) of every face in a font file.
fn table_records(font: &[u8]) -> Vec<usize> {
    let faces: Vec<usize> = if font.get(0..4) == Some(b"ttcf") {
        let n = be32(font, 8).unwrap_or(0).min(64) as usize;
        (0..n)
            .filter_map(|i| be32(font, 12 + i * 4).map(|o| o as usize))
            .collect()
    } else {
        vec![0]
    };
    let mut out = Vec::new();
    for off in faces {
        let n = be16(font, off + 4).unwrap_or(0) as usize;
        for i in 0..n {
            let r = off + 12 + i * 16;
            if r + 16 <= font.len() {
                out.push(r);
            }
        }
    }
    out
}

/// Whether any face of `font` has an embedded bitmap table.
fn bitmap_tables(font: &[u8]) -> bool {
    table_records(font)
        .into_iter()
        .any(|r| BITMAP_TABLES.iter().any(|t| font[r..r + 4] == t[..]))
}

/// Hide the embedded bitmap tables from egui, which takes any font with an
/// `EBDT` table for a colour font: it draws each glyph that has a bitmap as
/// an image in fixed white, never in the text's colour, so SimSun's Chinese
/// is white on every theme. Renamed (`EBDT` → `EBDt`), the tables are
/// unknown to egui and it draws the outlines. Lowercasing the last letter
/// keeps the directory sorted, which table lookups rely on. Fonts without
/// such tables are left as they are. Returns whether anything changed.
pub(crate) fn hide_bitmap_tables(font: &mut [u8]) -> bool {
    let mut changed = false;
    for r in table_records(font) {
        if BITMAP_TABLES.iter().any(|t| font[r..r + 4] == t[..]) {
            font[r + 3] = font[r + 3].to_ascii_lowercase();
            changed = true;
        }
    }
    changed
}

fn installed_key() -> egui::Id {
    egui::Id::unique("reader-fonts-installed")
}

/// Install `serif` and `sans` (family names; empty: the app's own font) as
/// new egui families named with a fresh generation, so text laid out before the
/// change never mixes with the new fonts. Reuses the already-installed families
/// when reopening a book with unchanged choices, skipping a font-atlas rebuild.
pub(super) fn install(
    ctx: &egui::Context,
    fams: &[FamilyInfo],
    serif: &str,
    sans: &str,
) -> Installed {
    if let Some(prev) = ctx.data(|d| d.get_temp::<Installed>(installed_key()))
        && prev.key.0 == serif
        && prev.key.1 == sans
        && ctx.fonts(|v| v.definitions().families.contains_key(&prev.serif))
    {
        return prev;
    }
    static GEN: AtomicU64 = AtomicU64::new(0);
    let generation = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let (defs, installed) = definitions(fams, serif, sans, generation);
    ctx.set_fonts(defs);
    ctx.data_mut(|d| d.insert_temp(installed_key(), installed.clone()));
    installed
}

/// The app's fonts plus the reader's families (see [`install`]).
pub(super) fn definitions(
    fams: &[FamilyInfo],
    serif: &str,
    sans: &str,
    generation: u64,
) -> (FontDefinitions, Installed) {
    let mut defs = BASE.get().cloned().unwrap_or_default();
    let fallback: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallback_vert = wenkai_vert();
    // Share a single mmap when regular/bold or serif/sans come from the same file.
    let mut mapped: HashMap<PathBuf, egui::epaint::text::Blob> = HashMap::new();
    let mut load_blob = |face: &Face| -> Option<egui::epaint::text::Blob> {
        match &face.path {
            None => Some(Arc::new(crate::png_report::WENKAI) as egui::epaint::text::Blob),
            Some(p) => {
                if let Some(b) = mapped.get(p) {
                    return Some(b.clone());
                }
                let b = blob(face)?;
                mapped.insert(p.clone(), b.clone());
                Some(b)
            }
        }
    };
    let mut add = |slot: &str, choice: &str| -> (FontFamily, FontFamily, bool, VertForms) {
        let regular_name = format!("rd{generation}-{slot}");
        let bold_name = format!("rd{generation}-{slot}-b");
        let fam = fams.iter().find(|f| f.name.eq_ignore_ascii_case(choice));
        let mut reg_list = Vec::new();
        let mut bold_list = Vec::new();
        let mut faux = true;
        let mut vert = fallback_vert;
        if let Some(fam) = fam {
            if let Some(face) = fam.face(400)
                && let Some(b) = load_blob(face)
            {
                vert = if face.path.is_none() {
                    fallback_vert
                } else {
                    VertForms::from_font(b.as_ref().as_ref(), face.index)
                };
                let key = format!("{regular_name}-r");
                defs.font_data
                    .insert(key.clone(), Arc::new(FontData::from_blob(b, face.index)));
                reg_list.push(key.clone());
                bold_list.push(key);
            }
            if fam.has_bold()
                && let Some(face) = fam.face(700)
                && let Some(b) = load_blob(face)
            {
                let key = format!("{regular_name}-b");
                defs.font_data
                    .insert(key.clone(), Arc::new(FontData::from_blob(b, face.index)));
                bold_list.insert(0, key);
                bold_list.truncate(1);
                faux = false;
            }
        }
        reg_list.extend(fallback.iter().cloned());
        bold_list.extend(fallback.iter().cloned());
        let r = FontFamily::Name(regular_name.into());
        let b = FontFamily::Name(bold_name.into());
        defs.families.insert(r.clone(), reg_list);
        defs.families.insert(b.clone(), bold_list);
        (r, b, faux, vert)
    };
    let (serif_f, serif_b, serif_faux, serif_vert) = add("serif", serif);
    let (sans_f, sans_b, sans_faux, sans_vert) = add("sans", sans);
    let installed = Installed {
        serif: serif_f,
        serif_bold: serif_b,
        sans: sans_f,
        sans_bold: sans_b,
        serif_faux_bold: serif_faux,
        sans_faux_bold: sans_faux,
        serif_vert,
        sans_vert,
        key: (serif.to_owned(), sans.to_owned()),
    };
    (defs, installed)
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(windows)]
    fn finds_system_cjk_fonts() {
        let fams = super::scan();
        let yahei = fams.iter().find(|f| f.name == "Microsoft YaHei");
        assert!(
            yahei.is_some(),
            "{:?}",
            fams.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert!(yahei.unwrap().has_bold());
        assert!(fams.iter().all(|f| f.name != "Arial"));
    }

    /// A font file's table directory with `tags`, one face or a collection.
    fn directory(faces: &[&[&[u8; 4]]]) -> Vec<u8> {
        let mut out = Vec::new();
        let ttc = faces.len() > 1;
        let head = if ttc { 12 + 4 * faces.len() } else { 0 };
        if ttc {
            out.extend(b"ttcf");
            out.extend(0x0001_0000u32.to_be_bytes());
            out.extend((faces.len() as u32).to_be_bytes());
        }
        let mut dirs = Vec::new();
        for tags in faces {
            let at = head + dirs.len();
            if ttc {
                out.extend((at as u32).to_be_bytes());
            }
            dirs.extend(0x0001_0000u32.to_be_bytes());
            dirs.extend((tags.len() as u16).to_be_bytes());
            dirs.extend([0; 6]);
            for t in *tags {
                dirs.extend(*t);
                dirs.extend([0; 12]);
            }
        }
        out.extend(dirs);
        out
    }

    fn tags(font: &[u8]) -> Vec<String> {
        super::table_records(font)
            .into_iter()
            .map(|r| String::from_utf8_lossy(&font[r..r + 4]).into_owned())
            .collect()
    }

    #[test]
    fn hides_bitmap_tables_keeping_the_directory_sorted() {
        let face: &[&[u8; 4]] = &[
            b"DSIG", b"EBDT", b"EBLC", b"EBSC", b"GDEF", b"OS/2", b"glyf",
        ];
        let plain: &[&[u8; 4]] = &[b"OS/2", b"cmap", b"glyf"];
        let mut f = directory(&[face, plain]);
        assert!(super::bitmap_tables(&f));
        assert!(super::hide_bitmap_tables(&mut f));
        let t = tags(&f);
        assert_eq!(
            t,
            [
                "DSIG", "EBDt", "EBLc", "EBSc", "GDEF", "OS/2", "glyf", "OS/2", "cmap", "glyf"
            ]
        );
        assert!(t[..7].is_sorted() && t[7..].is_sorted());
        assert!(!super::bitmap_tables(&f));
        assert!(!super::hide_bitmap_tables(&mut f));

        let mut p = directory(&[plain]);
        let before = p.clone();
        assert!(!super::hide_bitmap_tables(&mut p));
        assert_eq!(p, before);
    }

    /// SimSun's bitmaps are hidden; YaHei, which has none, is not touched.
    #[test]
    #[cfg(windows)]
    fn system_fonts_with_and_without_bitmaps() {
        let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("Fonts");
        let mut simsun = std::fs::read(dir.join("simsun.ttc")).unwrap();
        assert!(super::bitmap_tables(&simsun));
        assert!(super::hide_bitmap_tables(&mut simsun));
        assert!(!super::bitmap_tables(&simsun));
        let mut yahei = std::fs::read(dir.join("msyh.ttc")).unwrap();
        let before = yahei.clone();
        assert!(!super::hide_bitmap_tables(&mut yahei));
        assert!(yahei == before);
    }
}
