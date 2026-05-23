//! Headless smoke: mount `welcome::app` into the terminal backend
//! and dump four frames at 0ms, 400ms, 1500ms, 3000ms so you can see
//! the three-act cinematic resolve in ASCII.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use backend_terminal::TerminalBackend;

fn dump(backend: &Rc<RefCell<TerminalBackend>>, label: &str) {
    let grid = backend.borrow_mut().render_to_grid();
    println!("\n=== {} ===", label);
    for r in 0..grid.rows {
        let mut line = String::new();
        for c in 0..grid.cols {
            line.push(grid.cell(c, r).map(|x| x.glyph).unwrap_or(' '));
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
    backend_terminal::install_global_self(Rc::downgrade(&backend));

    let _owner = framework_core::mount(backend.clone(), welcome::app);

    dump(&backend, "t = 0ms (mount)");
    drive_for(&backend, 400);
    dump(&backend, "t = 400ms (act 1)");
    drive_for(&backend, 1100);
    dump(&backend, "t = 1500ms (act 2 — sun bloom)");
    drive_for(&backend, 1500);
    dump(&backend, "t = 3000ms (act 3 — subtitle)");
}
