//! M1b authenticated owner view/control/frame transport. Source registration is
//! trusted and local; catalog, browser UI and remote sharing are separate stages.
#![forbid(unsafe_code)]
mod assets;
pub mod auth;
mod layer_catalog;
mod operations;
pub mod origin;
mod owner;
pub mod service;
mod stream;
pub mod transport;
pub mod view;
