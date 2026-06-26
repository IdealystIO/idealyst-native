//! Minimal standalone relay: start it, print the ports, park. Used to verify a
//! native app's dial-out transport (run an app with
//! `IDEALYST_ROBOT_RELAY_URL=ws://127.0.0.1:<WS_PORT>`, then drive verbs against
//! `TCP_PORT`).

use std::io::Write;

fn main() {
    let relay = robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: false,
        identity: None,
        screenshot_dir: None,
    })
    .expect("relay starts");
    println!("WS_PORT={}", relay.ws_addr.port());
    println!("TCP_PORT={}", relay.tcp_addr.port());
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
