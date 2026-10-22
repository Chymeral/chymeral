//! Shared implementation for the Chymeral executable and its integration tests.
//!
//! Chymeral's supported application boundary is the language-neutral Chymeral
//! Link Protocol (CLP), not this Rust crate's module layout. Application
//! authors should use a CLP client or a language SDK such as
//! `chymeral-rust-sdk`; these host modules are intentionally not a stable Rust
//! API.

pub mod animated_image;
pub mod external_drop;
pub mod interaction;
pub mod program_scene;
pub mod protocol;
pub mod resources;
pub mod runtime;
pub mod scene;
pub mod scene_text;
pub mod transport;
pub mod window_metadata;
