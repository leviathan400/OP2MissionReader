//! The part of op2dll other programs use (OP2Imager among them): running a mission DLL's setup in op2-rs's sandbox and
//! drawing the map with what it placed. Built without the `app` feature it needs no windowing libraries.

pub mod exact;
pub mod export;
pub mod groups;
pub mod inspect;
pub mod scene;
pub mod setup;
pub mod sprites;
pub mod triggers;
