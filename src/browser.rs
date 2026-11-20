//! The mission list: every mission DLL in the game folder and the folders under it (OPU keeps its maps a few levels
//! down), read in the background so the window stays responsive.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align, RichText};

use crate::inspect;

/// How deep under the game folder to look for DLLs.
const MAX_DEPTH: usize = 5;

/// The mission type dropdown: a label and the start of the kinds it shows (`inspect`'s names; empty shows all).
const TYPES: [(&str, &str); 11] = [
    ("All types", ""),
    ("Campaign", "campaign"),
    ("Colony", "colony"),
    ("Tutorial", "tutorial"),
    ("Auto demo", "auto demo"),
    ("Multiplayer", "multiplayer"),
    ("  Land Rush", "multiplayer: Land Rush"),
    ("  Space Race", "multiplayer: Space Race"),
    ("  Resource Race", "multiplayer: Resource Race"),
    ("  Midas", "multiplayer: Midas"),
    ("  Last One Standing", "multiplayer: Last One Standing"),
];

/// One mission in the list.
pub struct Entry {
    /// The DLL's full path, as opened.
    pub path: PathBuf,
    /// The path under the game folder, for the filter and the second line.
    pub rel: String,
    /// The mission's level description, or `rel` when it has none.
    pub title: String,
    /// `inspect`'s mission type name (e.g. "colony", "multiplayer: Land Rush"), which the type dropdown matches.
    pub kind: String,
    /// The map file the mission names.
    pub map: String,
}

#[derive(Default)]
/// The mission browser window's list, filled from a background scan of the game folder.
pub struct Browser {
    /// Every mission found so far, in the scan's order (by path, a folder's files before its folders).
    pub entries: Vec<Entry>,
    /// The scan thread's entries; None once it has finished.
    scanning: Option<Receiver<Entry>>,
    /// The folder listed (or being read).
    root: Option<PathBuf>,
    /// The text filter, matched against any field ignoring case.
    filter: String,
    /// The `TYPES` entry chosen.
    kind: usize,
    /// Scroll the list to the open mission on the next frame.
    reveal: bool,
}

impl Browser {
    /// Starts reading `root`'s missions, unless that's the folder already listed.
    pub fn scan(&mut self, ctx: &egui::Context, root: &Path) {
        if self.root.as_deref() == Some(root) {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.entries.clear();
        let (tx, rx) = channel();
        let (root, ctx) = (root.to_path_buf(), ctx.clone());
        std::thread::spawn(move || {
            let mut dlls = Vec::new();
            find_dlls(&root, 0, &mut dlls);
            // OPU keeps a copy of every original mission under OPU/base/maps: list identical files once, at the first
            // path found (the game folder's own, as files come before folders).
            let mut seen = std::collections::HashSet::new();
            for path in dlls {
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let mut h = std::hash::DefaultHasher::new();
                std::hash::Hash::hash(&bytes, &mut h);
                if !seen.insert(std::hash::Hasher::finish(&h)) {
                    continue;
                }
                // No game folder here: the map isn't needed for the list.
                let Ok(r) = inspect::inspect(&path, None) else { continue };
                let Some(d) = r.desc.filter(|_| r.not_a_mission.is_none()) else { continue };
                let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                let title = if d.level_desc.is_empty() { rel.clone() } else { d.level_desc };
                if tx.send(Entry { path, rel, title, kind: r.kind.unwrap_or_default(), map: d.map_name }).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        self.scanning = Some(rx);
    }

    /// Still reading the folder.
    pub fn busy(&mut self) -> bool {
        self.poll();
        self.scanning.is_some()
    }

    /// Scrolls to the open mission on the next frame.
    pub fn reveal(&mut self) {
        self.reveal = true;
    }

    /// Takes the entries the scan has found so far.
    fn poll(&mut self) {
        let Some(rx) = &self.scanning else { return };
        loop {
            match rx.try_recv() {
                Ok(e) => self.entries.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        self.scanning = None;
    }

    /// Whether `e` passes the type dropdown and the text filter (any of its fields, ignoring case).
    fn matches(&self, e: &Entry) -> bool {
        let f = self.filter.to_lowercase();
        e.kind.starts_with(TYPES[self.kind].1)
            && (f.is_empty() || [&e.title, &e.rel, &e.kind, &e.map].iter().any(|s| s.to_lowercase().contains(&f)))
    }

    /// The mission `step` places after (or before) `current` in the filtered list.
    pub fn next(&self, current: Option<&Path>, step: isize) -> Option<PathBuf> {
        let shown: Vec<&Entry> = self.entries.iter().filter(|e| self.matches(e)).collect();
        let at = current.and_then(|c| shown.iter().position(|e| e.path == c));
        // Clamped at both ends; with no current mission in the list, Down starts at the top and Up at the bottom.
        let i = match at {
            Some(i) => (i as isize + step).clamp(0, shown.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => shown.len().checked_sub(1)?,
        };
        shown.get(i).map(|e| e.path.clone())
    }

    /// The list; returns the mission clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui, current: Option<&Path>) -> Option<PathBuf> {
        self.poll();
        let mut open = None;
        ui.heading("Missions");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("mission type").selected_text(TYPES[self.kind].0.trim()).show_ui(ui, |ui| {
                for (i, (label, _)) in TYPES.iter().enumerate() {
                    ui.selectable_value(&mut self.kind, i, *label);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter: name, file, type, map").desired_width(f32::INFINITY));
        });
        let shown = self.entries.iter().filter(|e| self.matches(e)).count();
        let status = match (&self.scanning, &self.root) {
            (_, None) => "No game folder".to_string(),
            (Some(_), _) => format!("Reading... {} missions so far", self.entries.len()),
            (None, _) if shown == self.entries.len() => format!("{} missions  (Up/Down to step through)", shown),
            (None, _) => format!("{shown} of {} missions", self.entries.len()),
        };
        ui.label(RichText::new(status).small().weak());
        ui.separator();
        // Kept until the open mission shows up in the list (it may still be being read).
        let (reveal, mut revealed) = (self.reveal, false);
        if self.scanning.is_none() {
            self.reveal = false;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for e in self.entries.iter().filter(|e| self.matches(e)) {
                let selected = current == Some(e.path.as_path());
                let text = format!("{}\n{}  ·  {}  ·  {}", e.title, e.rel, e.kind, e.map);
                let r = ui.selectable_label(selected, RichText::new(text).size(12.0));
                if r.clicked() {
                    open = Some(e.path.clone());
                }
                if selected && reveal {
                    r.scroll_to_me(Some(Align::Center));
                    revealed = true;
                }
            }
        });
        if revealed {
            self.reveal = false;
        }
        open
    }
}

/// The `.dll` files under `dir`, sorted by path, folders after files.
fn find_dlls(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
    paths.sort_by_key(|p| (p.is_dir(), p.to_string_lossy().to_lowercase()));
    for p in paths {
        if p.is_dir() {
            if depth < MAX_DEPTH {
                find_dlls(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
            out.push(p);
        }
    }
}
