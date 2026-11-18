//! The installed fonts that can show Chinese, for the reader's serif and
//! sans-serif choices, and their installation into egui.
//!
//! Fonts are found by reading the tables of every font file in the system's
//! font folders (`name` for the family, `OS/2` for weight and style, `cmap`
//! for Chinese coverage): a few small reads a file, done once in the
//! background. Chosen faces are memory-mapped, not read.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct Face {
    /// `None`: the bundled LXGW WenKai.
    pub path: Option<PathBuf>,
    pub index: u32,
    pub weight: u16,
    pub italic: bool,
}

#[derive(Clone, Debug)]
pub struct FamilyInfo {
    /// The English (or first) family name: what settings store.
    pub name: String,
    /// The Chinese name, if the font has one that differs.
    pub local: Option<String>,
    pub faces: Vec<Face>,
}

impl FamilyInfo {
    pub fn label(&self) -> String {
        match &self.local {
            Some(l) => format!("{} · {l}", self.name),
            None => self.name.clone(),
        }
    }

    /// The upright face closest to `weight`.
    pub fn face(&self, weight: u16) -> Option<&Face> {
        self.faces
            .iter()
            .filter(|f| !f.italic)
            .min_by_key(|f| (f.weight as i32 - weight as i32).abs())
            .or(self.faces.first())
    }

    pub fn has_bold(&self) -> bool {
        self.faces.iter().any(|f| !f.italic && f.weight >= 600)
    }
}

pub const BUNDLED: &str = "LXGW WenKai";

/// The fonts found, once the background scan is done.
pub fn families() -> Option<Arc<Vec<FamilyInfo>>> {
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
pub fn prefetch() {
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
pub fn set_base(defs: FontDefinitions) {
    let _ = BASE.set(defs);
}

/// What the reader's text uses: an egui family for each of serif and
/// sans-serif, regular and bold.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub serif: FontFamily,
    pub serif_bold: FontFamily,
    pub sans: FontFamily,
    pub sans_bold: FontFamily,
    /// The family has no bold face: bold is drawn twice, a little apart.
    pub serif_faux_bold: bool,
    pub sans_faux_bold: bool,
    /// The two choices this was made for.
    pub key: (String, String),
}

/// Default choices, by what the system has.
pub fn default_choice(fams: &[FamilyInfo], serif: bool, traditional: bool) -> String {
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

fn blob(face: &Face) -> Option<egui::epaint::text::Blob> {
    match &face.path {
        None => Some(Arc::new(crate::png_report::WENKAI) as egui::epaint::text::Blob),
        Some(p) => {
            let f = File::open(p).ok()?;
            // SAFETY: font files are not changed while the app runs; a
            // replaced file keeps the mapped inode on every platform.
            let m = unsafe { memmap2::Mmap::map(&f) }.ok()?;
            Some(Arc::new(m) as egui::epaint::text::Blob)
        }
    }
}

/// Install `serif` and `sans` (family names; empty: the app's own font) as
/// new egui families named with `generation`, so text laid out before the
/// change never mixes with the new fonts. Takes effect at the next pass.
pub fn install(
    ctx: &egui::Context,
    fams: &[FamilyInfo],
    serif: &str,
    sans: &str,
    generation: u64,
) -> Installed {
    let (defs, installed) = definitions(fams, serif, sans, generation);
    ctx.set_fonts(defs);
    installed
}

/// The app's fonts plus the reader's families (see [`install`]).
pub fn definitions(
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
    let mut add = |slot: &str, choice: &str| -> (FontFamily, FontFamily, bool) {
        let regular_name = format!("rd{generation}-{slot}");
        let bold_name = format!("rd{generation}-{slot}-b");
        let fam = fams.iter().find(|f| f.name.eq_ignore_ascii_case(choice));
        let mut reg_list = Vec::new();
        let mut bold_list = Vec::new();
        let mut faux = true;
        if let Some(fam) = fam {
            if let Some(face) = fam.face(400)
                && let Some(b) = blob(face)
            {
                let key = format!("{regular_name}-r");
                defs.font_data
                    .insert(key.clone(), Arc::new(FontData::from_blob(b, face.index)));
                reg_list.push(key.clone());
                bold_list.push(key);
            }
            if fam.has_bold()
                && let Some(face) = fam.face(700)
                && let Some(b) = blob(face)
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
        (r, b, faux)
    };
    let (serif_f, serif_b, serif_faux) = add("serif", serif);
    let (sans_f, sans_b, sans_faux) = add("sans", sans);
    let installed = Installed {
        serif: serif_f,
        serif_bold: serif_b,
        sans: sans_f,
        sans_bold: sans_b,
        serif_faux_bold: serif_faux,
        sans_faux_bold: sans_faux,
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
}
