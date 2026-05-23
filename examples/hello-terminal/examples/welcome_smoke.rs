//! Headless smoke: mount `welcome::app` into the terminal backend
//! and dump four frames at 0ms, 400ms, 1500ms, 3000ms so you can see
//! the three-act cinematic resolve in ASCII.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use backend_terminal::TerminalBackend;

fn dump_samples(backend: &Rc<RefCell<TerminalBackend>>, label: &str) {
    let grid = backend.borrow_mut().render_to_grid();
    let probes = [
        ("center        ", grid.cols / 2, grid.rows / 2),
        ("top-left      ", 1, 1),
        ("top-right     ", grid.cols - 2, 1),
        ("bottom-right  ", grid.cols - 2, grid.rows - 2),
        ("upper-right qr", grid.cols * 3 / 4, grid.rows / 4),
        ("planet zone   ", grid.cols / 3, grid.rows / 3),
    ];
    println!("\n[ {} ]", label);
    for (name, c, r) in probes {
        let cell = grid.cell(c, r).copied().unwrap_or_default();
        match cell.bg {
            Some(rgba) => println!(
                "  {} ({:>2},{:>2}) → rgb({:3},{:3},{:3}) a={}",
                name, c, r, rgba.r, rgba.g, rgba.b, rgba.a
            ),
            None => println!("  {} ({:>2},{:>2}) → none", name, c, r),
        }
    }
}

fn dump(backend: &Rc<RefCell<TerminalBackend>>, label: &str) {
    let grid = backend.borrow_mut().render_to_grid();
    println!("\n=== {} ===", label);
    // Render an ASCII heatmap of cell bg luminance — that way the
    // sun glare / vignette / planets are visible even though we
    // can't print ANSI escapes inside this dump format.
    // Legend: ' ' = no bg / dark; '·' = dim; '+' = mid; '*' = bright.
    for r in 0..grid.rows {
        let mut line = String::new();
        for c in 0..grid.cols {
            let cell = grid.cell(c, r).copied().unwrap_or_default();
            let glyph = cell.glyph;
            if glyph != ' ' {
                line.push(glyph);
                continue;
            }
            // Heatmap by luminance of bg, weighted by alpha.
            let l = match cell.bg {
                None => 0u32,
                Some(rgba) => {
                    let lum = (rgba.r as u32 * 299
                        + rgba.g as u32 * 587
                        + rgba.b as u32 * 114)
                        / 1000;
                    lum * rgba.a as u32 / 255
                }
            };
            line.push(match l {
                0..=24 => ' ',
                25..=80 => '·',
                81..=160 => '+',
                _ => '*',
            });
        }
        println!("{}", line.trim_end());
    }
}

fn drive_for(backend: &Rc<RefCell<TerminalBackend>>, ms: u32) {
    // Pump the scheduler at ~30 fps for `ms` real-time.
    let frame = Duration::from_millis(33);
    let mut elapsed = 0u32;
    while elapsed < ms {
        host_terminal::tick_scheduler_for_testing();
        let _ = backend.borrow_mut().render_to_grid();
        std::thread::sleep(frame);
        elapsed += 33;
    }
}

fn main() {
    host_terminal::install_scheduler_for_testing();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(80, 20);
    // Same `cell_size` the live `welcome` wrapper uses — gives the
    // welcome stylesheet a mobile-equivalent viewport instead of
    // letting its px values overflow.
    backend.borrow_mut().set_cell_size(8.0, 16.0);
    backend_terminal::install_global_self(Rc::downgrade(&backend));

    let _owner = framework_core::mount(backend.clone(), welcome::app);

    dump_samples(&backend, "t = 0ms (mount)");
    drive_for(&backend, 1500);
    dump_samples(&backend, "t = 1500ms (act 2 begins)");
    drive_for(&backend, 1500);
    dump_samples(&backend, "t = 3000ms");
    drive_for(&backend, 2000);
    dump_samples(&backend, "t = 5000ms (page dark, sun blooming)");
    dump(&backend, "heatmap at 5000ms");
}
