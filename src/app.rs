//! The window: a menu bar, the mission list and the mission's details on the left, the map in the middle, the hovered
//! tile at the bottom.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, RichText, vec2};
use op2_formats::GameFiles;
use op2_formats::render::MapTilesets;

use crate::browser::Browser;
use crate::inspect::{self, Report};
use crate::mapview::{MapView, ViewState};
use crate::objects::{self, Layers};
use crate::setup::{self, Kind, Setup, StartOptions};
use crate::sprites::Art;
use crate::textures::ArtTextures;

/// Saving one screenshot once the window has drawn and closing (`--screenshot`), for checking the window headlessly.
pub struct ScreenshotJob {
    /// Where the PNG goes.
    pub path: PathBuf,
    /// Frames still to draw, once the mission list is read, before the screenshot is asked for.
    frames_left: u32,
    /// Set by the screenshot callback once the file is written; the next frame closes the window.
    done: Arc<AtomicBool>,
}

impl ScreenshotJob {
    /// Saves to `path` a few frames after the map and the browser have loaded.
    pub fn new(path: PathBuf) -> Self {
        ScreenshotJob { path, frames_left: 5, done: Arc::new(AtomicBool::new(false)) }
    }
}

/// The main window's state: the game folder, the open mission, the View menu windows and what's picked in them.
pub struct App {
    /// The Outpost 2 folder maps, tilesets and art are read from; None until one is set.
    game_dir: Option<PathBuf>,
    mission: Option<Loaded>,
    /// Why the last DLL couldn't be opened, shown at the top of the side panel.
    error: Option<String>,
    /// What the last save did, shown in the status bar.
    saved: Option<String>,
    /// The longest side of a saved whole map, in pixels; None for full size (up to 32 px a tile).
    whole_map_size: Option<u32>,
    /// What the map reported last frame: the hovered tile and object, read by the status bar and the tooltip.
    view: ViewState,
    /// `--screenshot`'s pending job.
    screenshot: Option<ScreenshotJob>,
    /// The game's object art, loaded with the first mission.
    art: Option<ArtTextures>,
    /// Why the art didn't load; also stops it being tried again for every mission.
    art_error: Option<String>,
    /// What the map draws (the View menu's checkboxes and the hidden owners).
    layers: Layers,
    browser: Browser,
    /// The View menu's windows, each open while its flag is set.
    show_browser: bool,
    show_objectives: bool,
    show_research: bool,
    /// Help > About, a window of its own.
    show_about: bool,
    /// The About window's banner, loaded the first time it opens.
    about_banner: Option<egui::TextureHandle>,
    /// Outline the triggers' areas on the map.
    show_areas: bool,
    /// The trigger picked in the objectives window (its id).
    selected_trigger: Option<u32>,
    /// Whose research the research window shows.
    research_player: i32,
    show_groups: bool,
    /// The AI group picked in the groups window (its id), outlined on the map.
    selected_group: Option<u32>,
    /// Draw every building group's plans, not just the selected group's.
    all_plans: bool,
    show_disasters: bool,
    /// The disaster forecast for the open mission, run when the disasters window first needs it.
    forecast: Option<crate::background::Job<crate::disasters::Forecast>>,
    /// How far the forecast and the message replay run, in marks; the disasters and messages windows share it.
    forecast_marks: u32,
    /// The forecast disaster picked in the window (its index), marked on the map.
    selected_disaster: Option<usize>,
    show_messages: bool,
    /// The mission's messages, read and replayed when the messages window first needs them.
    messages: Option<crate::background::Job<crate::messages::Report>>,
    /// The replayed message picked in the window (its index), marked on the map.
    selected_message: Option<usize>,
}

/// The open mission: its static facts, its map and what its setup made.
struct Loaded {
    /// What the DLL's exports and description say (`op2dll inspect`'s report).
    report: Report,
    /// None without a game folder, a map name or a readable map.
    map: Option<MapView>,
    /// Why the map couldn't be shown, or tilesets that didn't load.
    map_problems: Vec<String>,
    /// What the mission's setup created, with the choices it ran with; an error if the DLL wouldn't load.
    setup: Option<(StartOptions, Result<Setup, String>)>,
    /// The object picked in the list or on the map (its id).
    selected: Option<u32>,
    /// Scroll the object list to the selected object on the next frame (it was picked on the map).
    reveal_selected: bool,
}

impl Loaded {
    /// The setup's result, if it ran and succeeded.
    fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()?.1.as_ref().ok()
    }

    /// The setup's object with this id.
    fn object(&self, id: u32) -> Option<&setup::Object> {
        self.setup()?.objects.iter().find(|o| o.id == id)
    }
}

/// What the side panel or the map asks for.
enum Action {
    /// Picked in the list: select the object (its id) and centre the map on its file tile.
    Select(u32, (i32, i32)),
    /// Clicked on the map: select it (or nothing).
    Pick(Option<u32>),
    /// Run the setup again with other start choices (difficulty, player count).
    Rerun(StartOptions),
    /// Open this mission DLL (picked in the browser or stepped to with Up and Down).
    Open(PathBuf),
    /// Walls and tubes shown or hidden.
    Graphics,
    /// A trigger picked in the objectives window: highlight it and centre the map on its area.
    Trigger(u32),
    /// An AI group picked in the groups window: outline it and centre the map on it.
    Group(u32),
    /// A forecast disaster picked in the disasters window: mark it and centre the map on it.
    Disaster(usize),
    /// A replayed message picked in the messages window: mark its place and centre the map there.
    Message(usize),
}

impl App {
    /// Sets up the window: the game folder, then `open` (a DLL to load), `center` (a file tile to show at 100%) and
    /// `screenshot` (save and quit) from the command line.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        game_dir: Option<PathBuf>,
        open: Option<PathBuf>,
        center: Option<(i32, i32)>,
        screenshot: Option<ScreenshotJob>,
    ) -> Self {
        let mut app = App {
            game_dir,
            mission: None,
            error: None,
            saved: None,
            whole_map_size: None,
            view: ViewState::default(),
            screenshot,
            art: None,
            art_error: None,
            layers: Layers::default(),
            browser: Browser::default(),
            show_browser: true,
            show_objectives: false,
            show_research: false,
            show_about: false,
            about_banner: None,
            show_areas: false,
            selected_trigger: None,
            research_player: 0,
            show_groups: false,
            selected_group: None,
            all_plans: false,
            show_disasters: false,
            forecast: None,
            forecast_marks: crate::disasters::DEFAULT_MARKS,
            selected_disaster: None,
            show_messages: false,
            messages: None,
            selected_message: None,
        };
        if let Some(dir) = app.game_dir.clone() {
            app.browser.scan(&cc.egui_ctx, &dir);
        }
        if let Some(path) = open {
            app.open(&cc.egui_ctx, &path);
        }
        if let (Some(tile), Some(map)) = (center, app.mission.as_mut().and_then(|m| m.map.as_mut())) {
            map.center_full_size(tile);
        }
        app
    }

    /// Opens a DLL: reads its report, loads its map and runs its setup with the mission's default options.
    fn open(&mut self, ctx: &egui::Context, path: &Path) {
        self.error = None;
        let report = match inspect::inspect(path, self.game_dir.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("OP2MissionReader - {name}")));
        let (map, map_problems) = match (&report.desc, &self.game_dir) {
            (Some(desc), Some(dir)) if !desc.map_name.is_empty() => load_map(ctx, dir, &desc.map_name),
            (Some(_), None) => (None, vec!["no game folder, so no map (File > Set game folder)".into()]),
            _ => (None, Vec::new()),
        };
        let start = report.desc.as_ref().map(|d| StartOptions::default_for(d.mission_type, d.num_players));
        self.mission = Some(Loaded { report, map, map_problems, setup: None, selected: None, reveal_selected: false });
        // Each mission has its own players and triggers.
        self.layers.hidden_owners.clear();
        self.selected_trigger = None;
        self.selected_group = None;
        if let Some(start) = start {
            self.run_setup(ctx, start);
        }
        self.load_art();
        self.browser.reveal();
    }

    /// Loads the object art from the game folder, once.
    fn load_art(&mut self) {
        if self.art.is_some() || self.art_error.is_some() {
            return;
        }
        let Some(dir) = &self.game_dir else { return };
        match GameFiles::open(dir).and_then(|f| Art::load(&f)) {
            Ok(a) => self.art = Some(ArtTextures::new(a)),
            Err(e) => self.art_error = Some(format!("object art: {e}")),
        }
    }

    /// Runs the open mission's InitProc with these start choices, and shows the walls, tubes and tile edits it made.
    fn run_setup(&mut self, ctx: &egui::Context, start: StartOptions) {
        let Some(m) = &mut self.mission else { return };
        self.forecast = None;
        self.selected_disaster = None;
        self.messages = None;
        self.selected_message = None;
        let result = setup::run(&m.report.file, start).map_err(|e| format!("{e:#}"));
        if let Some(map) = &mut m.map {
            map.selected = None;
        }
        m.setup = Some((start, result));
        m.selected = None;
        self.apply_graphics(ctx);
    }

    /// The map's tiles with the setup's walls, tubes and edits, or as in the file when that layer is off.
    fn apply_graphics(&mut self, ctx: &egui::Context) {
        let Some(m) = &mut self.mission else { return };
        let graphics = m.setup.as_ref().and_then(|(_, s)| s.as_ref().ok()).and_then(|s| s.graphics.as_deref());
        if let Some(map) = &mut m.map {
            map.set_graphics(ctx, graphics.filter(|_| self.layers.walls));
        }
    }

    /// File > Open: asks for a DLL, starting in the game folder.
    fn pick_dll(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Open an Outpost 2 mission DLL").add_filter("Mission DLL", &["dll"]);
        if let Some(dir) = &self.game_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open(ctx, &path);
        }
    }

    /// File > Set game folder.
    fn pick_game_dir(&mut self, ctx: &egui::Context) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Outpost 2 game folder").pick_folder() {
            self.set_game_dir(ctx, dir);
        }
    }

    /// Uses `dir` as the game folder (op2-formats reads from it too), rescans it and reloads the open mission.
    fn set_game_dir(&mut self, ctx: &egui::Context, dir: PathBuf) {
        crate::setup::use_game_dir(&dir);
        self.browser.scan(ctx, &dir);
        self.game_dir = Some(dir);
        (self.art, self.art_error) = (None, None);
        // Reload the open mission so its map and art appear.
        if let Some(path) = self.mission.as_ref().map(|m| m.report.file.clone()) {
            self.open(ctx, &path);
        }
    }

    /// An image path from a save dialog, named after the mission: a PNG, or with `jpeg` a PNG or a JPEG.
    fn save_path(&self, suffix: &str, jpeg: bool) -> Option<PathBuf> {
        let stem = self.mission.as_ref().and_then(|m| m.report.file.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or("map".into());
        let mut dialog = rfd::FileDialog::new().set_title("Save image").add_filter("PNG image", &["png"]);
        if jpeg {
            dialog = dialog.add_filter("JPEG image", &["jpg", "jpeg"]);
        }
        dialog.set_file_name(format!("{stem}{suffix}.png")).save_file()
    }

    /// The whole map with what's shown on it: at up to 100%, or shrunk to the chosen size; a JPEG for a `.jpg` name.
    fn save_whole_map(&mut self) {
        let Some(path) = self.save_path("", true) else { return };
        let Some(m) = &self.mission else { return };
        let Some(map) = &m.map else { return };
        let result = crate::export::save_map(&path, &map.map, &map.tilesets, m.setup(), self.art.as_ref().map(|a| &a.art), &self.layers, self.whole_map_size);
        self.saved = Some(match result {
            Ok(()) => match self.whole_map_size {
                Some(px) => format!("Saved {} (at most {px} px)", path.display()),
                None => format!("Saved {} ({} px per tile)", path.display(), crate::export::scale_for(&map.map)),
            },
            Err(e) => format!("Saving {} failed: {e:#}", path.display()),
        });
    }

    /// The mission's starting state, as the setup made it with the current choices, as an OP2MissionEditor `.opm`.
    fn export_opm(&mut self) {
        let Some(m) = &self.mission else { return };
        let (Some(desc), Some(setup)) = (&m.report.desc, m.setup()) else { return };
        let stem = m.report.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("mission".into());
        let Some(path) = rfd::FileDialog::new().set_title("Export mission").add_filter("Outpost 2 mission (.opm)", &["opm"]).set_file_name(format!("{stem}.opm")).save_file() else { return };
        self.saved = Some(match crate::opm::save(&path, desc, setup) {
            Ok(notes) if notes.is_empty() => format!("Exported {}", path.display()),
            Ok(notes) => format!("Exported {}. Not carried over: {}", path.display(), notes.join("; ")),
            Err(e) => format!("Exporting {} failed: {e:#}", path.display()),
        });
    }

    /// The map as it is on screen now.
    fn save_view(&mut self, ctx: &egui::Context) {
        let Some(rect) = self.mission.as_ref().and_then(|m| m.map.as_ref()).map(|m| m.rect) else { return };
        let Some(path) = self.save_path("-view", false) else { return };
        let ppp = ctx.pixels_per_point();
        self.saved = Some(format!("Saved {}", path.display()));
        // The screenshot is of the whole window in physical pixels: crop it to the map's rect (in points).
        ctx.request_screenshot(move |img| {
            let (x0, y0) = ((rect.min.x * ppp).round() as usize, (rect.min.y * ppp).round() as usize);
            let (x1, y1) = (((rect.max.x * ppp).round() as usize).min(img.width()), ((rect.max.y * ppp).round() as usize).min(img.height()));
            let rgba: Vec<u8> = (y0..y1).flat_map(|y| img.pixels[y * img.width() + x0..y * img.width() + x1].iter().flat_map(|c| c.to_array())).collect();
            if let Err(e) = image::save_buffer(&path, &rgba, (x1 - x0) as u32, (y1 - y0) as u32, image::ColorType::Rgba8) {
                eprintln!("op2dll: saving {}: {e}", path.display());
            }
        });
    }

    /// The File, View and Help menus.
    fn menu(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let has_map = self.mission.as_ref().is_some_and(|m| m.map.is_some());
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open mission DLL...  (Ctrl+O)").clicked() {
                    self.pick_dll(ui.ctx());
                }
                if ui.button("Set game folder...").clicked() {
                    self.pick_game_dir(ui.ctx());
                }
                ui.separator();
                if ui.add_enabled(has_map, egui::Button::new("Save view as PNG...  (Ctrl+S)")).clicked() {
                    self.save_view(ui.ctx());
                }
                if ui.add_enabled(has_map, egui::Button::new("Save whole map as image...  (Ctrl+Shift+S)")).clicked() {
                    self.save_whole_map();
                }
                ui.menu_button("Whole map size", |ui| {
                    for (size, label) in WHOLE_MAP_SIZES {
                        if ui.radio(self.whole_map_size == size, label).clicked() {
                            self.whole_map_size = size;
                            ui.close();
                        }
                    }
                });
                let has_setup = self.mission.as_ref().is_some_and(|m| m.setup().is_some() && m.report.desc.is_some());
                if ui.add_enabled(has_setup, egui::Button::new("Export as .opm...  (Ctrl+E)")).clicked() {
                    self.export_opm();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_browser, "Mission browser window  (M)");
                ui.checkbox(&mut self.show_objectives, "Objectives and triggers window");
                ui.checkbox(&mut self.show_research, "Research window");
                ui.checkbox(&mut self.show_groups, "AI groups window");
                ui.checkbox(&mut self.show_disasters, "Disasters and timers window");
                ui.checkbox(&mut self.show_messages, "Messages window");
                ui.checkbox(&mut self.show_areas, "Trigger areas on the map");
                ui.separator();
                ui.checkbox(&mut self.layers.objects, "Objects  (O)");
                ui.add_enabled_ui(self.layers.objects, |ui| {
                    ui.checkbox(&mut self.layers.art, "Object art (off: dots)  (A)");
                    ui.checkbox(&mut self.layers.vehicles, "Vehicles  (V)");
                    ui.checkbox(&mut self.layers.structures, "Structures  (S)");
                    ui.checkbox(&mut self.layers.resources, "Beacons, vents, fumaroles  (R)");
                    ui.checkbox(&mut self.layers.other, "Wrecks and markers");
                });
                if ui.checkbox(&mut self.layers.walls, "Walls, tubes, tile edits  (W)").changed() {
                    action = Some(Action::Graphics);
                }
                ui.separator();
                if let Some(map) = self.mission.as_mut().and_then(|m| m.map.as_mut()) {
                    ui.checkbox(&mut map.grid, "Tile grid  (G)");
                    if ui.button("Fit map  (F)").clicked() {
                        map.fit();
                    }
                } else {
                    ui.label("Open a mission first");
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About OP2MissionReader").clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
        action
    }

    /// The side panel: the mission's facts, the start options, the players table and the object list.
    fn info(&self, ui: &mut egui::Ui) {
        ui.heading("Mission");
        let Some(m) = &self.mission else {
            ui.label("Pick one from the mission list, use File > Open mission DLL, or drop a DLL onto the window.");
            return;
        };
        let r = &m.report;
        ui.label(RichText::new(r.file.display().to_string()).small().weak());
        ui.add_space(4.0);
        if let Some(why) = &r.not_a_mission {
            ui.colored_label(Color32::LIGHT_RED, format!("Not an Outpost 2 mission: {why}"));
            return;
        }
        if let Some(d) = &r.desc {
            let or_none = |s: &str| if s.is_empty() { "(none)".to_string() } else { s.to_string() };
            ui.label(RichText::new(or_none(&d.level_desc)).strong());
            ui.add_space(4.0);
            egui::Grid::new("desc").num_columns(2).striped(true).show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                };
                row("Type", r.kind.clone().unwrap_or_default());
                row("Players", d.num_players.to_string());
                if let Some(ai) = d.ai_players {
                    row("AI players", ai.to_string());
                }
                row("Tech level", d.max_tech_level.to_string());
                row("Unit mission", if d.unit_mission { "yes" } else { "no" }.into());
                row("Map", or_none(&d.map_name));
                if let Some(map) = &m.map {
                    row("Map size", format!("{} x {} tiles{}", map.map.width, map.map.height, if map.map.wraps() { ", wraps" } else { "" }));
                }
                row("Tech tree", or_none(&d.techtree_name));
                row("Game API imports", r.game_imports.to_string());
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Exports: {}", r.mission_exports.join(", "))).small());
        for n in r.notes.iter().chain(&m.map_problems) {
            ui.colored_label(Color32::from_rgb(230, 180, 80), n);
        }
    }

    /// The start choices and everything the mission's setup created, grouped by owner.
    fn objects(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let m = self.mission.as_mut()?;
        let reveal = std::mem::take(&mut m.reveal_selected);
        let m = &*m;
        let (start, result) = m.setup.as_ref()?;
        let mut action = None;
        ui.separator();
        ui.heading("Start");
        ui.horizontal(|ui| {
            ui.label("Difficulty");
            for (d, name) in [(0, "Easy"), (1, "Normal"), (2, "Hard")] {
                if ui.selectable_label(start.difficulty == d, name).clicked() && start.difficulty != d {
                    action = Some(Action::Rerun(StartOptions { difficulty: d, ..*start }));
                }
            }
        });
        if let (Some(n), Some(d)) = (start.players, &m.report.desc) {
            ui.horizontal(|ui| {
                ui.label("Players");
                for p in 2..=d.num_players.clamp(2, 6) as u8 {
                    if ui.selectable_label(n == p, p.to_string()).clicked() && n != p {
                        action = Some(Action::Rerun(StartOptions { players: Some(p), ..*start }));
                    }
                }
            });
        }
        let s = match result {
            Ok(s) => s,
            Err(e) => {
                ui.colored_label(Color32::LIGHT_RED, format!("The mission didn't load: {e}"));
                return action;
            }
        };
        if !s.players.is_empty() {
            ui.separator();
            ui.heading("Players");
            egui::Grid::new("players").striped(true).show(ui, |ui| {
                ui.label("");
                for p in &s.players {
                    let color = objects::player_color(Some(p.color.clamp(0, 5) as u8));
                    ui.label(RichText::new(format!("{} {}", p.id, if p.eden { "Eden" } else { "Plym." })).color(color).strong())
                        .on_hover_text(format!("player {}, {}", p.id, if p.human { "human" } else { "AI" }));
                }
                ui.end_row();
                let rows: Vec<_> = s.players.iter().map(|p| p.start.rows()).collect();
                for (i, (label, _)) in rows[0].iter().enumerate() {
                    ui.label(RichText::new(*label).weak());
                    for r in &rows {
                        ui.label(&r[i].1);
                    }
                    ui.end_row();
                }
            });
        }
        ui.separator();
        ui.heading("Objects");
        ui.label(s.summary());
        if let Some(r) = s.resources().describe() {
            ui.label(RichText::new(r).weak());
        }
        if let Some(f) = &s.failed {
            ui.colored_label(Color32::LIGHT_RED, format!("Setup stopped in {f}; showing what it created until then."));
        }

        if let Some(o) = m.selected.and_then(|id| m.object(id)) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                details(ui, o, s);
            });
        }

        let selected_owner = m.selected.and_then(|id| m.object(id)).map(|o| o.owner);
        let mut owners: std::collections::BTreeMap<i32, Vec<&setup::Object>> = std::collections::BTreeMap::new();
        for o in &s.objects {
            owners.entry(o.owner).or_default().push(o);
        }
        for (owner, objects) in owners {
            let player = s.players.iter().find(|p| p.id == owner);
            let title = match player {
                Some(p) => format!(
                    "Player {owner}: {}, {} ({} objects)",
                    if p.eden { "Eden" } else { "Plymouth" },
                    if p.human { "human" } else { "AI" },
                    objects.len()
                ),
                None => format!("Gaia, owner {owner} ({} objects)", objects.len()),
            };
            ui.horizontal(|ui| {
                // Show or hide this owner's objects on the map.
                let mut shown = !self.layers.hidden_owners.contains(&owner);
                let color = objects::player_color(player.map(|p| p.color.clamp(0, 5) as u8));
                if ui.checkbox(&mut shown, "").on_hover_text("Show on the map").changed() {
                    if shown {
                        self.layers.hidden_owners.remove(&owner);
                    } else {
                        self.layers.hidden_owners.insert(owner);
                    }
                }
                let (r, painter) = ui.allocate_painter(vec2(10.0, 10.0), egui::Sense::hover());
                painter.rect_filled(r.rect, 2.0, color);
                egui::CollapsingHeader::new(title)
                    .id_salt(("owner", owner))
                    .default_open(owner == 0)
                    .open((reveal && selected_owner == Some(owner)).then_some(true))
                    .show(ui, |ui| {
                        for kind in [Kind::Structure, Kind::Vehicle, Kind::Resource, Kind::Wreckage, Kind::Other] {
                            for o in objects.iter().filter(|o| o.kind == kind) {
                                let text = RichText::new(format!("{:>3},{:<3} {}", o.tile.0, o.tile.1, o.describe())).monospace();
                                let r = ui.selectable_label(m.selected == Some(o.id), text);
                                if r.clicked() {
                                    action = Some(Action::Select(o.id, o.tile));
                                }
                                if reveal && m.selected == Some(o.id) {
                                    r.scroll_to_me(Some(egui::Align::Center));
                                }
                            }
                        }
                    });
            });
        }
        if !s.walls.is_empty() {
            let mut counts = std::collections::BTreeMap::new();
            for w in &s.walls {
                *counts.entry(w.type_name.as_str()).or_insert(0) += 1;
            }
            let list: Vec<String> = counts.iter().map(|(t, n)| format!("{n} {t}")).collect();
            ui.label(format!("Walls and tubes: {}", list.join(", ")));
        }
        if !s.unimplemented.is_empty() || !s.spawn_errors.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Sandbox notes").color(Color32::from_rgb(230, 180, 80))).show(ui, |ui| {
                for (f, n) in &s.unimplemented {
                    ui.label(format!("stubbed game call: {f} x{n}"));
                }
                for e in &s.spawn_errors {
                    ui.label(e);
                }
            });
        }
        action
    }

    /// Applies what the panels and the map asked for this frame.
    fn act(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Rerun(start) => self.run_setup(ctx, start),
            Action::Open(path) => self.open(ctx, &path),
            Action::Graphics => self.apply_graphics(ctx),
            Action::Message(i) => {
                self.selected_message = Some(i);
                let at = self.messages.as_ref().and_then(|j| j.done()?.replay.messages.get(i)?.at);
                if let (Some(at), Some(map)) = (at, self.mission.as_mut().and_then(|m| m.map.as_mut())) {
                    map.center_on(at);
                }
            }
            Action::Disaster(i) => {
                self.selected_disaster = Some(i);
                let at = self.forecast.as_ref().and_then(|j| j.done()?.disasters.get(i)?.at);
                if let (Some(at), Some(map)) = (at, self.mission.as_mut().and_then(|m| m.map.as_mut())) {
                    map.center_on(at);
                }
            }
            Action::Group(id) => {
                self.selected_group = Some(id);
                let focus = self.mission.as_ref().and_then(|m| {
                    let s = m.setup()?;
                    let g = s.groups.iter().find(|g| g.id == id)?;
                    g.focus().or_else(|| g.members.first().and_then(|&u| m.object(u)).map(|o| o.footprint))
                });
                if let (Some(((x1, y1), (x2, y2))), Some(map)) = (focus, self.mission.as_mut().and_then(|m| m.map.as_mut())) {
                    map.center_on(((x1 + x2) / 2, (y1 + y2) / 2));
                }
            }
            Action::Trigger(id) => {
                self.selected_trigger = Some(id);
                let area = self.mission.as_ref().and_then(|m| m.setup()?.triggers.iter().find(|t| t.id == id)?.area);
                if let (Some(((x1, y1), (x2, y2))), Some(map)) = (area, self.mission.as_mut().and_then(|m| m.map.as_mut())) {
                    map.center_on(((x1 + x2) / 2, (y1 + y2) / 2));
                }
            }
            Action::Pick(id) => {
                if let Some(m) = &mut self.mission {
                    m.selected = id;
                    m.reveal_selected = id.is_some();
                    let area = id.and_then(|id| m.object(id)).map(|o| o.footprint);
                    if let Some(map) = &mut m.map {
                        map.selected = area;
                    }
                }
            }
            Action::Select(id, tile) => {
                if let Some(m) = &mut self.mission {
                    m.selected = Some(id);
                    let area = m.object(id).map(|o| o.footprint);
                    if let Some(map) = &mut m.map {
                        map.selected = area;
                        map.center_on(tile);
                    }
                }
            }
        }
    }

    /// The status bar: the object or tile under the mouse (else the last save), the zoom and the game folder.
    fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let map = self.mission.as_ref().and_then(|m| m.map.as_ref());
            let object = self.mission.as_ref().zip(self.view.hovered_object).and_then(|(m, id)| m.object(id));
            match (self.view.hovered, map, object) {
                (_, _, Some(o)) => {
                    ui.label(format!("{}   player {}   tile {}, {}", o.describe(), o.owner, o.tile.0, o.tile.1));
                }
                (Some((x, y)), Some(v), None) => {
                    let t = v.map.tile(x, y);
                    ui.label(format!(
                        "Tile {x}, {y}  (game x {})   cell {:?}   mapping {}",
                        x as i32 + v.map.x_padding(),
                        t.cell_type,
                        t.mapping_index
                    ));
                }
                _ => {
                    ui.label(RichText::new(self.saved.as_deref().unwrap_or(" ")).weak());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(v) = map {
                    ui.label(format!("{:.0}%", v.zoom * 100.0));
                }
                match &self.game_dir {
                    Some(d) => ui.label(RichText::new(d.display().to_string()).weak()),
                    None => ui.colored_label(Color32::LIGHT_RED, "no game folder"),
                };
            });
        });
    }

    /// The hovered object's details beside the mouse.
    fn hover_tooltip(&self, ctx: &egui::Context) {
        let Some(m) = &self.mission else { return };
        let (Some(o), Some(s), Some(p)) = (self.view.hovered_object.and_then(|id| m.object(id)), m.setup(), ctx.pointer_hover_pos()) else {
            return;
        };
        egui::Area::new(egui::Id::unique("object-hover"))
            .order(egui::Order::Tooltip)
            .fixed_pos(p + vec2(18.0, 18.0))
            .interactable(false)
            .show(ctx, |ui| egui::Frame::popup(ui.style()).show(ui, |ui| details(ui, o, s)));
    }

    /// Up and Down step through the mission list, in either window and even while typing in the filter.
    fn step_keys(&self, ctx: &egui::Context) -> Option<Action> {
        let step = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as isize - i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as isize);
        if step == 0 || !self.show_browser {
            return None;
        }
        let current = self.mission.as_ref().map(|m| m.report.file.clone());
        self.browser.next(current.as_deref(), step).filter(|p| Some(p) != current.as_ref()).map(Action::Open)
    }

    /// The mission browser, a window of its own beside the main one; closing it is the same as View > Mission browser.
    fn browser_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        // First shown to the left of the main window, where there's room, else over its left edge.
        let main = ctx.input(|i| i.viewport().outer_rect);
        let mut builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - Missions").with_inner_size([380.0, 760.0]);
        if let Some(r) = main {
            builder = builder.with_position(egui::pos2((r.min.x - 390.0).max(0.0), r.min.y));
        }
        let current = self.mission.as_ref().map(|m| m.report.file.clone());
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("missions"), builder, |ui, _| {
            if ui.input(|i| i.viewport().close_requested()) {
                self.show_browser = false;
                return None;
            }
            let picked = egui::CentralPanel::default().show(ui, |ui| self.browser.ui(ui, current.as_deref())).inner;
            picked.map(Action::Open).or_else(|| self.step_keys(ui.ctx()))
        })
    }

    /// View > Objectives and triggers, a window of its own; closing it unticks the menu item.
    fn objectives_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - Objectives and triggers").with_inner_size([760.0, 640.0]);
        let (setup, selected) = (self.mission.as_ref().and_then(|m| m.setup()), self.selected_trigger);
        let show_areas = &mut self.show_areas;
        let mut close = false;
        let picked = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("objectives"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested());
            egui::CentralPanel::default()
                .show(ui, |ui| match setup {
                    Some(s) => crate::windows::objectives(ui, s, selected, show_areas),
                    None => {
                        ui.label(RichText::new("Open a mission whose setup ran.").weak());
                        None
                    }
                })
                .inner
        });
        self.show_objectives &= !close;
        picked.map(Action::Trigger)
    }

    /// View > AI groups, a window of its own.
    fn groups_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - AI groups").with_inner_size([620.0, 700.0]);
        let mission = self.mission.as_ref();
        let (setup, selected) = (mission.and_then(|m| m.setup()), self.selected_group);
        let all_plans = &mut self.all_plans;
        let mut close = false;
        let picked = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("groups"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested());
            egui::CentralPanel::default()
                .show(ui, |ui| match setup {
                    Some(s) => crate::windows::groups(ui, s, selected, all_plans),
                    None => {
                        ui.label(RichText::new("Open a mission whose setup ran.").weak());
                        None
                    }
                })
                .inner
        });
        self.show_groups &= !close;
        match picked? {
            crate::windows::GroupPick::Group(id) => Some(Action::Group(id)),
            crate::windows::GroupPick::Unit(id) => mission?.object(id).map(|o| Action::Select(id, o.tile)),
        }
    }

    /// View > Disasters and timers, a window of its own. Starts the forecast for the open mission when there's none for
    /// it (or for the length chosen).
    fn disasters_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - Disasters and timers").with_inner_size([760.0, 640.0]);
        let mission = self.mission.as_ref();
        let key = mission.and_then(|m| Some((m.report.file.clone(), m.setup.as_ref()?.0, self.forecast_marks)));
        if let Some(key) = key.filter(|k| self.forecast.as_ref().is_none_or(|j| &j.key != k)) {
            self.selected_disaster = None;
            self.forecast = Some(crate::background::Job::start(ctx, key, |(dll, start, marks)| crate::disasters::forecast(dll, *start, *marks)));
        }
        let setup = mission.and_then(|m| m.setup());
        let (job, marks, selected) = (&mut self.forecast, &mut self.forecast_marks, self.selected_disaster);
        let mut close = false;
        let picked = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("disasters"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested());
            egui::CentralPanel::default()
                .show(ui, |ui| match setup {
                    Some(s) => crate::windows::disasters(ui, s, job.as_mut(), marks, selected),
                    None => {
                        ui.label(RichText::new("Open a mission whose setup ran.").weak());
                        None
                    }
                })
                .inner
        });
        self.show_disasters &= !close;
        picked.map(Action::Disaster)
    }

    /// View > Messages, a window of its own. Reads and replays the open mission when there's no report for it (or for
    /// the length chosen).
    fn messages_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - Messages").with_inner_size([760.0, 640.0]);
        let mission = self.mission.as_ref();
        let key = mission.and_then(|m| Some((m.report.file.clone(), m.setup.as_ref()?.0, self.forecast_marks)));
        if let Some(key) = key.filter(|k| self.messages.as_ref().is_none_or(|j| &j.key != k)) {
            self.selected_message = None;
            self.messages = Some(crate::background::Job::start(ctx, key, |(dll, start, marks)| crate::messages::report(dll, *start, *marks)));
        }
        let (job, marks, selected) = (&mut self.messages, &mut self.forecast_marks, self.selected_message);
        let mut close = false;
        let picked = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("messages"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested());
            egui::CentralPanel::default().show(ui, |ui| crate::windows::messages(ui, job.as_mut(), marks, selected)).inner
        });
        self.show_messages &= !close;
        picked.map(Action::Message)
    }

    /// View > Research, a window of its own.
    fn research_window(&mut self, ctx: &egui::Context) {
        let builder = egui::ViewportBuilder::default().with_icon(icon()).with_title("OP2MissionReader - Research").with_inner_size([420.0, 640.0]);
        let mission = self.mission.as_ref();
        let setup = mission.and_then(|m| m.setup());
        let techtree = mission.and_then(|m| m.report.desc.as_ref()).map(|d| d.techtree_name.clone()).unwrap_or_default();
        let player = &mut self.research_player;
        let mut close = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("research"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested());
            egui::CentralPanel::default().show(ui, |ui| match setup {
                Some(s) => crate::windows::research(ui, s, &techtree, player),
                None => {
                    ui.label(RichText::new("Open a mission whose setup ran.").weak());
                }
            });
        });
        self.show_research &= !close;
    }

    /// Help > About, a small window of its own: the banner, the name and version, what it does and where it lives.
    fn about_window(&mut self, ctx: &egui::Context) {
        let builder = egui::ViewportBuilder::default()
            .with_icon(icon())
            .with_title("About OP2MissionReader")
            .with_inner_size([660.0, 400.0])
            .with_resizable(false);
        let banner = self
            .about_banner
            .get_or_insert_with(|| {
                let image = image::load_from_memory_with_format(include_bytes!("../assets/banner.png"), image::ImageFormat::Png)
                    .map(|i| i.into_rgba8())
                    .unwrap_or_default();
                let size = [image.width() as usize, image.height() as usize];
                ctx.load_texture("about-banner", egui::ColorImage::from_rgba_unmultiplied(size, &image), egui::TextureOptions::LINEAR)
            })
            .clone();
        let mut close = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("about"), builder, |ui, _| {
            close = ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape));
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new(&banner).fit_to_exact_size(vec2(620.0, 100.0)));
                });
                ui.add_space(16.0);
                ui.indent("about", |ui| {
                    ui.label(RichText::new("OP2MissionReader").size(22.0).strong());
                    ui.add_space(4.0);
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(8.0);
                    ui.label(
                        "Opens Outpost 2 mission DLLs: runs each mission's setup, shows its map with every unit, structure                          and beacon, and rewrites it as OP2Lua.",
                    );
                    ui.add_space(16.0);
                    ui.label(RichText::new("Outpost Universe © 2026").weak());
                    ui.hyperlink("https://github.com/leviathan400/OP2MissionReader");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                    if ui.add(egui::Button::new("OK").min_size(vec2(90.0, 28.0))).clicked() {
                        close = true;
                    }
                });
            });
        });
        self.show_about &= !close;
    }

    /// Keys that work whenever no text field has the keyboard.
    fn keys(&mut self, ctx: &egui::Context) -> Option<Action> {
        let (save_view, save_map, export) = ctx.input_mut(|i| {
            let all = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::S);
            (i.consume_key(egui::Modifiers::COMMAND, egui::Key::S), all, i.consume_key(egui::Modifiers::COMMAND, egui::Key::E))
        });
        if export {
            self.export_opm();
        }
        if save_map {
            self.save_whole_map();
        }
        if save_view {
            self.save_view(ctx);
        }
        let mut action = self.step_keys(ctx);
        // The single-letter toggles below would otherwise fire while typing in the browser's filter.
        if ctx.egui_wants_keyboard_input() {
            return action;
        }
        let plain = |i: &egui::InputState, k| i.key_pressed(k) && !i.modifiers.command;
        let keys = [egui::Key::G, egui::Key::F, egui::Key::O, egui::Key::A, egui::Key::V, egui::Key::S, egui::Key::R, egui::Key::W, egui::Key::M];
        let [g, f, o, a, v, s, r, w, m] = ctx.input(|i| keys.map(|k| plain(i, k)));
        self.layers.objects ^= o;
        self.layers.art ^= a;
        self.layers.vehicles ^= v;
        self.layers.structures ^= s;
        self.layers.resources ^= r;
        self.show_browser ^= m;
        if w {
            self.layers.walls ^= true;
            action = Some(Action::Graphics);
        }
        if let Some(map) = self.mission.as_mut().and_then(|m| m.map.as_mut()) {
            map.grid ^= g;
            if f {
                map.fit();
            }
        }
        action
    }

    /// `--screenshot`: once the window has drawn (and the mission list is read), save it and close.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.screenshot else { return };
        if job.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.request_repaint();
        if self.browser.busy() {
            return;
        }
        if job.frames_left > 0 {
            job.frames_left -= 1;
            if job.frames_left == 0 {
                let (path, done) = (job.path.clone(), job.done.clone());
                ctx.request_screenshot(move |img| {
                    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Err(e) = image::save_buffer(&path, &rgba, img.width() as u32, img.height() as u32, image::ColorType::Rgba8) {
                        eprintln!("op2dll: saving {}: {e}", path.display());
                    }
                    done.store(true, Ordering::SeqCst);
                });
            }
        }
    }
}

/// An object's type, owner, place and what the mission set on it.
fn details(ui: &mut egui::Ui, o: &setup::Object, s: &Setup) {
    ui.label(RichText::new(&o.type_name).strong());
    egui::Grid::new(("details", o.id)).num_columns(2).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(RichText::new(k).weak());
            ui.label(v);
            ui.end_row();
        };
        let owner = match s.players.iter().find(|p| p.id == o.owner) {
            Some(p) => format!(
                "player {}, {}, {}, {}",
                o.owner,
                if p.eden { "Eden" } else { "Plymouth" },
                if p.human { "human" } else { "AI" },
                objects::color_name(o.color)
            ),
            None => format!("Gaia ({})", o.owner),
        };
        row("Owner", owner);
        row("Tile", format!("{}, {}", o.tile.0, o.tile.1));
        row("Object", format!("#{}  ({:?})", o.id, o.kind));
        for (k, v) in o.details() {
            row(k, v);
        }
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let (open_key, dropped) = ui.input(|i| {
            (i.modifiers.command && i.key_pressed(egui::Key::O), i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
        });
        if open_key {
            self.pick_dll(&ctx);
        }
        if let Some(path) = dropped {
            if path.is_dir() {
                self.set_game_dir(&ctx, path);
            } else {
                self.open(&ctx, &path);
            }
        }
        let mut actions: Vec<Action> = self.keys(&ctx).into_iter().collect();

        actions.extend(egui::Panel::top("menu").show(ui, |ui| self.menu(ui)).inner);
        egui::Panel::bottom("status").show(ui, |ui| self.status(ui));
        if self.show_browser {
            actions.extend(self.browser_window(&ctx));
        }
        if self.show_objectives {
            actions.extend(self.objectives_window(&ctx));
        }
        if self.show_research {
            self.research_window(&ctx);
        }
        if self.show_about {
            self.about_window(&ctx);
        }
        if self.show_groups {
            actions.extend(self.groups_window(&ctx));
        }
        if self.show_disasters {
            actions.extend(self.disasters_window(&ctx));
        }
        if self.show_messages {
            actions.extend(self.messages_window(&ctx));
        }
        let action = egui::Panel::left("info")
            .resizable(true)
            .default_size(340.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        for e in self.error.iter().chain(&self.art_error) {
                            ui.colored_label(Color32::LIGHT_RED, e);
                        }
                        self.info(ui);
                        self.objects(ui)
                    })
                    .inner
            })
            .inner;
        actions.extend(action);
        // Apply this frame's picks before the map draws, so it shows the new selection and centre at once.
        for a in actions {
            self.act(&ctx, a);
        }
        let (layers, art) = (&self.layers, self.art.as_mut());
        let (all_areas, trigger) = (self.show_areas, self.show_objectives.then_some(self.selected_trigger).flatten());
        let forecast = self.show_disasters.then(|| self.forecast.as_ref()?.done()).flatten();
        let selected_disaster = self.selected_disaster;
        let messages = self.show_messages.then(|| self.messages.as_ref()?.done()).flatten();
        let selected_message = self.selected_message;
        let (group, all_plans) = (self.show_groups.then_some(self.selected_group).flatten(), self.show_groups && self.all_plans);
        let view = egui::CentralPanel::no_frame()
            .show(ui, |ui| match self.mission.as_mut() {
                Some(Loaded { map: Some(map), setup, .. }) => {
                    let setup = setup.as_ref().and_then(|(_, s)| s.as_ref().ok());
                    map.ui(ui, |painter, t| {
                        setup.and_then(|s| {
                            let hovered = objects::draw(&ctx, painter, t, s, art, layers);
                            if all_areas || trigger.is_some() {
                                objects::draw_areas(painter, t, s, trigger, all_areas);
                            }
                            objects::draw_groups(painter, t, s, group, all_plans);
                            if let Some(f) = forecast {
                                objects::draw_disasters(painter, t, f, selected_disaster);
                            }
                            if let Some(r) = messages {
                                objects::draw_messages(painter, t, &r.replay, selected_message);
                            }
                            hovered
                        })
                    })
                }
                _ => {
                    ui.centered_and_justified(|ui| ui.label(RichText::new("No map").weak()));
                    ViewState::default()
                }
            })
            .inner;
        if view.clicked {
            self.act(&ctx, Action::Pick(view.hovered_object));
        }
        self.view = view;
        self.hover_tooltip(&ctx);
        self.screenshot_step(&ctx);
    }
}

/// Reads the mission's map and its tilesets from the game folder.
fn load_map(ctx: &egui::Context, dir: &Path, name: &str) -> (Option<MapView>, Vec<String>) {
    let files = match GameFiles::open(dir) {
        Ok(f) => f,
        Err(e) => return (None, vec![format!("game folder {}: {e}", dir.display())]),
    };
    let map = match files.read_map(name) {
        Ok(m) => m,
        Err(e) => return (None, vec![format!("map {name}: {e}")]),
    };
    let tilesets = MapTilesets::load(&files, &map);
    let problems = map
        .tilesets
        .iter()
        .zip(&tilesets.sets)
        .filter_map(|(src, set)| match (src, set) {
            (Some(src), Some(Err(e))) => Some(format!("tileset {}: {e}", src.name)),
            _ => None,
        })
        .collect();
    (Some(MapView::new(ctx, map, tilesets)), problems)
}

/// The app's icon (assets/Tech_Planet.ico) for every window's title bar and taskbar button; the exe carries the same
/// file as a resource (build.rs).
pub fn icon() -> std::sync::Arc<egui::IconData> {
    /// Decoded on first use and shared by every window (an empty icon if it doesn't decode).
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let image = image::load_from_memory_with_format(include_bytes!("../assets/Tech_Planet.ico"), image::ImageFormat::Ico);
        let icon = image.map(|i| i.into_rgba8()).map(|i| egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() });
        std::sync::Arc::new(icon.unwrap_or_default())
    })
    .clone()
}

/// The window's choices for a saved whole map's longest side: full size, or shrunk (a Lanczos filter) for the web.
const WHOLE_MAP_SIZES: [(Option<u32>, &str); 5] =
    [(None, "Full size (up to 32 px a tile)"), (Some(4096), "4096 px"), (Some(2048), "2048 px"), (Some(1024), "1024 px"), (Some(512), "512 px")];

#[cfg(test)]
mod tests {
    /// The embedded .ico decodes to a non-empty RGBA image (`icon` falls back to an empty one silently).
    #[test]
    fn icon_decodes() {
        let icon = super::icon();
        assert!(icon.width > 0 && icon.rgba.len() == (icon.width * icon.height * 4) as usize);
    }

    /// The About window's embedded banner decodes (it falls back to an empty image silently).
    #[test]
    fn banner_decodes() {
        let banner = image::load_from_memory_with_format(include_bytes!("../assets/banner.png"), image::ImageFormat::Png).unwrap();
        assert_eq!((banner.width(), banner.height()), (620, 100));
    }
}
