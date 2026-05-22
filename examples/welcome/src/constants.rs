//! Cross-cutting timing constants for the act schedule in
//! [`crate::app`]. Per-component constants live next to the
//! component that uses them.

/// Pause after page load before Act 1 begins.
pub const INTRO_PAUSE_MS: i32 = 400;

/// How long Act 1's phrase sits at rest before the dark wash.
pub const ACT_1_HOLD_MS: i32 = 1700;

/// Lag we schedule against before Act 2 — springs don't have a hard
/// duration so this is a hand-picked "roughly settled" budget.
pub const PHRASE_ENTER_BUDGET_MS: i32 = 900;

/// Dark wash duration. Slow + deliberate.
pub const DARK_FADE_MS: u64 = 1300;

/// How long after the dark wash starts before the sun-glare begins
/// blooming. The lag makes the glare read as arriving INTO the
/// dark scene, not painted with it. Also gates the unified raf
/// pulse driver — same scheduled offset.
pub const GLARE_LAG_AFTER_DARK_MS: i32 = 200;

/// Delay between the sun-glare starting and the content arriving.
/// Content enters *during* the bloom so the dark+glare+content
/// transition reads as one composed motion, not three beats.
pub const CONTENT_OFFSET_AFTER_GLARE_MS: i32 = 600;
