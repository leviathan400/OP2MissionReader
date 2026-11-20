//! Work for a window run on a thread of its own (the mission replays behind the disaster forecast and the messages),
//! keyed by what it was run for so a window can tell when it's out of date.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use crate::setup::StartOptions;

/// The mission, start and length (in marks) a job ran for.
pub type Key = (PathBuf, StartOptions, u32);

/// One background run and its result.
pub struct Job<T> {
    /// What the job was run for; a window compares it with its current key to see whether the result is stale.
    pub key: Key,
    /// The result once it has arrived (`poll` fills it), the error as text.
    pub result: Option<Result<T, String>>,
    /// The channel the thread sends its result on; `None` once it has been received.
    rx: Option<Receiver<Result<T, String>>>,
}

impl<T: Send + 'static> Job<T> {
    /// Runs `work` on a new thread and repaints `ctx` when it's done.
    pub fn start(ctx: &egui::Context, key: Key, work: impl FnOnce(&Key) -> anyhow::Result<T> + Send + 'static) -> Job<T> {
        let (tx, rx) = channel();
        let (k, ctx) = (key.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work(&k).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Job { key, result: None, rx: Some(rx) }
    }

    /// The result, once the thread has sent it.
    pub fn poll(&mut self) -> Option<&Result<T, String>> {
        if let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.result = Some(r);
            self.rx = None;
        }
        self.result.as_ref()
    }

    /// The finished result, if it succeeded.
    pub fn done(&self) -> Option<&T> {
        self.result.as_ref()?.as_ref().ok()
    }
}
