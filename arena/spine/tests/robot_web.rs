//! Arena robot-on-web integration (`#[ignore]`): stand up the relay + a
//! headless browser around a pre-built robot bundle, then run the real
//! `RobotVerifier` on a `robot`-tier item and assert it introspects the live
//! browser app. Proves the arena consumes robot-on-web end to end.
//!
//! Build a bundle once:
//! ```text
//! cd /tmp && rm -rf rw && mkdir rw && cd rw
//! IDEALYST_FRAMEWORK_PATH=<repo> <repo>/target/debug/idealyst new app
//! <repo>/target/debug/idealyst build --web --robot /tmp/rw/app
//! ```
//! Run:
//! ```text
//! ARENA_ROBOT_DIST=/tmp/rw/app/dist/web \
//!   cargo test -p arena-spine --test robot_web -- --ignored --nocapture
//! ```

use arena_spine::harness::robot_web;
use arena_spine::rubric::{Assertion, ItemClass, RubricItem, Tier};
use arena_spine::verify::robot::RobotVerifier;
use arena_spine::verify::{RunContext, Verifier};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

struct Serve(std::process::Child);
impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "needs a headless browser + a pre-built robot web bundle (see header)"]
fn robot_tier_introspects_the_web_app_through_the_relay() {
    let dist = PathBuf::from(
        std::env::var("ARENA_ROBOT_DIST")
            .expect("set ARENA_ROBOT_DIST to a `idealyst build --web --robot` dist/web dir"),
    );
    assert!(dist.join("index.html").is_file(), "no bundle at {}", dist.display());
    // <project>/dist/web → <project>
    let project_dir = dist.parent().unwrap().parent().unwrap().to_path_buf();

    let run_dir = std::env::temp_dir().join("arena_robot_web_test");
    let _ = std::fs::remove_dir_all(&run_dir);
    std::fs::create_dir_all(&run_dir).unwrap();

    // Relay + URL injection (before serving), then serve, then load the page.
    let relay = robot_web::start_relay(&project_dir, &dist).expect("relay starts + injects");
    let port = free_port();
    let _server = Serve(
        Command::new("python3")
            .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"])
            .arg("--directory")
            .arg(&dist)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 http.server"),
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    let base_url = format!("http://127.0.0.1:{port}");
    let _host = robot_web::host(relay, &base_url, &run_dir).expect("browser app dials the relay");

    // A robot-tier item: the app exposes a "Welcome…" element, found via the
    // relay-fronted Robot bridge. This is the exact path verify::robot takes.
    let item = RubricItem {
        id: "robot-welcome".into(),
        description: String::new(),
        points: 10,
        class: ItemClass::Outcome,
        tier: Tier::Robot,
        verifier: "robot".into(),
        depends_on: None,
        assertion: Assertion {
            expect_name: Some("Welcome".into()),
            ..Default::default()
        },
    };
    let ctx = RunContext::source_only(project_dir);
    let result = RobotVerifier.verify(&item, &ctx);
    assert!(
        result.passed,
        "robot tier should find the app element via the relay: {}",
        result.evidence
    );
    println!("✅ arena robot-on-web verified: {}", result.evidence);
}
