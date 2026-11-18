//! The reader's Dictionaries window: the imported Yomitan dictionaries
//! (turn on and off, reorder, delete), importing more (a file dialog or
//! zips dropped on the reader), their updates, and the popup's settings.

use super::Settings;
use super::popup::{LookupKey, Popup};
use eframe::egui::{self, Color32, RichText};
use hanstat::dict::format::{CheckResult, Entry, Meta, VERSION};
use hanstat::dict::jobs::{Jobs, State, Task, check_updates};
use hanstat::dict::manage;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Automatic update checks at most this often (seconds).
const CHECK_EVERY: u64 = 24 * 3600;

pub struct Manager {
    pub open: bool,
    root: PathBuf,
    rows: Vec<(Entry, Option<Meta>)>,
    jobs: Jobs,
    checking: Option<Receiver<usize>>,
    confirm_delete: Option<String>,
    /// What the last finished jobs or checks said.
    notes: Vec<(bool, String)>,
    /// Install the updates the running check finds.
    install_found: bool,
}

fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn fmt_size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.0} MB", (b as f64 / (1u64 << 20) as f64).max(0.1))
    }
}

/// Dictionary updates off, as app updates are, with `HANSTAT_NO_UPDATE`.
fn updates_off() -> bool {
    std::env::var_os("HANSTAT_NO_UPDATE").is_some()
}

impl Manager {
    pub fn new(ctx: &egui::Context, popup: &Popup) -> Manager {
        let root = hanstat::dict::root();
        let c = ctx.clone();
        let jobs = Jobs::start(
            root.clone(),
            popup.unloader(),
            true,
            Arc::new(move || c.request_repaint()),
        );
        Manager {
            open: false,
            rows: manage::list(&root),
            root,
            jobs,
            checking: None,
            confirm_delete: None,
            notes: Vec::new(),
            install_found: false,
        }
    }

    fn refresh(&mut self) {
        self.rows = manage::list(&self.root);
    }

    /// What finished jobs and checks said: (ok, text).
    #[cfg(test)]
    pub(super) fn notes(&self) -> &[(bool, String)] {
        &self.notes
    }

    /// The dictionaries listed: (id, title).
    #[cfg(test)]
    pub(super) fn listed(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .map(|(e, m)| {
                (
                    e.id.clone(),
                    m.as_ref().map_or(String::new(), |m| m.title.clone()),
                )
            })
            .collect()
    }

    /// Dictionaries with an update found by the last check.
    pub fn updates(&self) -> usize {
        self.rows
            .iter()
            .filter(|(e, _)| {
                matches!(
                    e.check.as_ref().map(|c| &c.result),
                    Some(CheckResult::Available { .. })
                )
            })
            .count()
    }

    /// An import or download running (or waiting).
    pub fn working(&self) -> bool {
        self.jobs.busy()
    }

    pub fn busy(&self) -> bool {
        self.jobs.busy() || self.checking.is_some()
    }

    /// Check for updates now (in the background); `install`: install what
    /// is found.
    pub fn check(&mut self, ctx: &egui::Context, install: bool) {
        if self.checking.is_some() || updates_off() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let root = self.root.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check_updates(&root, true));
            c.request_repaint();
        });
        self.checking = Some(rx);
        self.install_found = install;
    }

    /// With no dictionary at all, CC-CEDICT is downloaded (once ever: a
    /// deleted one stays deleted).
    pub fn install_default(&mut self) {
        if updates_off() || !self.rows.is_empty() {
            return;
        }
        let mut first = false;
        let _ = manage::edit_manifest(&self.root, |m| {
            first = m.dicts.is_empty() && !m.default_installed;
            m.default_installed = true;
        });
        if first {
            let (title, url) = hanstat::dict::jobs::DEFAULT;
            self.jobs.add(
                title.to_owned(),
                Task::Install {
                    url: url.to_owned(),
                },
            );
        }
    }

    /// The daily check, when the reader opens.
    pub fn auto_check(&mut self, ctx: &egui::Context, settings: &Settings) {
        if !settings.check_updates || updates_off() {
            return;
        }
        let last = hanstat::dict::format::Manifest::load(&self.root).last_auto_check;
        if hanstat::dict::format::now().saturating_sub(last) >= CHECK_EVERY
            && self
                .rows
                .iter()
                .any(|(_, m)| m.as_ref().is_some_and(|m| m.update.is_some()))
        {
            self.check(ctx, settings.auto_install);
        }
    }

    pub fn import(&mut self, zips: Vec<PathBuf>) {
        for z in zips {
            let label = z.file_name().map_or_else(
                || z.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            self.jobs.add(
                label,
                Task::Import {
                    zip: z,
                    replace: None,
                },
            );
        }
    }

    fn update(&mut self, id: &str) {
        let Some((e, m)) = self.rows.iter().find(|(e, _)| e.id == id) else {
            return;
        };
        let title = m.as_ref().map_or(id.to_owned(), |m| m.title.clone());
        let (url, revision) = match e.check.as_ref().map(|c| &c.result) {
            Some(CheckResult::Available {
                revision,
                download_url,
            }) => (download_url.clone(), revision.clone()),
            // Another version's files: the same revision downloaded again.
            _ => match m.as_ref().and_then(|m| m.update.clone()) {
                Some(u) => (u.download_url, String::new()),
                None => return,
            },
        };
        if self
            .jobs
            .list
            .iter()
            .any(|j| j.label == title && !matches!(j.state(), State::Done(_)))
        {
            return;
        }
        self.jobs.add(
            title,
            Task::Update {
                id: id.to_owned(),
                url,
                revision,
            },
        );
    }

    fn update_all(&mut self) {
        let ids: Vec<String> = self
            .rows
            .iter()
            .filter(|(e, _)| {
                matches!(
                    e.check.as_ref().map(|c| &c.result),
                    Some(CheckResult::Available { .. })
                )
            })
            .map(|(e, _)| e.id.clone())
            .collect();
        for id in ids {
            self.update(&id);
        }
    }

    /// Take in finished jobs and checks; reload the popup's dictionaries
    /// after any change. Call every frame.
    pub fn poll(&mut self, ctx: &egui::Context, popup: &Popup, settings: &Settings) {
        let done = self.jobs.take_finished();
        if !done.is_empty() {
            for (label, r) in done {
                self.notes.push(match r {
                    Ok(m) => (true, m),
                    Err(e) => (false, format!("{label}: {e}")),
                });
            }
            self.refresh();
            popup.reload(settings.freq_sort.clone());
        }
        if let Some(rx) = &self.checking
            && let Ok(n) = rx.try_recv()
        {
            self.checking = None;
            self.refresh();
            let failed = self
                .rows
                .iter()
                .filter(|(e, _)| {
                    matches!(
                        e.check.as_ref().map(|c| &c.result),
                        Some(CheckResult::Failed(_))
                    )
                })
                .count();
            self.notes.push((
                failed == 0,
                match (n, failed) {
                    (0, 0) => "Every dictionary is up to date.".into(),
                    (0, f) => format!("No updates found; {f} could not be checked."),
                    (n, 0) => format!("{n} update{} available.", if n == 1 { "" } else { "s" }),
                    (n, f) => format!(
                        "{n} update{} available; {f} could not be checked.",
                        if n == 1 { "" } else { "s" }
                    ),
                },
            ));
            if self.install_found && n > 0 {
                self.update_all();
            }
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// Change the list (on/off, order), then reload the popup's dictionaries.
    pub(super) fn edit(
        &mut self,
        popup: &Popup,
        settings: &Settings,
        f: impl FnOnce(&mut hanstat::dict::format::Manifest),
    ) {
        if let Err(e) = manage::edit_manifest(&self.root, f) {
            self.notes.push((false, e));
        }
        self.refresh();
        popup.reload(settings.freq_sort.clone());
    }

    pub(super) fn delete(&mut self, id: &str, popup: &mut Popup, settings: &Settings) {
        popup.close();
        let r = popup
            .unloader()
            .unload(vec![id.to_owned()])
            .and_then(|_| manage::delete(&self.root, id));
        if let Err(e) = r {
            self.notes.push((false, e));
        }
        self.refresh();
        popup.reload(settings.freq_sort.clone());
    }

    /// The window. Returns whether the popup's settings changed.
    pub fn ui(&mut self, ctx: &egui::Context, popup: &mut Popup, settings: &mut Settings) -> bool {
        if !self.open {
            return false;
        }
        let before = (
            settings.lookup_key,
            settings.freq_sort.clone(),
            settings.show_freq,
            settings.check_updates,
            settings.auto_install,
            settings.popup_size,
        );
        let mut open = self.open;
        let mut action: Option<Action> = None;
        egui::Window::new("Dictionaries")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .max_height(ctx.content_rect().height() * 0.85)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("Import…")
                        .on_hover_text("Yomitan dictionaries (.zip); you can also drop them on the reader")
                        .clicked()
                    {
                        action = Some(Action::Pick);
                    }
                    let can_check = !updates_off() && self.checking.is_none();
                    let check = ui
                        .add_enabled(can_check, egui::Button::new(if self.checking.is_some() { "Checking…" } else { "Check for updates" }))
                        .on_hover_text("Asks each dictionary's site for its latest revision")
                        .on_disabled_hover_text(if updates_off() { "HANSTAT_NO_UPDATE is set: updates are off." } else { "Checking…" });
                    if check.clicked() {
                        action = Some(Action::Check);
                    }
                    let n = self.updates();
                    if n > 1 && ui.button(format!("Update all ({n})")).clicked() {
                        action = Some(Action::UpdateAll);
                    }
                });
                ui.add_space(4.0);
                if self.rows.is_empty() {
                    ui.label(
                        RichText::new(
                            "No dictionaries yet. Import Yomitan dictionaries (.zip files): term dictionaries for the \
                             popup's definitions, frequency lists for its ranks.",
                        )
                        .color(super::theme::BASE00),
                    );
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    egui::Grid::new("dict-rows")
                        .num_columns(9)
                        .spacing([10.0, 6.0])
                        .striped(true)
                        .show(ui, |ui| {
                            let n = self.rows.len();
                            if n > 0 {
                                let head = |ui: &mut egui::Ui, t: &str| {
                                    ui.label(RichText::new(t).size(12.0).color(super::theme::BASE1));
                                };
                                for t in ["On", "", "Dictionary", "Kind", "Entries", "Size", "Updates", "Collapsed", ""] {
                                    head(ui, t);
                                }
                                ui.end_row();
                            }
                            for (i, (e, m)) in self.rows.iter().enumerate() {
                                let mut on = e.enabled;
                                if ui.checkbox(&mut on, "").on_hover_text("Use in the popup").changed() {
                                    action = Some(Action::Enable(e.id.clone(), on));
                                }
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(i > 0, egui::Button::new("▲").small()).on_hover_text("Earlier in the popup").clicked() {
                                        action = Some(Action::Move(i, i - 1));
                                    }
                                    if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).on_hover_text("Later in the popup").clicked() {
                                        action = Some(Action::Move(i, i + 1));
                                    }
                                });
                                match m {
                                    Some(m) => {
                                        let title = ui.label(RichText::new(&m.title).strong());
                                        let mut tip = format!("Revision {}", m.revision);
                                        for (k, v) in [("By", &m.author), ("", &m.description), ("", &m.attribution)] {
                                            if let Some(v) = v {
                                                tip.push_str(&format!("\n{}{v}", if k.is_empty() { String::new() } else { format!("{k}: ") }));
                                            }
                                        }
                                        title.on_hover_text(tip);
                                        ui.label(match (m.has_terms(), m.has_freq()) {
                                            (true, true) => "Terms, frequency",
                                            (false, true) => "Frequency",
                                            _ => "Terms",
                                        });
                                        ui.label(fmt_count(m.terms.max(m.freqs)));
                                        ui.label(fmt_size(m.bytes)).on_hover_text("On disk");
                                        // Update status.
                                        let busy_here = self.jobs.list.iter().find(|j| j.label == m.title && !matches!(j.state(), State::Done(_)));
                                        if busy_here.is_some() {
                                            ui.label(RichText::new("Updating…").color(super::theme::BASE00));
                                        } else if m.version != VERSION {
                                            if m.update.is_some() {
                                                if ui.button("Import again").on_hover_text("Made by another version of hanstat: downloaded again from its site").clicked() {
                                                    action = Some(Action::Update(e.id.clone()));
                                                }
                                            } else {
                                                ui.label(RichText::new("Import its zip again").color(Color32::from_rgb(0xcb, 0x4b, 0x16)))
                                                    .on_hover_text("Made by another version of hanstat");
                                            }
                                        } else {
                                            match e.check.as_ref().map(|c| &c.result) {
                                                Some(CheckResult::Available { revision, .. }) => {
                                                    if ui
                                                        .button(format!("Update to {revision}"))
                                                        .on_hover_text(format!("Installed: {}", m.revision))
                                                        .clicked()
                                                    {
                                                        action = Some(Action::Update(e.id.clone()));
                                                    }
                                                }
                                                Some(CheckResult::UpToDate) => {
                                                    ui.label(RichText::new("Up to date").color(super::theme::BASE1));
                                                }
                                                Some(CheckResult::Failed(err)) => {
                                                    ui.label(RichText::new("Check failed").color(Color32::from_rgb(0xcb, 0x4b, 0x16))).on_hover_text(err);
                                                }
                                                None if m.update.is_none() => {
                                                    ui.label(RichText::new("No updates").color(super::theme::BASE1))
                                                        .on_hover_text("This dictionary doesn't say where updates are published");
                                                }
                                                None => {
                                                    ui.label("");
                                                }
                                            }
                                        }
                                    }
                                    None => {
                                        ui.label(RichText::new("Unreadable").color(Color32::from_rgb(0xcb, 0x4b, 0x16)));
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                    }
                                }
                                if m.as_ref().is_some_and(|m| m.has_terms()) {
                                    let mut c = e.collapsed;
                                    if ui
                                        .checkbox(&mut c, "")
                                        .on_hover_text("Its entries start collapsed in the popup (click its bar there to open them)")
                                        .changed()
                                    {
                                        action = Some(Action::Collapse(e.id.clone(), c));
                                    }
                                } else {
                                    ui.label("");
                                }
                                if self.confirm_delete.as_deref() == Some(&e.id) {
                                    ui.horizontal(|ui| {
                                        if ui.button(RichText::new("Delete").color(Color32::from_rgb(0xdc, 0x32, 0x2f))).clicked() {
                                            action = Some(Action::Delete(e.id.clone()));
                                        }
                                        if ui.button("Keep").clicked() {
                                            action = Some(Action::KeepIt);
                                        }
                                    });
                                } else if ui.button("🗑").on_hover_text("Delete this dictionary").clicked() {
                                    action = Some(Action::AskDelete(e.id.clone()));
                                }
                                ui.end_row();
                            }
                        });
                });
                // Jobs.
                for j in &self.jobs.list {
                    ui.horizontal(|ui| {
                        let (text, frac) = match j.state() {
                            State::Waiting => ("waiting".to_owned(), 0.0),
                            State::Downloading { done, total } => (
                                format!("downloading {}", total.map_or(fmt_size(done), |t| format!("{} of {}", fmt_size(done), fmt_size(t)))),
                                total.map_or(0.0, |t| done as f32 / t.max(1) as f32),
                            ),
                            State::Importing => ("importing".to_owned(), j.progress.fraction()),
                            State::Done(_) => ("done".to_owned(), 1.0),
                        };
                        ui.label(&j.label);
                        ui.add(egui::ProgressBar::new(frac).desired_width(220.0).text(text));
                        if ui.small_button("Cancel").clicked() {
                            j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    });
                }
                for (ok, n) in self.notes.iter().rev().take(4) {
                    ui.label(RichText::new(n).color(if *ok { super::theme::BASE00 } else { Color32::from_rgb(0xdc, 0x32, 0x2f) }));
                }
                for e in &popup.errors {
                    ui.label(RichText::new(e).color(Color32::from_rgb(0xdc, 0x32, 0x2f)));
                }
                ui.separator();
                egui::Grid::new("dict-settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Look up with");
                    ui.horizontal(|ui| {
                        for k in LookupKey::ALL {
                            ui.radio_value(&mut settings.lookup_key, k, k.label());
                        }
                    })
                    .response
                    .on_hover_text("Hold it with the pointer on a word (or, with no key, rest the pointer on it)");
                    ui.end_row();
                    let lists: Vec<(String, String)> = popup
                        .dicts
                        .iter()
                        .filter(|d| d.has_freq)
                        .map(|d| (d.id.clone(), d.title.clone()))
                        .collect();
                    ui.label("Order by");
                    let shown = settings
                        .freq_sort
                        .as_ref()
                        .and_then(|id| lists.iter().find(|(i, _)| i == id))
                        .map_or("Dictionary order".to_owned(), |(_, t)| format!("{t} ranks"));
                    egui::ComboBox::from_id_salt("dict-freq-sort").selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.freq_sort, None, "Dictionary order");
                        for (id, t) in &lists {
                            ui.selectable_value(&mut settings.freq_sort, Some(id.clone()), format!("{t} ranks"));
                        }
                    })
                    .response
                    .on_hover_text("Which reading or word comes first when several match as long");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut settings.show_freq, "Show frequency ranks");
                    ui.end_row();
                    ui.label("Popup size");
                    ui.horizontal(|ui| {
                        ui.label(match settings.popup_size {
                            Some([w, h]) => format!("{w:.0} × {h:.0}"),
                            None => "Default".to_owned(),
                        })
                        .on_hover_text("Drag the popup's corner to resize it; longer entries scroll inside it (also with ↑ ↓)");
                        if ui
                            .add_enabled(settings.popup_size.is_some(), egui::Button::new("Reset"))
                            .clicked()
                        {
                            settings.popup_size = None;
                        }
                    });
                    ui.end_row();
                    ui.label("Updates");
                    ui.vertical(|ui| {
                        ui.add_enabled(!updates_off(), egui::Checkbox::new(&mut settings.check_updates, "Check once a day"));
                        ui.add_enabled(!updates_off(), egui::Checkbox::new(&mut settings.auto_install, "Install what is found without asking"));
                        ui.label(
                            RichText::new("Checks ask each dictionary's own site (most are on GitHub) for its latest revision.")
                                .size(12.0)
                                .color(super::theme::BASE1),
                        );
                    });
                    ui.end_row();
                });
            });
        self.open = open;
        match action {
            Some(Action::Pick) => {
                if let Some(files) = rfd::FileDialog::new()
                    .set_title("Import Yomitan dictionaries")
                    .add_filter("Yomitan dictionary", &["zip"])
                    .pick_files()
                {
                    self.import(files);
                }
            }
            Some(Action::Check) => self.check(ctx, false),
            Some(Action::UpdateAll) => self.update_all(),
            Some(Action::Update(id)) => self.update(&id),
            Some(Action::Enable(id, on)) => self.edit(popup, settings, |m| {
                if let Some(e) = m.dicts.iter_mut().find(|e| e.id == id) {
                    e.enabled = on;
                }
            }),
            Some(Action::Collapse(id, c)) => self.edit(popup, settings, |m| {
                if let Some(e) = m.dicts.iter_mut().find(|e| e.id == id) {
                    e.collapsed = c;
                }
            }),
            Some(Action::Move(a, b)) => self.edit(popup, settings, |m| {
                if a < m.dicts.len() && b < m.dicts.len() {
                    m.dicts.swap(a, b);
                }
            }),
            Some(Action::AskDelete(id)) => self.confirm_delete = Some(id),
            Some(Action::KeepIt) => self.confirm_delete = None,
            Some(Action::Delete(id)) => {
                self.confirm_delete = None;
                self.delete(&id, popup, settings);
            }
            None => {}
        }
        let after = (
            settings.lookup_key,
            settings.freq_sort.clone(),
            settings.show_freq,
            settings.check_updates,
            settings.auto_install,
            settings.popup_size,
        );
        if after.1 != before.1 {
            popup.reload(settings.freq_sort.clone());
        }
        after != before
    }
}

enum Action {
    Pick,
    Check,
    UpdateAll,
    Update(String),
    Enable(String, bool),
    Collapse(String, bool),
    Move(usize, usize),
    AskDelete(String),
    KeepIt,
    Delete(String),
}
