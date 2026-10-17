# Dev sessions

These tools launch, watch and stop `idealyst dev` from MCP. The loop is `run_dev` → `wait_for_app` → Robot tools → `stop_dev`, with `read_dev_log` to follow the build.

## list_apps

Every running idealyst app, as `{ name, selector, bundle_id, project_root, bridge_addr, pid, … }`. Pass `selector` as the `app` argument of Robot tools (see `get_docs("app")`).

Apps are discovered from the per-process registration files `~/.idealyst/apps/<name>-<pid>.json` that an app's Robot bridge writes on bind. An entry disappears when the app exits: it is removed on graceful shutdown, and a stale one is pruned at scan time once `kill(pid, 0)` reports the process gone.

It also lists a dev session's pinned Robot relay from `<project>/.idealyst/robot.json` in this server's working directory or above (a devcontainer's app, seen from the host through its forwarded port) while that port answers.

## run_dev

Launches an `idealyst dev` session, the MCP equivalent of running `idealyst dev` in a terminal. The CLI is spawned detached, so the MCP connection never blocks, and the process is TRACKED so `stop_dev` can tear the whole session down. One call can run several platforms at once; they run in parallel inside the single dev process.

Arguments:

- `platforms`: any of `web`, `ios`, `android`, `macos`, `terminal`, `linux`, `windows` (the last two are native desktop targets and build only on that host). Omit, with `all` false, to use the project manifest's `targets`.
- `all`: every platform the host can build (web + android everywhere, plus ios + macos on macOS hosts).
- `local`: build natively per platform with a file-watch rebuild loop, bypassing the runtime-server wire. State does not survive saves. Default false (runtime-server hot reload). For WEB targets, Robot introspection requires `local: true`; the robot feature and relay dial-out are only wired in local mode.
- `headless_client`: force a headless Chromium at the served web URL. Usually unnecessary: when the relay is active and there is no display (container/CI), dev launches one automatically so the web app loads and registers.
- `no_robot`: disable the Robot bridge/relay. Default false, so the Robot tools can drive the app.
- `bridge_port`: pin the Robot bridge to a fixed port instead of an ephemeral one.
- `screenshot_dir`: where the relay saves screenshot PNGs.
- `no_build` (web only): skip the initial build and just serve.
- `dir`: project directory; defaults to the server's working directory.

Returns `{ id, pid, dir, platforms, local, no_robot, log_path, status }`. Output goes to `~/.idealyst/dev-logs/<name>-<id>.log` (`log_path`); follow it with `read_dev_log`. The process runs until `stop_dev`.

Rust edits are picked up WITHOUT re-running this tool. The default runtime-server mode hot-patches the running app on save and keeps component state. `local` mode's watcher rebuilds and reloads: web does a full page reload, native platforms restart the app, and state is lost.

The tool returns as soon as the build starts; use `wait_for_app` before calling Robot tools.

## wait_for_app

Blocks until a running app registers its Robot bridge, then returns it in `list_apps` shape (with the `selector` to pass as `app`). It bridges `run_dev`, which returns as soon as the build starts, and the Robot tools, which need the app up.

- `name` waits for a specific app (the registration name); omit it to wait for ANY app.
- `timeout_secs` defaults to 60, max 600.

On timeout it returns an error. If you launched with `run_dev`, check `read_dev_log` for a build still running or a compile failure. Errors immediately when robot control is disabled (`--no-robot`).

## read_dev_log

Reads the tail of a `run_dev` session's log: the way to follow build progress and see compile errors without filesystem access.

- `lines`: lines from the end, default 100, max 2000.
- `filter`: case-insensitive substring applied BEFORE the tail, so `filter: "error"` returns the last N matching lines, which jumps straight to a compile failure.

Logs stay readable after the session exits and is pruned from `list_dev_sessions` (the file is found by globbing `~/.idealyst/dev-logs/`). Returns `{ session_id, log_path, lines, content }`.

## list_dev_sessions

Sessions this server launched with `run_dev`: `[{ id, pid, dir, platforms, local, no_robot, log_path, uptime_secs, status }]`, where `status` is `running` or `exited(<code>)`. An exited session is reported once, with its exit status, then dropped on the next call. Apps launched from a terminal show up in `list_apps`, not here.

## stop_dev

Stops one tracked session (`session_id`) or all of them (`all: true`), and returns their final info. On unix the whole dev process GROUP is signalled (the `idealyst dev` process and its cargo builds, web servers and simulators): SIGINT first so simulators and servers shut down cleanly, escalating to SIGKILL after a grace window. A stopped session is removed from `list_dev_sessions`.
