//! aether-web: Aether's Rust/WASM frontend (vertigo). See README.md.

mod api;
mod app;
mod auth;
mod brand;
mod components;
mod i18n;
mod icons;
#[rustfmt::skip]
mod icons_data;
mod pages;
mod player;
mod position;
mod prefs;
mod route;
mod shell;
mod station;
mod style;
mod theme;
mod tracklist;
mod ui;

use vertigo::{DomNode, main};

#[main]
fn app() -> DomNode {
    app::render()
}
