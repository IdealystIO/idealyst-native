//! No custom cfgs are surfaced by this build script anymore — the
//! previous `webview_node` / `blitz_active` flags belonged to the
//! WebView primitive's wgpu backing, which has been removed. The
//! script is retained as a stub so changes to the build pipeline
//! can land here without re-adding it.

fn main() {}
