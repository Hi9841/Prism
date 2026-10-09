//! Prism launcher UI, ported from React to GPUI (see `migration/RULEBOOK.md`).

// `lib` mirrors Prism's `src/lib/` (RULEBOOK §4); this crate's root is not `lib.rs`.
#![allow(special_module_name)]

pub mod features;
pub mod js;
pub mod lib;
pub mod state;
