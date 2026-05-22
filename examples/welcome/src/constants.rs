//! Cross-cutting timing constants used by the act schedule in
//! [`crate::app`]. Per-component constants (motion ranges, palette
//! choices, etc.) live next to the component that uses them.

// ---- Timing (milliseconds) ----------------------------------------------

/// Pause after page load before Act 1's phrase begins entering.
pub const INTRO_PAUSE_MS: i32 = 400;

/// Act 1 hold — how long the welcome phrase sits at rest before
/// the dark wash begins.
pub const ACT_1_HOLD_MS: i32 = 1700;

/// Welcome phrase enter — how long the spring takes to roughly
/// settle (springs don't have a hard duration; this is the lag we
/// schedule against before Act 2 begins).
pub const PHRASE_ENTER_BUDGET_MS: i32 = 900;

/// Dark wash duration. Slow, deliberate.
pub const DARK_FADE_MS: u64 = 1300;

/// How long after the sun-glare starts blooming the content arrives.
/// The content enters *during* the glare's bloom so the scene
/// transformation (welcome out → dark + glare in → content in) reads
/// as one composed motion rather than three sequential beats.
///
/// Tuned by ear: long enough that the welcome phrase is well past
/// gone before content lands; short enough that the glare and
/// content are visibly arriving together.
pub const CONTENT_OFFSET_AFTER_GLARE_MS: i32 = 600;
