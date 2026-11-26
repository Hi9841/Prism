#[cfg(feature = "gpui")]
pub mod palette;
#[cfg(feature = "gpui")]
pub mod palette_search_input;
pub mod phase1;
#[cfg(feature = "gpui")]
pub mod result_context_menu;
pub mod sections;

#[cfg(feature = "gpui")]
const FRAME_INSET: f32 = 20.;
#[cfg(feature = "gpui")]
const RESULT_ROW_INSET: f32 = 10.;
#[cfg(feature = "gpui")]
const RESULT_ICON_SIZE: f32 = 40.;
#[cfg(feature = "gpui")]
const RESULT_LABEL_GAP: f32 = 12.;
