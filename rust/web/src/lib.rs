//! M1b authenticated owner view/control/frame transport. Source registration is
//! trusted and local; catalog, browser UI and remote sharing are separate stages.
#![forbid(unsafe_code)]
pub mod about;
mod assets;
pub mod auth;
pub mod browse;
mod defaults;
pub mod drc;
mod exports;
mod idle_io;
pub mod launch;
mod layer_catalog;
mod operations;
pub mod origin;
mod owner;
mod prepared;
mod query;
pub mod service;
mod settings;
mod stream;
pub mod transport;
pub mod view;
mod window_display;
