//! GTK4 / libadwaita front end of Penguin Player.
//!
//! See `docs/ARCHITECTURE.md` for how the pieces fit together. In short:
//! `context::AppContext` holds the shared state, `player::Player` drives playback,
//! `model` exposes the library as GTK list models, `services` do background work
//! and `ui` builds the window.
//!
//! Only [`run`] is public, so the compiler points out code nothing uses any more.

mod application;
mod context;
mod display;
mod model;
mod player;
#[cfg(debug_assertions)]
mod screenshot;
mod services;
mod signals;
mod ui;
mod util;

pub use application::run;
