//! Prism launcher UI, ported from React to GPUI (see `migration/RULEBOOK.md`).

// `lib` mirrors Prism's `src/lib/` (RULEBOOK §4); this crate's root is not `lib.rs`.
#![allow(special_module_name)]

#[cfg(feature = "gpui")]
pub mod app;
#[cfg(feature = "gpui")]
pub mod components;
pub mod features;
#[cfg(feature = "gpui")]
pub mod gfx;
pub mod js;
pub mod lib;
pub mod state;
#[cfg(feature = "gpui")]
pub mod theme;
