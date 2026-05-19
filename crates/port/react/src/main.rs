//! `port-react` CLI. All logic lives in `port_core::cli`.

fn main() -> std::process::ExitCode {
    port_core::cli::run("port-react", port_react::port)
}
