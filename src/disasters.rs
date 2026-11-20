//! Disasters and timed events: the mission's timers from its setup, and op2-missionhost's forecast of the disasters
//! its code sets off. The forecast replays the mission headless with nobody playing (`forecast::predict`), so it's a
//! prediction: what a trigger does once the player acts, or on the game's random numbers, can differ.

use std::path::Path;

use anyhow::Result;
use op2_missionhost::forecast::{self, PredictedTime};
use serde::Serialize;

use crate::setup::StartOptions;

/// Ticks in a mark.
pub const MARK: u32 = 100;
/// How far the forecast looks unless asked: 600 marks, op2-inspect's horizon.
pub const DEFAULT_MARKS: u32 = 600;

/// The disasters a replay with nobody playing set off ([`forecast`]).
#[derive(Debug, Clone, Serialize)]
pub struct Forecast {
    /// How far the replay got, in ticks.
    pub reached: u32,
    /// How far it was asked to go, in ticks.
    pub horizon: u32,
    /// Why the replay stopped early, or how the mission ended in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// op2-missionhost's notes on what the replay doesn't model: skipped script verbs, unsupported APIs, reads of
    /// the original game's memory it can't serve.
    pub warnings: Vec<String>,
    /// In the order they're announced.
    pub disasters: Vec<Disaster>,
}

/// One disaster the mission sets off.
#[derive(Debug, Clone, Serialize)]
pub struct Disaster {
    /// `METEOR`, `EARTHQUAKE`, `ERUPTION`, `LIGHTNING`, `TORNADO` or `BLIGHT`.
    pub kind: String,
    /// When it's announced, in ticks: one tick, or a window the mission's random numbers choose in. `exact` is false
    /// when no window can be given (the replay's tick is shown then).
    pub from: u32,
    /// The window's last tick (`from` for one tick).
    pub to: u32,
    /// `from` and `to` are a sure tick or window, not just the replay's tick.
    pub exact: bool,
    /// File tiles; `None` when the mission picks the place at random.
    pub at: Option<(i32, i32)>,
    /// A storm's or vortex's destination, or the far corner of a blight area.
    pub end_at: Option<(i32, i32)>,
    /// Size, magnitude, lava speed or duration.
    pub detail: String,
    /// The mission function that set it off.
    pub callback: String,
}

impl Disaster {
    /// The disaster as the game calls it (the API's tornado is the Vortex, lightning the Electrical storm).
    pub fn name(&self) -> &str {
        match self.kind.as_str() {
            "METEOR" => "Meteor",
            "EARTHQUAKE" => "Earthquake",
            "ERUPTION" => "Volcanic eruption",
            "LIGHTNING" => "Electrical storm",
            "TORNADO" => "Vortex",
            "BLIGHT" => "Blight",
            k => k,
        }
    }

    /// "at mark 12", "between marks 30 and 45", "around mark 50 (random)".
    pub fn when(&self) -> String {
        let mark = |t: u32| t as f32 / MARK as f32;
        if !self.exact {
            format!("around mark {:.0} (random)", mark(self.from))
        } else if self.from == self.to {
            format!("mark {:.0}", mark(self.from))
        } else {
            format!("marks {:.0} to {:.0}", mark(self.from), mark(self.to))
        }
    }

    /// Where: a tile, a path ("toward"), blight's spread, or "random".
    pub fn place(&self) -> String {
        match (self.at, self.end_at) {
            (Some((x, y)), Some((x2, y2))) if self.kind == "BLIGHT" => format!("{x}, {y} to {x2}, {y2}"),
            (Some((x, y)), Some((x2, y2))) => format!("{x}, {y} toward {x2}, {y2}"),
            (Some((x, y)), None) => format!("{x}, {y}"),
            _ => "random".into(),
        }
    }
}

/// Replays the mission at `start`'s difficulty for `marks` marks and collects the disasters it sets off.
pub fn forecast(dll: &Path, start: StartOptions, marks: u32) -> Result<Forecast> {
    let horizon = marks * MARK;
    let f = forecast::predict(dll, "", None, start.difficulty, 0, start.players, horizon)?;
    let stopped = f.error.clone().or_else(|| f.end.map(|(end, tick)| format!("the mission ended ({end:?}) at mark {}", tick / MARK)));
    let mut disasters: Vec<Disaster> = f
        .entries
        .into_iter()
        .map(|e| {
            let (from, to, exact) = match e.time {
                PredictedTime::At(t) => (t, t, true),
                PredictedTime::Window { from, to } => (from, to, true),
                PredictedTime::Unknown { candidate } => (candidate, candidate, false),
            };
            Disaster { kind: e.kind, from, to, exact, at: e.at, end_at: e.end_at, detail: e.detail, callback: e.callback }
        })
        .collect();
    disasters.sort_by_key(|d| d.from);
    Ok(Forecast { reached: f.reached, horizon, stopped, warnings: f.warnings, disasters })
}
