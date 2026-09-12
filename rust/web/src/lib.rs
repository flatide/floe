//! M1b authenticated owner view/control/frame transport. Source registration is
//! trusted and local; catalog, browser UI and remote sharing are separate stages.
#![forbid(unsafe_code)]
pub mod auth;
pub mod origin;
mod stream;
pub mod transport;
pub mod view;
