//! The windows the View menu opens beside the main one: the mission's objectives and triggers, each player's
//! finished research by name, the AI groups, the timers and disaster forecast, and the messages. Each draws into the
//! window `app.rs` opens for it and returns what was clicked, for the map to mark.

use eframe::egui::{self, Color32, RichText};

use crate::objects;
use crate::setup::Setup;

/// Objectives first (what wins or loses the mission and what that waits on), then every trigger. Returns a clicked
/// trigger's id, which the map highlights and centres on.
pub fn objectives(ui: &mut egui::Ui, s: &Setup, selected: Option<u32>, show_areas: &mut bool) -> Option<u32> {
    let mut picked = None;
    let by_id = |id: u32| s.triggers.iter().find(|t| t.id == id);
    ui.checkbox(show_areas, "Show every trigger's area on the map");
    if let Some(f) = &s.failed {
        ui.colored_label(Color32::LIGHT_RED, format!("The setup stopped in {f}; triggers it would have made after that are missing."));
    }
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.heading("Objectives");
        let goals: Vec<_> = s.triggers.iter().filter_map(|t| Some((t, t.objective.as_ref()?))).collect();
        if goals.is_empty() {
            ui.label(RichText::new("The setup made no victory or failure conditions.").weak());
        }
        for (t, o) in &goals {
            let (word, color) = if o.victory { ("Win", Color32::from_rgb(90, 190, 90)) } else { ("Lose", Color32::from_rgb(220, 90, 80)) };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(word).color(color).strong());
                ui.label(if o.text.is_empty() { "(no text)" } else { &o.text });
            });
            let when = by_id(o.linked).map_or_else(|| format!("trigger #{} (not found)", o.linked), |l| format!("#{} {}: {}", l.id, l.kind, l.what));
            ui.indent(("goal", t.id), |ui| {
                if ui.selectable_label(selected == Some(o.linked), RichText::new(format!("when {when}")).weak()).clicked() {
                    picked = Some(o.linked);
                }
            });
        }
        ui.separator();
        ui.heading(format!("Triggers ({})", s.triggers.len()));
        egui::Grid::new("triggers").striped(true).num_columns(4).show(ui, |ui| {
            for h in ["#", "Kind", "Watches", "Calls"] {
                ui.label(RichText::new(h).weak());
            }
            ui.end_row();
            for t in &s.triggers {
                ui.label(t.id.to_string());
                let mut kind = t.kind.clone();
                if !t.enabled {
                    kind += " (off)";
                }
                if !t.one_shot {
                    kind += " (repeats)";
                }
                ui.label(kind);
                let text = if t.area.is_some() { format!("{}  [area]", t.what) } else { t.what.clone() };
                if ui.selectable_label(selected == Some(t.id), text).clicked() {
                    picked = Some(t.id);
                }
                ui.label(RichText::new(t.callback.as_deref().unwrap_or("")).monospace());
                ui.end_row();
            }
        });
    });
    picked
}

/// The research each player starts with done, by name from the mission's tech tree.
pub fn research(ui: &mut egui::Ui, s: &Setup, techtree: &str, player: &mut i32) {
    if s.players.is_empty() {
        ui.label("No players.");
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Player");
        for p in &s.players {
            let color = objects::player_color(Some(p.color.clamp(0, 5) as u8));
            let text = RichText::new(format!("{} {} ({})", p.id, if p.eden { "Eden" } else { "Plymouth" }, p.start.techs.len())).color(color);
            if ui.selectable_label(*player == p.id, text).clicked() {
                *player = p.id;
            }
        }
    });
    let Some(p) = s.players.iter().find(|p| p.id == *player).or(s.players.first()) else { return };
    if s.tech_names.is_empty() {
        ui.colored_label(Color32::from_rgb(230, 180, 80), format!("Couldn't read the tech tree {techtree}; showing topic numbers only."));
    } else {
        ui.label(RichText::new(format!("{techtree}: {} topics", s.tech_names.len())).weak());
    }
    ui.separator();
    if p.start.techs.is_empty() {
        ui.label(RichText::new("No research done at the start.").weak());
        return;
    }
    let mut techs = p.start.techs.clone();
    techs.sort_unstable();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new(("research", p.id)).striped(true).num_columns(2).show(ui, |ui| {
            for id in techs {
                ui.label(RichText::new(format!("{id:05}")).monospace());
                match s.tech_names.get(&id) {
                    Some(name) => ui.label(name),
                    None => ui.label(RichText::new("(not in the tech tree)").weak()),
                };
                ui.end_row();
            }
        });
    });
}

/// What was clicked in the AI groups window.
pub enum GroupPick {
    /// A group's header or "Show on the map" (the group's id): outline it.
    Group(u32),
    /// One of a group's units (its object id): select it in the main window.
    Unit(u32),
}

/// The computer players' AI groups by player: what each is told, its units, what it wants and a building group's
/// plans. Opening a group selects it, which outlines it on the map.
pub fn groups(ui: &mut egui::Ui, s: &Setup, selected: Option<u32>, all_plans: &mut bool) -> Option<GroupPick> {
    let mut picked = None;
    ui.checkbox(all_plans, "Show every building group's plans on the map");
    ui.label(RichText::new("Groups the setup made; ones the mission makes later, from a trigger or its AI code, aren't listed.").weak());
    ui.separator();
    if s.groups.is_empty() {
        ui.label(RichText::new("The setup made no AI groups.").weak());
        return None;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let owners: std::collections::BTreeSet<i32> = s.groups.iter().map(|g| g.owner).collect();
        for owner in owners {
            let player = s.players.iter().find(|p| p.id == owner);
            let color = objects::player_color(player.map(|p| p.color.clamp(0, 5) as u8));
            let colony = player.map_or("", |p| if p.eden { " Eden" } else { " Plymouth" });
            ui.heading(RichText::new(format!("Player {owner}{colony}")).color(color));
            for g in s.groups.iter().filter(|g| g.owner == owner) {
                let mut header = format!("{}  ·  {} units", g.title(), g.members.len());
                if let Some((_, objective)) = g.orders.iter().find(|(l, _)| *l == "Objective") {
                    header += &format!("  ·  {objective}");
                }
                if !g.enabled {
                    header += "  (off)";
                }
                let text = if selected == Some(g.id) { RichText::new(header).strong().color(Color32::from_rgb(80, 220, 255)) } else { RichText::new(header) };
                let r = egui::CollapsingHeader::new(text).id_salt(("group", g.id)).show(ui, |ui| group(ui, s, g, &mut picked));
                if r.header_response.clicked() {
                    picked = Some(GroupPick::Group(g.id));
                }
            }
            ui.add_space(6.0);
        }
    });
    picked
}

/// One AI group's orders, units, wants and plans.
fn group(ui: &mut egui::Ui, s: &Setup, g: &crate::groups::Group, picked: &mut Option<GroupPick>) {
    if ui.selectable_label(false, "Show on the map").clicked() {
        *picked = Some(GroupPick::Group(g.id));
    }
    egui::Grid::new(("orders", g.id)).num_columns(2).show(ui, |ui| {
        for (label, value) in &g.orders {
            ui.label(RichText::new(*label).weak());
            ui.label(value);
            ui.end_row();
        }
        for (label, ((x1, y1), (x2, y2))) in &g.areas {
            ui.label(RichText::new("Tiles").weak());
            ui.label(if (x1, y1) == (x2, y2) { format!("{label}: {x1}, {y1}") } else { format!("{label}: {x1}, {y1} to {x2}, {y2}") });
            ui.end_row();
        }
        for w in &g.wants {
            ui.label(RichText::new("Wants").weak());
            ui.label(format!("{} {}  (has {})", w.count, w.unit, w.have));
            ui.end_row();
        }
    });
    if !g.members.is_empty() {
        ui.label(RichText::new(format!("Units ({})", g.members.len())).weak());
        ui.indent(("members", g.id), |ui| {
            for o in g.members.iter().filter_map(|&id| s.objects.iter().find(|o| o.id == id)) {
                if ui.selectable_label(false, RichText::new(format!("{:>3},{:<3} {}", o.tile.0, o.tile.1, o.describe())).monospace()).clicked() {
                    *picked = Some(GroupPick::Unit(o.id));
                }
            }
        });
    }
    // Structures and mines are listed one by one; wall and tube tiles only counted by type.
    let (walls, plans): (Vec<_>, Vec<_>) = g.plans.iter().partition(|p| p.wall);
    if !plans.is_empty() {
        egui::CollapsingHeader::new(format!("Plans: {} structures and mines", plans.len())).id_salt(("plans", g.id)).show(ui, |ui| {
            for p in plans {
                let ((x1, y1), _) = p.area;
                let line = RichText::new(format!("{x1:>3},{y1:<3} {}", p.what)).monospace();
                ui.label(if p.standing { line } else { line.color(Color32::from_rgb(120, 220, 120)) });
            }
            ui.label(RichText::new("Green: not built at the start.").weak().small());
        });
    }
    if !walls.is_empty() {
        let mut counts = std::collections::BTreeMap::new();
        for w in &walls {
            *counts.entry(w.what.as_str()).or_insert(0) += 1;
        }
        let list: Vec<String> = counts.iter().map(|(t, n)| format!("{n} {t}")).collect();
        ui.label(format!("Plans: {} tiles", list.join(", ")));
    }
    if !g.calls.is_empty() {
        egui::CollapsingHeader::new(format!("Setup calls ({})", g.calls.len())).id_salt(("calls", g.id)).show(ui, |ui| {
            for c in &g.calls {
                ui.label(RichText::new(c).monospace().small());
            }
        });
    }
}

/// How far ahead the disasters window can forecast, in marks.
pub const FORECAST_MARKS: [u32; 5] = [100, 300, 600, 1000, 2000];

/// The mission's timers from its setup, then the forecast of the disasters it sets off. Returns a clicked
/// disaster's index, which the map marks and centres on.
pub fn disasters(ui: &mut egui::Ui, s: &Setup, job: Option<&mut crate::background::Job<crate::disasters::Forecast>>, marks: &mut u32, selected: Option<usize>) -> Option<usize> {
    let mut picked = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.heading("Timers");
        // Time triggers, by the kind name the setup gave them.
        let timers: Vec<_> = s.triggers.iter().filter(|t| t.kind.contains("Time")).collect();
        if timers.is_empty() {
            ui.label(RichText::new("The setup made no time triggers.").weak());
        } else {
            egui::Grid::new("timers").striped(true).num_columns(3).show(ui, |ui| {
                for h in ["#", "When", "Calls"] {
                    ui.label(RichText::new(h).weak());
                }
                ui.end_row();
                for t in timers {
                    ui.label(t.id.to_string());
                    let mut when = if t.one_shot { t.what.replacen("every", "once, after", 1) } else { t.what.clone() };
                    if !t.enabled {
                        when += " (off)";
                    }
                    ui.label(when);
                    ui.label(RichText::new(t.callback.as_deref().unwrap_or("")).monospace());
                    ui.end_row();
                }
            });
        }
        ui.separator();
        ui.heading("Disasters");
        ui.horizontal(|ui| {
            ui.label("Forecast the first");
            egui::ComboBox::from_id_salt("forecast marks").selected_text(format!("{marks} marks")).show_ui(ui, |ui| {
                for m in FORECAST_MARKS {
                    ui.selectable_value(marks, m, format!("{m} marks"));
                }
            });
        });
        ui.label(
            RichText::new(
                "op2-rs replays the mission with nobody playing and lists the disasters its code sets off: a prediction. \
                 Triggers that wait on the player don't fire, and random times and places can differ in a game.",
            )
            .weak(),
        );
        let Some(job) = job else { return };
        match job.poll() {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Replaying the mission...");
                });
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::LIGHT_RED, format!("The forecast failed: {e}"));
            }
            Some(Ok(f)) => {
                let mark = crate::disasters::MARK;
                if let Some(why) = &f.stopped {
                    ui.colored_label(Color32::from_rgb(230, 180, 80), format!("Stopped at mark {}: {why}.", f.reached / mark));
                }
                for w in &f.warnings {
                    ui.label(RichText::new(format!("Note: {w}")).weak().small());
                }
                if f.disasters.is_empty() {
                    ui.label(RichText::new(format!("No disasters in the first {} marks.", f.reached / mark)).weak());
                    return;
                }
                egui::Grid::new("disasters").striped(true).num_columns(5).show(ui, |ui| {
                    for h in ["When", "Disaster", "Where", "", "Set off by"] {
                        ui.label(RichText::new(h).weak());
                    }
                    ui.end_row();
                    for (i, d) in f.disasters.iter().enumerate() {
                        ui.label(d.when());
                        if ui.selectable_label(selected == Some(i), d.name()).clicked() {
                            picked = Some(i);
                        }
                        ui.label(d.place());
                        ui.label(RichText::new(&d.detail).weak());
                        ui.label(RichText::new(&d.callback).monospace());
                        ui.end_row();
                    }
                });
            }
        }
    });
    picked
}

/// The functions that post a message, one a line, each with the triggers that call it.
fn posters(ui: &mut egui::Ui, posters: &[crate::messages::Poster]) {
    ui.vertical(|ui| {
        if posters.is_empty() {
            ui.label(RichText::new("no exported function reaches it (unused code, or called through a pointer)").weak());
        }
        for p in posters {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&p.function).monospace());
                let by = if p.by.is_empty() { "no trigger in the setup or the replay calls it".to_string() } else { p.by.join("; ") };
                ui.label(RichText::new(by).weak());
            });
        }
    });
}

/// The messages window: every message text in the DLL with who posts it, then the messages a replay with nobody
/// playing posts, in order. Returns a clicked replay message (its index), which the map marks and centres on.
pub fn messages(ui: &mut egui::Ui, job: Option<&mut crate::background::Job<crate::messages::Report>>, marks: &mut u32, selected: Option<usize>) -> Option<usize> {
    let mut picked = None;
    let r = match job?.poll() {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the mission and replaying it...");
            });
            return None;
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::LIGHT_RED, format!("Couldn't read the messages: {e}"));
            return None;
        }
        Some(Ok(r)) => r,
    };
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.heading(format!("In the mission ({})", r.texts.len()));
        ui.label(RichText::new("Every message text the mission's code passes to AddMessage, whether or not anything triggers it.").weak());
        if r.texts.is_empty() {
            ui.label(RichText::new("No message texts in the DLL.").weak());
        }
        egui::Grid::new("texts").striped(true).num_columns(3).show(ui, |ui| {
            if !r.texts.is_empty() {
                for h in ["Message", "Replay", "Posted by, and what calls it"] {
                    ui.label(RichText::new(h).weak());
                }
                ui.end_row();
            }
            for t in &r.texts {
                let calls = if t.calls > 1 { format!("  (×{})", t.calls) } else { String::new() };
                ui.label(format!("{}{calls}", t.text));
                match t.first.and_then(|i| Some((i, r.replay.messages.get(i)?))) {
                    Some((i, m)) => {
                        if ui.selectable_label(selected == Some(i), RichText::new(m.when()).weak()).clicked() {
                            picked = Some(i);
                        }
                    }
                    None => {
                        ui.label("");
                    }
                }
                posters(ui, &t.from);
                ui.end_row();
            }
        });
        if r.runtime > 0 {
            ui.label(RichText::new(format!("{} more calls post a text the mission makes while it runs; the replay shows the ones it reaches.", r.runtime)).weak());
            posters(ui, &r.runtime_from);
        }
        ui.separator();
        ui.heading("Replay");
        ui.horizontal(|ui| {
            ui.label("Replay the first");
            egui::ComboBox::from_id_salt("message marks").selected_text(format!("{marks} marks")).show_ui(ui, |ui| {
                for m in FORECAST_MARKS {
                    ui.selectable_value(marks, m, format!("{m} marks"));
                }
            });
        });
        ui.label(
            RichText::new("op2-rs replays the mission with nobody playing: messages that answer the player (a building finished, a unit lost) don't come.")
                .weak(),
        );
        let m = &r.replay;
        let mark = crate::disasters::MARK;
        if let Some(why) = &m.stopped {
            ui.colored_label(Color32::from_rgb(230, 180, 80), format!("Stopped at mark {}: {why}.", m.reached / mark));
        }
        if m.messages.is_empty() {
            ui.label(RichText::new(format!("No messages in the first {} marks.", m.reached / mark)).weak());
            return;
        }
        egui::Grid::new("replay").striped(true).num_columns(4).show(ui, |ui| {
            for h in ["When", "To", "Message", "Posted by"] {
                ui.label(RichText::new(h).weak());
            }
            ui.end_row();
            for (i, msg) in m.messages.iter().enumerate() {
                ui.label(msg.when());
                ui.label(msg.to_text());
                let text = if msg.at.is_some() { format!("{}  [place]", msg.text) } else { msg.text.clone() };
                if ui.selectable_label(selected == Some(i), text).clicked() {
                    picked = Some(i);
                }
                ui.label(RichText::new(&msg.from).monospace());
                ui.end_row();
            }
        });
    });
    picked
}
