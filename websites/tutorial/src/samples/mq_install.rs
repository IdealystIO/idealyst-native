use runtime_core::{install_breakpoints, Breakpoints};

// Call once before mounting (or before the first current_breakpoint read).
// First install wins.
fn use_our_scale() {
    install_breakpoints(Breakpoints {
        sm_min: 600.0,
        md_min: 900.0,
        lg_min: 1200.0,
        xl_min: 1600.0,
    })
    .ok();
}
