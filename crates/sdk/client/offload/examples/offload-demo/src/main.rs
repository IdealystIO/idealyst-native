//! The app's entry point — every platform, one line (see `idealyst::entry!`).
//! On web the same wasm also boots inside offload's Web Workers, where the
//! boot returns before mounting (there is no `window`).
idealyst::entry!(offload_demo);
