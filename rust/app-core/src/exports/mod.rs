//! Managed exports. No HTTP, browser paths, uploads or sharing permissions.
//! A caller must bind the registered dataset/revision and artifact IDs to its
//! authenticated owner. This module cannot authenticate a user by itself.
pub mod artifacts;
pub mod clip;
