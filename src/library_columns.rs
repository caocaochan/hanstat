//! The Library's Books columns: which are shown and in what order, the
//! user's own columns and their values, and the filters. Kept in
//! `library-columns.json` in the app folder (through [`hanstat::saver`], so
//! an unreadable file is set aside, never overwritten). The table itself is
//! drawn by `library_books.rs`.
//!
//! Book (with 📕 Read) is always first and ✔ Read and ✖ always last; the
//! columns between them can be reordered and hidden, stat columns added and
//! removed, and the user's own columns made and deleted. Default columns
//! can be hidden but not removed.

use hanstat::search;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

const FILE: &str = "library-columns.json";

/// A column of the Books table. Its name is saved (the sort, in `UiState`;
/// the layout and filters, as [`HistCol::key`]): never rename a variant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum HistCol {
    Title,
    Language,
    Unique,
    Known,
    Chars,
    Words,
    Time,
    Analysed,
    Style,
    Read,
    Difficulty,
    // Stat columns, added from the Columns window.
    Progress,
    UniqueChars,
    Words98,
    KnownUnique,
    Unknown,
    KnownChars,
    Segmenter,
    Format,
    Sections,
    TimeRead,
    Speed,
    FirstRead,
    LastRead,
    Finished,
    Series,
    /// The user's own column of this id.
    Field(u32),
}

/// The movable default columns, in their default order.
pub(crate) const DEFAULTS: [HistCol; 9] = [
    HistCol::Language,
    HistCol::Chars,
    HistCol::Words,
    HistCol::Unique,
    HistCol::Known,
    HistCol::Difficulty,
    HistCol::Style,
    HistCol::Time,
    HistCol::Analysed,
];

/// The stat columns, in the Add menu's order.
pub(crate) const STATS: [HistCol; 15] = [
    HistCol::Progress,
    HistCol::Series,
    HistCol::UniqueChars,
    HistCol::Words98,
    HistCol::KnownUnique,
    HistCol::Unknown,
    HistCol::KnownChars,
    HistCol::TimeRead,
    HistCol::Speed,
    HistCol::FirstRead,
    HistCol::LastRead,
    HistCol::Finished,
    HistCol::Segmenter,
    HistCol::Format,
    HistCol::Sections,
];

/// What kind of filter a column takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FilterKind {
    /// Min and max, in the column's unit.
    Range,
    /// A tick list of the values present.
    Choice,
    /// Dates: in the last N days, more than N days ago, from–to.
    Age,
    /// Contains, with the search box's matcher.
    Text,
}

impl HistCol {
    /// As saved in the columns file: the variant's name, `Field:3` for a
    /// user column.
    pub(crate) fn key(self) -> String {
        match self {
            HistCol::Field(id) => format!("Field:{id}"),
            c => format!("{c:?}"),
        }
    }

    /// The column a [`HistCol::key`] names; `None` for one this build
    /// doesn't know (a newer build's).
    pub(crate) fn from_key(k: &str) -> Option<HistCol> {
        if let Some(id) = k.strip_prefix("Field:") {
            return id.parse().ok().map(HistCol::Field);
        }
        [HistCol::Title, HistCol::Read]
            .into_iter()
            .chain(DEFAULTS)
            .chain(STATS)
            .find(|c| format!("{c:?}") == k)
    }

    pub(crate) fn is_stat(self) -> bool {
        STATS.contains(&self)
    }

    /// Numbers and dates sort largest first on the first click, text A–Z.
    pub(crate) fn first_desc(self, fields: &[Field]) -> bool {
        match self {
            HistCol::Title
            | HistCol::Language
            | HistCol::Segmenter
            | HistCol::Format
            | HistCol::Series => false,
            HistCol::Field(id) => !matches!(field_type(fields, id), Some(FieldType::Text)),
            _ => true,
        }
    }

    pub(crate) fn filter_kind(self, fields: &[Field]) -> FilterKind {
        use HistCol::*;
        match self {
            Title | Series => FilterKind::Text,
            Language | Difficulty | Segmenter | Format => FilterKind::Choice,
            Analysed | Read | FirstRead | LastRead | Finished => FilterKind::Age,
            Field(id) => match field_type(fields, id) {
                Some(FieldType::Text) | None => FilterKind::Text,
                Some(FieldType::YesNo) => FilterKind::Choice,
                Some(FieldType::Date) => FilterKind::Age,
                Some(FieldType::Number | FieldType::Rating) => FilterKind::Range,
            },
            _ => FilterKind::Range,
        }
    }
}

fn field_type(fields: &[Field], id: u32) -> Option<FieldType> {
    fields.iter().find(|f| f.id == id).map(|f| f.ty)
}

/// The type of a user column's values.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) enum FieldType {
    Text,
    Number,
    YesNo,
    Rating,
    Date,
}

impl FieldType {
    pub(crate) const ALL: [FieldType; 5] = [
        FieldType::Text,
        FieldType::Number,
        FieldType::YesNo,
        FieldType::Rating,
        FieldType::Date,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            FieldType::Text => "Text",
            FieldType::Number => "Number",
            FieldType::YesNo => "Yes/No",
            FieldType::Rating => "Rating \u{e46a}",
            FieldType::Date => "Date",
        }
    }
}

/// A user column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Field {
    /// Never reused, so values of a deleted column can't turn up in a new one.
    pub id: u32,
    pub name: String,
    pub ty: FieldType,
}

/// A value in a user column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Value {
    Text(String),
    Number(f64),
    Yes(bool),
    /// 1–5.
    Stars(u8),
    /// Days since 1970-01-01 (`reading_stats::civil`).
    Date(i32),
}

/// A row's value in a column, for sorting and filtering, in the column's
/// unit (percent, hours, …).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    /// Shown as "–".
    None,
    Num(f64),
    Text(String),
    /// A label, sorted by its rank and then the label.
    Choice(String, f64),
    /// Unix time.
    Time(f64),
    /// Days since 1970-01-01.
    Day(i32),
}

/// What rows sort by: [`Val`] made comparable. No value sorts below every
/// value, as the "–" rows did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Key {
    None,
    Num(f64),
    Text(String),
    Pair(f64, String),
}

impl Val {
    pub(crate) fn key(&self) -> Key {
        match self {
            Val::None => Key::None,
            Val::Num(x) | Val::Time(x) => Key::Num(*x),
            Val::Day(d) => Key::Num(*d as f64),
            Val::Text(s) => Key::Text(s.to_lowercase()),
            Val::Choice(s, r) => Key::Pair(*r, s.to_lowercase()),
        }
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Key {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self, o) {
            (Key::None, Key::None) => Equal,
            (Key::None, _) => Less,
            (_, Key::None) => Greater,
            (Key::Num(a), Key::Num(b)) => a.total_cmp(b),
            (Key::Text(a), Key::Text(b)) => a.cmp(b),
            (Key::Pair(r, a), Key::Pair(q, b)) => r.total_cmp(q).then_with(|| a.cmp(b)),
            // Never mixed within a column; some order all the same.
            (a, b) => a.rank().cmp(&b.rank()),
        }
    }
}

impl Key {
    fn rank(&self) -> u8 {
        match self {
            Key::None => 0,
            Key::Num(_) => 1,
            Key::Text(_) => 2,
            Key::Pair(..) => 3,
        }
    }
}

/// A column's filter. Absent from [`ColumnsFile::filters`] while every
/// book passes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Filter {
    /// Range: in the column's unit.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Choice: the labels let through; empty: all.
    pub picked: BTreeSet<String>,
    /// Text: what the value must contain (the search box's matcher).
    pub text: String,
    /// Age: in the last `within` days; more than `older` days ago; from–to
    /// (days since 1970-01-01, inclusive).
    pub within: Option<u32>,
    pub older: Option<u32>,
    pub from: Option<i32>,
    pub to: Option<i32>,
    /// Books with a value pass (when they meet the above).
    pub values: bool,
    /// Books with no value ("–") pass.
    pub blanks: bool,
    /// `blanks` was set by hand: setting a bound no longer turns it off.
    pub blanks_set: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            min: None,
            max: None,
            picked: BTreeSet::new(),
            text: String::new(),
            within: None,
            older: None,
            from: None,
            to: None,
            values: true,
            blanks: true,
            blanks_set: false,
        }
    }
}

impl Filter {
    pub(crate) fn has_bounds(&self) -> bool {
        self.min.is_some()
            || self.max.is_some()
            || !self.picked.is_empty()
            || !self.text.trim().is_empty()
            || self.within.is_some()
            || self.older.is_some()
            || self.from.is_some()
            || self.to.is_some()
    }

    /// Every book passes.
    pub(crate) fn is_all(&self) -> bool {
        !self.has_bounds() && self.values && self.blanks
    }

    /// After an edit: a bound set for the first time leaves out the books
    /// with no value, unless that was chosen by hand.
    pub(crate) fn settle(&mut self, had_bounds: bool) {
        if !had_bounds && self.has_bounds() && !self.blanks_set {
            self.blanks = false;
        }
        if had_bounds && !self.has_bounds() && !self.blanks_set {
            self.blanks = true;
        }
    }

    /// Whether a row with value `v` passes. `today` and `day_of` (Unix time
    /// to day number) are for the date bounds.
    #[cfg(test)]
    pub(crate) fn passes(&self, v: &Val, today: i32, day_of: &dyn Fn(f64) -> i32) -> bool {
        let q = matches!(v, Val::Text(_))
            .then(|| (!self.text.trim().is_empty()).then(|| search::Query::new(&self.text)))
            .flatten();
        self.passes_with(v, today, day_of, q.as_ref())
    }

    /// Same as [`passes`], reusing a pre-parsed `query` for [`Val::Text`] across rows.
    pub(crate) fn passes_with(
        &self,
        v: &Val,
        today: i32,
        day_of: &dyn Fn(f64) -> i32,
        query: Option<&search::Query>,
    ) -> bool {
        if *v == Val::None {
            return self.blanks;
        }
        if !self.values {
            return false;
        }
        match v {
            Val::None => unreachable!(),
            Val::Num(x) => self.min.is_none_or(|m| *x >= m) && self.max.is_none_or(|m| *x <= m),
            Val::Text(s) => match query {
                Some(q) => q.matches(std::iter::once(s.as_str())),
                None => true,
            },
            Val::Choice(s, _) => self.picked.is_empty() || self.picked.contains(s),
            Val::Time(t) => self.day_passes(day_of(*t), today),
            Val::Day(d) => self.day_passes(*d, today),
        }
    }

    fn day_passes(&self, d: i32, today: i32) -> bool {
        let ago = today - d;
        self.within.is_none_or(|n| ago <= n as i32)
            && self.older.is_none_or(|n| ago > n as i32)
            && self.from.is_none_or(|f| d >= f)
            && self.to.is_none_or(|t| d <= t)
    }
}

/// What is saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ColumnsFile {
    /// The columns between the pinned ones, in order: ([`HistCol::key`], shown).
    pub order: Vec<(String, bool)>,
    pub fields: Vec<Field>,
    pub next_id: u32,
    /// By book id, then field id.
    pub values: BTreeMap<String, BTreeMap<u32, Value>>,
    /// Active filters, by [`HistCol::key`].
    pub filters: Vec<(String, Filter)>,
}

/// The columns in use, read from [`ColumnsFile`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Columns {
    /// Between the pinned ones, in order, with whether each is shown.
    pub order: Vec<(HistCol, bool)>,
    pub fields: Vec<Field>,
    pub next_id: u32,
    pub values: BTreeMap<String, BTreeMap<u32, Value>>,
    pub filters: Vec<(HistCol, Filter)>,
}

impl Default for Columns {
    fn default() -> Self {
        Columns::from_file(ColumnsFile::default())
    }
}

impl Columns {
    /// Unknown columns, duplicates and deleted fields dropped, missing
    /// defaults put back at their default place.
    pub(crate) fn from_file(f: ColumnsFile) -> Columns {
        let fields: Vec<Field> = {
            let mut seen = BTreeSet::new();
            f.fields.into_iter().filter(|x| seen.insert(x.id)).collect()
        };
        let next_id = f
            .next_id
            .max(fields.iter().map(|x| x.id + 1).max().unwrap_or(0));
        let known = |c: HistCol| match c {
            HistCol::Field(id) => fields.iter().any(|x| x.id == id),
            HistCol::Title | HistCol::Read => false,
            _ => true,
        };
        let mut order: Vec<(HistCol, bool)> = Vec::new();
        for (k, shown) in f.order {
            if let Some(c) = HistCol::from_key(&k)
                && known(c)
                && !order.iter().any(|(o, _)| *o == c)
            {
                order.push((c, shown));
            }
        }
        // Fields not in the order (it was lost): at the end, hidden.
        for x in &fields {
            let c = HistCol::Field(x.id);
            if !order.iter().any(|(o, _)| *o == c) {
                order.push((c, false));
            }
        }
        for (k, d) in DEFAULTS.iter().enumerate() {
            if order.iter().any(|(o, _)| o == d) {
                continue;
            }
            // After the nearest default before it that is there.
            let at = DEFAULTS[..k]
                .iter()
                .rev()
                .find_map(|p| order.iter().position(|(o, _)| o == p))
                .map_or(0, |i| i + 1);
            order.insert(at, (*d, true));
        }
        let mut filters: Vec<(HistCol, Filter)> = Vec::new();
        for (k, fl) in f.filters {
            if let Some(c) = HistCol::from_key(&k)
                && (known(c) || matches!(c, HistCol::Title | HistCol::Read))
                && !filters.iter().any(|(o, _)| *o == c)
                && !fl.is_all()
            {
                filters.push((c, fl));
            }
        }
        Columns {
            order,
            fields,
            next_id,
            values: f.values,
            filters,
        }
    }

    pub(crate) fn to_file(&self) -> ColumnsFile {
        ColumnsFile {
            order: self.order.iter().map(|(c, s)| (c.key(), *s)).collect(),
            fields: self.fields.clone(),
            next_id: self.next_id,
            values: self.values.clone(),
            filters: self
                .filters
                .iter()
                .map(|(c, f)| (c.key(), f.clone()))
                .collect(),
        }
    }

    pub(crate) fn load() -> Columns {
        hanstat::saver::load_json::<ColumnsFile>(&path())
            .map(Columns::from_file)
            .unwrap_or_default()
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.to_file()).map_err(|e| e.to_string())?;
        hanstat::saver::write(&path(), bytes)
    }

    /// Shown columns between the pinned ones, in order.
    pub(crate) fn shown(&self) -> impl Iterator<Item = HistCol> + '_ {
        self.order.iter().filter(|(_, s)| *s).map(|(c, _)| *c)
    }

    pub(crate) fn is_shown(&self, c: HistCol) -> bool {
        matches!(c, HistCol::Title | HistCol::Read) || self.order.iter().any(|(o, s)| *o == c && *s)
    }

    pub(crate) fn field(&self, id: u32) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// A column's name, as its header shows it.
    pub(crate) fn name(&self, c: HistCol) -> String {
        match c {
            HistCol::Field(id) => self
                .field(id)
                .map_or_else(|| "?".to_owned(), |f| f.name.clone()),
            c => name(c).to_owned(),
        }
    }

    /// Defaults shown, in their order; stat columns removed; the user's
    /// own columns kept (with their values), hidden, at the end.
    pub(crate) fn reset(&mut self) {
        let mut order: Vec<(HistCol, bool)> = DEFAULTS.iter().map(|c| (*c, true)).collect();
        order.extend(self.fields.iter().map(|f| (HistCol::Field(f.id), false)));
        self.order = order;
        self.filters.retain(|(c, _)| !c.is_stat());
    }

    /// Add a stat column, shown, at the end.
    pub(crate) fn add_stat(&mut self, c: HistCol) {
        if c.is_stat() && !self.order.iter().any(|(o, _)| *o == c) {
            self.order.push((c, true));
        }
    }

    /// Take a stat column away (it goes back into the Add menu).
    pub(crate) fn remove_stat(&mut self, c: HistCol) {
        if c.is_stat() {
            self.order.retain(|(o, _)| *o != c);
            self.filters.retain(|(o, _)| *o != c);
        }
    }

    /// A new user column, shown, at the end; its id.
    pub(crate) fn add_field(&mut self, name: &str, ty: FieldType) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.fields.push(Field {
            id,
            name: name.trim().to_owned(),
            ty,
        });
        self.order.push((HistCol::Field(id), true));
        id
    }

    /// Whether `name` is free for a user column (case ignored).
    pub(crate) fn name_free(&self, name: &str, except: Option<u32>) -> bool {
        let n = name.trim().to_lowercase();
        !n.is_empty()
            && !DEFAULTS
                .iter()
                .chain(STATS.iter())
                .chain([HistCol::Title, HistCol::Read].iter())
                .any(|c| self::name(*c).to_lowercase() == n)
            && !self
                .fields
                .iter()
                .any(|f| Some(f.id) != except && f.name.to_lowercase() == n)
    }

    /// Delete a user column: what is needed to put it back.
    pub(crate) fn delete_field(&mut self, id: u32) -> Option<Deleted> {
        let at = self
            .order
            .iter()
            .position(|(c, _)| *c == HistCol::Field(id))?;
        let (_, shown) = self.order.remove(at);
        let fi = self.fields.iter().position(|f| f.id == id)?;
        let field = self.fields.remove(fi);
        let mut values = Vec::new();
        for (book, vals) in &mut self.values {
            if let Some(v) = vals.remove(&id) {
                values.push((book.clone(), v));
            }
        }
        self.values.retain(|_, v| !v.is_empty());
        let filter = self
            .filters
            .iter()
            .position(|(c, _)| *c == HistCol::Field(id))
            .map(|i| self.filters.remove(i).1);
        Some(Deleted {
            field,
            at,
            shown,
            field_at: fi,
            values,
            filter,
        })
    }

    /// Put back a deleted column, with its values.
    pub(crate) fn restore(&mut self, d: Deleted) {
        let id = d.field.id;
        self.fields
            .insert(d.field_at.min(self.fields.len()), d.field);
        self.order
            .insert(d.at.min(self.order.len()), (HistCol::Field(id), d.shown));
        for (book, v) in d.values {
            self.values.entry(book).or_default().insert(id, v);
        }
        if let Some(f) = d.filter {
            self.filters.push((HistCol::Field(id), f));
        }
    }

    pub(crate) fn value(&self, book: &str, field: u32) -> Option<&Value> {
        self.values.get(book)?.get(&field)
    }

    /// Set or (with `None`) clear a value.
    pub(crate) fn set_value(&mut self, book: &str, field: u32, v: Option<Value>) {
        match v {
            Some(v) => {
                self.values
                    .entry(book.to_owned())
                    .or_default()
                    .insert(field, v);
            }
            None => {
                if let Some(vals) = self.values.get_mut(book) {
                    vals.remove(&field);
                    if vals.is_empty() {
                        self.values.remove(book);
                    }
                }
            }
        }
    }

    /// Book `old` is now `new` (its file was rebuilt): its values follow.
    pub(crate) fn rekey(&mut self, old: &str, new: &str) -> bool {
        match self.values.remove(old) {
            Some(v) => {
                self.values.insert(new.to_owned(), v);
                true
            }
            None => false,
        }
    }

    /// A book is gone for good.
    pub(crate) fn forget(&mut self, book: &str) -> bool {
        self.values.remove(book).is_some()
    }

    /// The text of the book's Text columns, for the search box.
    pub(crate) fn texts<'a>(&'a self, book: &str) -> impl Iterator<Item = &'a str> + 'a {
        let fields = &self.fields;
        self.values.get(book).into_iter().flat_map(move |vals| {
            vals.iter().filter_map(move |(id, v)| match v {
                Value::Text(s)
                    if fields
                        .iter()
                        .any(|f| f.id == *id && f.ty == FieldType::Text) =>
                {
                    Some(s.as_str())
                }
                _ => None,
            })
        })
    }

    pub(crate) fn filter(&self, c: HistCol) -> Option<&Filter> {
        self.filters.iter().find(|(o, _)| *o == c).map(|(_, f)| f)
    }

    /// Set a column's filter; one that lets every book through is dropped.
    pub(crate) fn set_filter(&mut self, c: HistCol, f: Filter) {
        let at = self.filters.iter().position(|(o, _)| *o == c);
        match (at, f.is_all()) {
            (Some(i), true) => {
                self.filters.remove(i);
            }
            (Some(i), false) => self.filters[i].1 = f,
            (None, false) => self.filters.push((c, f)),
            (None, true) => {}
        }
    }

    /// Move the column at `from` (in [`Columns::order`]) to `to`.
    pub(crate) fn move_to(&mut self, from: usize, to: usize) {
        if from < self.order.len() {
            let c = self.order.remove(from);
            self.order.insert(to.min(self.order.len()), c);
        }
    }
}

/// A user column deleted, while it can be put back.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Deleted {
    pub field: Field,
    at: usize,
    shown: bool,
    field_at: usize,
    pub values: Vec<(String, Value)>,
    filter: Option<Filter>,
}

fn path() -> std::path::PathBuf {
    hanstat::models::app_dir().join(FILE)
}

/// The built-in columns' names.
pub(crate) fn name(c: HistCol) -> &'static str {
    use HistCol::*;
    match c {
        Title => "Book",
        Language => "Language",
        Chars => "Characters",
        Words => "Words",
        Unique => "Unique words",
        Known => "Known",
        Difficulty => "Difficulty",
        Style => "Writing style",
        Time => "Reading",
        Analysed => "Analysed",
        Read => "Read",
        Progress => "Progress",
        UniqueChars => "Unique characters",
        Words98 => "Words for 98%",
        KnownUnique => "Known unique words",
        Unknown => "Unknown words",
        KnownChars => "Known unique characters",
        Segmenter => "Segmenter",
        Format => "Format",
        Sections => "Sections",
        TimeRead => "Time read",
        Speed => "Reading speed",
        FirstRead => "First read",
        LastRead => "Last read",
        Finished => "Finished",
        Series => "Series",
        Field(_) => "",
    }
}

/// The built-in columns' explanations (header hover and the Columns window).
pub(crate) fn hover(c: HistCol) -> &'static str {
    use HistCol::*;
    match c {
        Title => "The book's title, or its file name",
        Language => "Simplified or Traditional Chinese",
        Chars => "Chinese characters",
        Words => "Running words, as the segmenter counted them",
        Unique => "Distinct words",
        Known => {
            "Running words you knew (Anki cards and the levels and word lists marked known) at the last analysis; names are left out"
        }
        Difficulty => {
            "From the share of running words known (and the writing style, while rated), as on the Overview"
        }
        Style => {
            "How hard the prose is when you know its words, rated by a local AI model (in Settings). The percentage compares it with the Library's rated books (50% is their median); the word is the model's own judgment. Chinese books only"
        }
        Time => "Reading time at the speeds set on the Overview",
        Analysed => "When the book was last analysed",
        Read => {
            "Mark a book read to count its most frequent words as known and take it off the reading path"
        }
        Progress => {
            "How far through the book the reader is (100% once finished there or marked read)"
        }
        UniqueChars => "Distinct Chinese characters",
        Words98 => "The most frequent words needed to cover 98% of the running words",
        KnownUnique => "Share of the distinct words you knew at the last analysis",
        Unknown => "Distinct words not known at the last analysis",
        KnownChars => "Share of the distinct characters you knew at the last analysis",
        Segmenter => "The word segmenter the book was analysed with",
        Format => "EPUB or TXT",
        Sections => "Sections included in the analysis, of all the book's sections",
        TimeRead => "Time spent reading the book in the reader",
        Speed => "Characters per minute in the reader, once there is enough reading",
        FirstRead => "When the book was first read in the reader",
        LastRead => "The last day the book was read in the reader",
        Finished => "When the book was finished in the reader",
        Series => {
            "The series and volume: from the book's metadata (as calibre writes it) or else its title, when another book of the Library is in it"
        }
        Field(_) => "",
    }
}

/// Split columns of `widths` into pages, in order, each at most `room` wide
/// with `gap` between columns. A column wider than `room` gets a page of
/// its own. No columns: one empty page.
pub(crate) fn pack_pages(widths: &[f32], room: f32, gap: f32) -> Vec<Range<usize>> {
    let mut pages = Vec::new();
    let mut start = 0;
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        let need = if i == start { *w } else { used + gap + w };
        if i > start && need > room {
            pages.push(start..i);
            start = i;
            used = *w;
        } else {
            used = need;
        }
    }
    pages.push(start..widths.len());
    pages
}

/// Day number ↔ (year, month, day): see `reading_stats::civil`.
pub(crate) fn date_label(day: i32) -> String {
    let (y, m, d) = crate::reading_stats::civil(day as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(c: &Columns) -> Vec<HistCol> {
        c.order.iter().map(|(c, _)| *c).collect()
    }

    #[test]
    fn empty_file_gives_todays_columns() {
        let c = Columns::default();
        assert_eq!(keys(&c), DEFAULTS.to_vec());
        assert!(c.order.iter().all(|(_, s)| *s));
    }

    #[test]
    fn normalising_drops_unknown_and_duplicates_and_restores_defaults() {
        let f = ColumnsFile {
            order: vec![
                ("Analysed".into(), false),
                ("NewerBuildColumn".into(), true),
                ("Words".into(), true),
                ("Words".into(), false),
                ("Field:7".into(), true),
                ("Field:9".into(), true),
                ("Progress".into(), true),
            ],
            fields: vec![Field {
                id: 7,
                name: "Author".into(),
                ty: FieldType::Text,
            }],
            next_id: 0,
            values: BTreeMap::new(),
            filters: vec![
                (
                    "Field:9".into(),
                    Filter {
                        min: Some(1.0),
                        ..Filter::default()
                    },
                ),
                (
                    "Known".into(),
                    Filter {
                        min: Some(90.0),
                        ..Filter::default()
                    },
                ),
                ("Chars".into(), Filter::default()),
            ],
        };
        let c = Columns::from_file(f);
        assert_eq!(c.next_id, 8);
        // A missing default goes after the nearest default before it that
        // is there (Language, first of all, at the start); Analysed stays
        // hidden.
        let k = keys(&c);
        assert!(c.order.contains(&(HistCol::Analysed, false)));
        assert!(!k.contains(&HistCol::Field(9)));
        assert!(k.contains(&HistCol::Field(7)) && k.contains(&HistCol::Progress));
        for d in DEFAULTS {
            assert_eq!(k.iter().filter(|c| **c == d).count(), 1, "{d:?}");
        }
        assert_eq!(&k[..2], &[HistCol::Language, HistCol::Chars]);
        let w = k.iter().position(|c| *c == HistCol::Words).unwrap();
        assert_eq!(k[w + 1], HistCol::Unique);
        assert_eq!(c.filters.len(), 1);
        assert_eq!(c.filters[0].0, HistCol::Known);
    }

    #[test]
    fn file_round_trip() {
        let mut c = Columns::default();
        let id = c.add_field("Author", FieldType::Text);
        c.add_stat(HistCol::Progress);
        c.set_value("b1", id, Some(Value::Text("刘慈欣".into())));
        c.set_value("b1", 99, Some(Value::Date(20_000)));
        c.set_filter(
            HistCol::Known,
            Filter {
                min: Some(95.0),
                ..Filter::default()
            },
        );
        let json = serde_json::to_string(&c.to_file()).unwrap();
        let back = Columns::from_file(serde_json::from_str(&json).unwrap());
        assert_eq!(back, c);
    }

    #[test]
    fn values_of_every_type_round_trip() {
        for v in [
            Value::Text("x".into()),
            Value::Number(2.5),
            Value::Yes(true),
            Value::Stars(4),
            Value::Date(-3),
        ] {
            let j = serde_json::to_string(&v).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&j).unwrap(), v);
        }
    }

    #[test]
    fn sort_keys_put_no_value_lowest() {
        let mut v = [
            Val::Num(3.0).key(),
            Val::None.key(),
            Val::Num(-1.0).key(),
            Val::Num(10.0).key(),
        ];
        v.sort();
        assert_eq!(v[0], Key::None);
        assert_eq!(v[1], Key::Num(-1.0));
        let mut t = [
            Val::Text("b".into()).key(),
            Val::Text("A".into()).key(),
            Val::None.key(),
        ];
        t.sort();
        assert_eq!(t, [Key::None, Key::Text("a".into()), Key::Text("b".into())]);
        // Difficulty: by rank, not by label.
        assert!(Val::Choice("Hard".into(), 1.0).key() < Val::Choice("Easy".into(), 2.0).key());
    }

    #[test]
    fn filters_by_kind() {
        let day = |_t: f64| 0;
        let range = Filter {
            min: Some(90.0),
            blanks: false,
            ..Filter::default()
        };
        assert!(range.passes(&Val::Num(95.0), 0, &day));
        assert!(!range.passes(&Val::Num(80.0), 0, &day));
        assert!(!range.passes(&Val::None, 0, &day));
        let text = Filter {
            text: "三體".into(),
            ..Filter::default()
        };
        assert!(text.passes(&Val::Text("三体2".into()), 0, &day));
        assert!(!text.passes(&Val::Text("球状闪电".into()), 0, &day));
        let choice = Filter {
            picked: ["Traditional".to_owned()].into(),
            ..Filter::default()
        };
        assert!(choice.passes(&Val::Choice("Traditional".into(), 0.0), 0, &day));
        assert!(!choice.passes(&Val::Choice("Simplified".into(), 0.0), 0, &day));
        let age = Filter {
            within: Some(7),
            ..Filter::default()
        };
        assert!(age.passes(&Val::Day(95), 100, &day));
        assert!(!age.passes(&Val::Day(90), 100, &day));
        let older = Filter {
            older: Some(30),
            from: Some(0),
            ..Filter::default()
        };
        assert!(older.passes(&Val::Day(10), 100, &day));
        assert!(!older.passes(&Val::Day(80), 100, &day));
        assert!(!older.passes(&Val::Day(-5), 100, &day));
        // ✔ Read = not read: no values, only blanks.
        let not_read = Filter {
            values: false,
            ..Filter::default()
        };
        assert!(not_read.passes(&Val::None, 0, &day));
        assert!(!not_read.passes(&Val::Time(5.0), 0, &day));
    }

    #[test]
    fn first_bound_leaves_out_blanks_unless_chosen() {
        let mut f = Filter {
            min: Some(1.0),
            ..Filter::default()
        };
        f.settle(false);
        assert!(!f.blanks);
        f.min = None;
        f.settle(true);
        assert!(f.blanks && f.is_all());
        let mut g = Filter {
            blanks_set: true,
            ..Filter::default()
        };
        g.max = Some(3.0);
        g.settle(false);
        assert!(g.blanks);
    }

    #[test]
    fn delete_and_restore_a_field() {
        let mut c = Columns::default();
        let id = c.add_field("Author", FieldType::Text);
        c.set_value("b1", id, Some(Value::Text("x".into())));
        c.set_value("b2", id, Some(Value::Text("y".into())));
        c.set_filter(
            HistCol::Field(id),
            Filter {
                text: "x".into(),
                ..Filter::default()
            },
        );
        let before = c.clone();
        let d = c.delete_field(id).unwrap();
        assert_eq!(d.values.len(), 2);
        assert!(c.values.is_empty() && c.filters.is_empty());
        assert!(!c.order.iter().any(|(o, _)| *o == HistCol::Field(id)));
        c.restore(d);
        assert_eq!(c, before);
        // Ids are never reused.
        let d = c.delete_field(id).unwrap();
        drop(d);
        assert_ne!(c.add_field("Genre", FieldType::Text), id);
    }

    #[test]
    fn rekey_and_forget_values() {
        let mut c = Columns::default();
        let id = c.add_field("N", FieldType::Number);
        c.set_value("old", id, Some(Value::Number(3.0)));
        assert!(c.rekey("old", "new"));
        assert_eq!(c.value("new", id), Some(&Value::Number(3.0)));
        assert!(c.value("old", id).is_none());
        assert!(c.forget("new"));
        assert!(c.values.is_empty());
    }

    #[test]
    fn texts_are_only_text_columns() {
        let mut c = Columns::default();
        let a = c.add_field("Author", FieldType::Text);
        let n = c.add_field("Pages", FieldType::Number);
        c.set_value("b", a, Some(Value::Text("刘慈欣".into())));
        c.set_value("b", n, Some(Value::Number(300.0)));
        assert_eq!(c.texts("b").collect::<Vec<_>>(), vec!["刘慈欣"]);
    }

    #[test]
    fn reset_keeps_fields_hidden() {
        let mut c = Columns::default();
        let id = c.add_field("Author", FieldType::Text);
        c.add_stat(HistCol::Series);
        c.order[0].1 = false;
        c.move_to(3, 0);
        c.reset();
        let mut want: Vec<(HistCol, bool)> = DEFAULTS.iter().map(|d| (*d, true)).collect();
        want.push((HistCol::Field(id), false));
        assert_eq!(c.order, want);
    }

    #[test]
    fn names_must_be_free() {
        let mut c = Columns::default();
        c.add_field("Author", FieldType::Text);
        assert!(!c.name_free("author", None));
        assert!(!c.name_free("Known", None));
        assert!(!c.name_free("  ", None));
        assert!(c.name_free("Genre", None));
        assert!(c.name_free("Author", Some(0)));
    }

    #[test]
    fn pages_pack_in_order() {
        assert_eq!(pack_pages(&[], 100.0, 4.0), vec![0..0]);
        assert_eq!(pack_pages(&[30.0, 30.0, 30.0], 100.0, 4.0), vec![0..3]);
        // 30 + 4 + 30 + 4 + 30 = 98: fits; one more doesn't.
        assert_eq!(pack_pages(&[30.0; 4], 98.0, 4.0), vec![0..3, 3..4]);
        assert_eq!(pack_pages(&[30.0; 4], 97.9, 4.0), vec![0..2, 2..4]);
        // Too wide alone: a page of its own.
        assert_eq!(
            pack_pages(&[50.0, 200.0, 50.0], 100.0, 4.0),
            vec![0..1, 1..2, 2..3]
        );
    }

    #[test]
    fn keys_round_trip() {
        for c in DEFAULTS.into_iter().chain(STATS).chain([
            HistCol::Title,
            HistCol::Read,
            HistCol::Field(12),
        ]) {
            assert_eq!(HistCol::from_key(&c.key()), Some(c));
        }
        assert_eq!(HistCol::from_key("Nope"), None);
    }

    /// A `UiState` sort saved by the build before these columns still reads.
    #[test]
    fn old_sort_still_reads() {
        let (c, d): (HistCol, bool) = serde_json::from_str(r#"["Analysed",true]"#).unwrap();
        assert_eq!((c, d), (HistCol::Analysed, true));
        let s = serde_json::to_string(&(HistCol::Field(3), false)).unwrap();
        assert_eq!(
            serde_json::from_str::<(HistCol, bool)>(&s).unwrap(),
            (HistCol::Field(3), false)
        );
    }

    #[test]
    fn dates_label_around_1970_and_leap_days() {
        assert_eq!(date_label(0), "1970-01-01");
        assert_eq!(date_label(-1), "1969-12-31");
        let leap = crate::reading_stats::days_from_civil(2024, 2, 29) as i32;
        assert_eq!(date_label(leap), "2024-02-29");
        assert_eq!(date_label(leap + 1), "2024-03-01");
    }
}
