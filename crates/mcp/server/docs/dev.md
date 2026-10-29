# Dev sessions

These tools launch, watch and stop `idealyst dev` from MCP. The loop is `run_dev` → `wait_for_app` → Robot tools → `stop_dev`, with `wait_for_build` to block until a save or build has landed (never by polling a log), `read_dev_log` and `dev_status` for detail, and `read_build_metrics` to see what each save cost.

They work on a session a PERSON started in a terminal too, not only on ones `run_dev` started: every `idealyst dev` (CLI 1.18 and later) registers itself in `~/.idealyst/dev-sessions/<pid>.json` and runs a loopback control endpoint, so `list_dev_sessions` shows it, `read_dev_log`, `dev_status` and `rebuild_dev` take its `pid`, and `stop_dev` stops it when told `include_external: true`. A person who watches a session themselves (often with `--no-watch`, so saves wait) and wants an agent to drive its rebuilds needs nothing else.

A project directory runs ONE session. Two would share its target directory, its `dev.log` and its metrics file, and their cargo builds would race (it surfaces as `can't find crate for …` in a registry crate). So when `list_dev_sessions` shows a session in the directory, use it — by `pid` — rather than starting another: `run_dev` refuses, and so does `idealyst dev` itself.

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
- `no_watch`: saves are queued, not applied; `rebuild_dev` applies the queue as one save (overlay patch, hot patch or rebuild, as the save would have been). See `rebuild_dev`.
- `experimental_build` (web `local` only): a save that only ADDS items — a helper and its first call, a new component, a type and its impls — hot patches instead of rebuilding. See "Adding items" in the framework's `docs/hot-reload.md`.
- `metrics`: record what every save cost — its tier and why, each stage's time, and for a cargo build every crate recompiled with its compile time and cargo's reason. Read the records with `read_build_metrics`.
- `metrics_diffs`: `metrics`, plus each saved file's diff against the previous save. Keeps a copy of every watched text file in memory for the session.
- `dir`: project directory; defaults to the server's working directory.

Refuses, naming the running session's pid and who started it, when a session already runs in `dir` (see above): use that one, or `stop_dev` it first.

Returns `{ id, pid, dir, platforms, local, no_robot, log_path, status }`. Output goes to `~/.idealyst/dev-logs/<name>-<id>.log` (`log_path`); follow it with `read_dev_log`. The process runs until `stop_dev`.

Rust edits are picked up WITHOUT re-running this tool. The default runtime-server mode hot-patches the running app on save and keeps component state. `local` mode's watcher rebuilds and reloads: web does a full page reload, native platforms restart the app, and state is lost.

The tool returns as soon as the build starts: `wait_for_build` waits for the first build to finish (and reports its errors if it fails), then `wait_for_app` before calling Robot tools.

## wait_for_app

Blocks until a running app registers its Robot bridge, then returns it in `list_apps` shape (with the `selector` to pass as `app`). It bridges `run_dev`, which returns as soon as the build starts, and the Robot tools, which need the app up.

- `name` waits for a specific app (the registration name); omit it to wait for ANY app.
- `timeout_secs` defaults to 60, max 600.

On timeout it returns an error that says which wait failed: an app registered but never answered a ping (the page or process hasn't dialed in — for web, `run_dev` needs `local=true` and something must load the page), or nothing registered at all, with the state of `~/.idealyst/apps` (missing, not creatable, not writable, or empty). For the latter, `read_dev_log` shows whether the build is still running or failed, and warns if the relay could not write its registration. Errors immediately when robot control is disabled (`--no-robot`).

## wait_for_build

Blocks until a session's build or save has landed, then says how it ended. Use it after every save, and after `run_dev` for the first build, instead of polling `read_dev_log` or `dev_status`: it reads the session's typed event stream, so it knows every way a save can end, and it ALWAYS returns, at the latest when its timeout runs out.

Name the session by `session_id` or `pid` (a person's session works too).

- `after`: a `cursor` from an earlier answer. The wait is for an outcome that arrives after it: the build of a save made after the cursor, a build that was already running when the cursor was taken, or a `--no-watch` queue that `rebuild_dev` applies. If that outcome is already in, the answer is immediate. If nothing is building and nothing new starts within 1.5 s, the answer is `idle`: a save that changed no bytes, or a cursor taken after the save it meant to cover, never waits out the timeout for a build that will not come.
- `request`: the `request` number `rebuild_dev` returned. The wait is for the builds that apply that request, one per watcher it went to (the web bundle, a full-stack server, …), and for nothing after them: when two agents request rebuilds, and the second request arrives while the first's build runs, that build answers the first agent and the next build the second. If the request was already applied, even with later saves built since, the answer is immediate. A number the session never gave answers `unknown_request` at once. Pass `request` or `after`, not both.
- Without `after`: a build in progress is waited for. With nothing building, the same 1.5 s for a save made just before the call to be noticed, then `idle`.
- `timeout_secs`: default 120, max 600.

The loop: `wait_for_build` → keep its `cursor` → edit and save → `wait_for_build` with `after: <cursor>` → … A save that lands as a hot patch or overlay patch moves no page generation, so the cursor, not `gen`, is how a build is tracked.

Returns `{ status, cursor, builds, next }`, plus `building`, `preparing`, `queued`, `last`, `log` where they apply:

- `status`: `finished`, `failed`, `building` (still running at the timeout: call again with `after`, or `request`, from `next`; with `request`, `waiting_for` names the targets that have not applied it yet), `preparing` (the session's watcher is still starting up, scanning the workspace, which takes seconds after the first build on a large one: a save made now is held and applied once it is ready, so the wait does not answer `idle` until it is; call again), `queued` (`--no-watch` saves wait for `rebuild_dev`; waiting would wait for nothing), `idle` (nothing built after the cursor; `last` holds each target's latest outcome), `unknown_request` (a `request` this session never gave), or `session_ended` (the session exited mid-wait).
- `builds`: per target, `{ target, id, closed_seq, outcome, failed, tier, reason, gen, ms, error, hot_patch_failed, superseded, errors, requests }`. `id` is the `seq` of the event that started the build, `closed_seq` the one that ended it. `outcome` is `ready`, `reloaded`, `unchanged`, `premint_refreshed`, `hot_patch`, `overlay_patch`, `no_change`, `sidecar_hot_patch`, `sidecar_respawn`, or `failed`. `gen` is the page generation a rebuild loaded. A save superseded by a newer one is followed to the restart's outcome (`superseded` counts them). A hot patch that could not be built shows `hot_patch_failed`, and the rebuild that replaced it is the outcome. `requests` lists the `rebuild_dev` requests a build applied.
- On `failed`: `error` is how the build stopped (`cargo exited with 101`), `errors` the rustc errors (up to 20, each `{ message, code, location, rendered }`, `location` as `file:line:col`), and `log` the session's `dev.log`, the full output: `read_dev_log` with `filter: "error"` reads it.
- `building`: per target in progress, `{ id, tier, stage, compiled, total, current, errors_so_far }`.
- `cursor`: the highest event `seq` seen; pass it as `after` next time. `next` says what to call next.

A full-stack session builds the web bundle and the server on separate watchers. The wait answers once neither has started anything for 400 ms after the last outcome, so both are in `builds`.

Errors only when the session cannot be found or its event stream does not answer (a session whose `idealyst` predates the stream says so); everything after that is an answer.

## read_dev_log

Reads the tail of a session's log: the way to follow build progress and see compile errors without filesystem access. Name the session by `session_id` (from `run_dev`: the log this server captured) or `pid` (any session in `list_dev_sessions`, a person's included: its `dev.log`).

- `lines`: lines from the end, default 100, max 2000.
- `filter`: case-insensitive substring applied BEFORE the tail, so `filter: "error"` returns the last N matching lines, which jumps straight to a compile failure.

A `run_dev` session's log stays readable after it exits and is pruned from `list_dev_sessions` (the file is found by globbing `~/.idealyst/dev-logs/`). Returns `{ session_id, pid, log_path, lines, content }`.

## read_build_metrics

What each save cost, from the records a session started with `metrics` (or `idealyst dev --metrics`) appends: one per save and one per initial build, in `<project>/target/idealyst/<package>/build-metrics.jsonl`. Records accumulate across sessions, so this is how to tell a seconds-long rebuild from a minutes-long one and why.

Name the records by `session_id` or `pid` (a running session, a person's included) for that session's records only, or by `dir` for every session's records of a project, whether or not a session runs (default the server's working directory). Every session of a project appends to the same file, and each record carries the pid of the session that wrote it (`session`); records from an `idealyst` that predates it carry none, and only `dir` reads them.

- `last`: the newest N matching records, oldest first. Default 10, max 200. Records are ordered by when their work STARTED: one is written when its work ends, so a long build lands in the file after quicker ones begun later.
- `rebuilds_only`: only records whose work ran cargo — rebuilds (a hot patch that fell back to one included) and the initial and requested builds, not overlay or hot patches.
- `detail`: the records as written, with every recompiled crate and the diffs. Without it each record is a summary.

Returns `{ path, total, matching, records, session? }`. An answer stays under 24 000 characters: when the records asked for do not fit, the oldest are left out, `omitted_for_size` counts them and `note` says so — ask for fewer with `last`, or read one record from `path` by its `line`. A summary is `{ line, session, started, target, kind, trigger, tier, reason, outcome, failed, total_ms, changes, changes_total, stages, cargo, hot_patch, diffed_files, diffed_files_total }`, each present when the record has it:

- `line`: the record's line in `path`, where the full record is.
- `kind`: what the work was — `initial build`, `requested rebuild` (`rebuild_dev`, the panel's `r`), `rebuild`, `hot patch`, `rebuild after a failed hot patch`, `overlay patch`, `no change`, `save, not built` (superseded before it built), `one-shot build`. Every record has one; a full-stack server's saves have no `tier`, so their `kind` is `rebuild`.
- `changes`: the first ten saved files, each with its `kind` (`rust source`, `manifest`, `lockfile`, `build script`, `config`, `asset`, `deleted`); `changes_total` counts them when there are more (a branch switch saves thousands). `diffed_files` and `diffed_files_total` likewise.
- `tier` and `reason`: what the watcher decided (`overlay`, `hot_patch`, `rebuild`, `unchanged`) and why; absent when nothing was decided (the initial and requested builds, a server's saves).
- `hot_patch`: a hot patch the save attempted: `steps`, each `{ name, ms }` in order (`decide`, `seed-wait`, `cargo`, `link`, `scan-wait`, `install`, …), kept when the patch failed or was superseded; and for a built one `crates` (each re-emitted crate's replay `ms`, `null` when its objects were reused), `redirected` and `bytes`.
- `cargo`: `{ recompiled, packages_seen, root_causes, slowest }`. `root_causes` are the reasons other than "a dependency was rebuilt", each `{ kind, detail, packages, package_count }`. `slowest` is the ten slowest recompiled crates with `ms` and reason kinds.

Reading a slow rebuild: the root causes say what started it. `file_changed` names an edited file in a crate low in the graph. `features_changed`, `rustflags_changed` and `profile_configuration_changed` say how the crates are compiled changed. `no_prior_build` across many crates means cargo had no build of them with these settings: two builds sharing a target dir with different features, or removed artifacts. Crate times overlap (cargo builds in parallel), so they do not sum to `total_ms`.

Per-crate detail covers the builds the session runs through cargo itself: the web bundle, a full-stack server, the runtime-server sidecar. Native targets' records have stage times only. With no records yet the result has `total: 0` and a `note` on how to start recording.

## list_dev_sessions

Every running `idealyst dev` session on this machine: the ones a person started and the ones this server launched with `run_dev`.

`[{ pid, session_id, started_by, dir, app, targets, local, no_watch, experimental_build, log_path, uptime_secs, control }]`:

- `started_by` is `this server`, `a person`, `another idealyst mcp` (one still running: another agent's), or `an idealyst mcp that has exited` (an update or a client restart replaced the server that started it; this server treats it as its own, see `stop_dev`); `session_id` is set only for sessions this server started.
- `no_watch` says saves wait for `rebuild_dev`.
- `control` is false for a session this server launched that has not registered yet (it registers as soon as it starts reporting), or one whose `idealyst` predates the registry. That one shows this server's `{ id, platforms, status, … }`, where `status` is `running` or `exited(<code>)`. An exited session is reported once, then dropped.

A session that crashed leaves its registry entry behind; it is pruned here once its pid is gone.

## dev_status

One session's current state, from its control endpoint, folded into a summary:

- `session`: its registry entry (project, targets, `no_watch`, `experimental_build`, log path, who started it, …).
- `targets`: per target (`web`, `server`, `ios`, …) `latest`, its latest outcome as `wait_for_build` reports one (`outcome`, `failed`, `tier`, `reason`, `ms`, `gen`, `errors`); `building`, the work in flight with its `stage` and cargo progress; `queued: true` for saves waiting for `rebuild_dev` under `--no-watch`; `preparing: true` for a watcher still starting; `watched_roots`, how many paths it watches; `warnings`, its last build's warning count; `urls`, what it serves; `server_down`; `page`, the page's last acknowledgement.
- `started`: the session's mode and hot-reload tier; `urls`: the session's own event stream; `robot_relay` and `robot_apps`: its Robot relay and the apps connected to it.
- `cursor`: the highest event `seq` seen, to pass to `wait_for_build` as `after`.
- `other`: any event this server does not know, as sent.

`detail: true` returns what the session sent instead, `{ session, events }`: `events` is the session as the events that describe it now — the same snapshot its `/__idealyst/events` stream opens with — every watched root and up to 64 diagnostics included.

Takes `session_id` or `pid`, and `detail`.

## rebuild_dev

Asks a session to rebuild now, exactly as pressing `r` in its terminal panel does: every watcher it runs (the web bundle, a full-stack server, the runtime-server host, app icons) gets the request. Under `--no-watch`, a watcher with queued saves applies them as ONE save, through the same decision a save takes — so a literal edit still lands as an overlay patch and a body edit as a hot patch; with nothing queued, it rebuilds. Returns as soon as the request is taken: `{ request, requested: { accepted, request, targets, no_watch } }`, where `request` is the session's number for this request and `targets` the watchers that took it. To wait for the outcome, call `wait_for_build` with `request: <request>`: it waits for exactly the builds that apply it, not a later request's. A session from an older CLI returns no `request`; wait on it with `after:` a cursor taken before the request. `read_dev_log` has the detail.

Takes `session_id` or `pid`, and works on a person's session as on this server's own. A session started with `--no-build` has no watcher to take the request, and says so.

## stop_dev

Stops a session and returns what happened. Name it by `session_id` or `pid`, or pass `all: true`.

- This server's own sessions stop as they always have: on unix the whole dev process GROUP is signalled (the `idealyst dev` process and its cargo builds, web servers and simulators), SIGINT first so simulators and servers shut down cleanly, escalating to SIGKILL after a grace window.
- A session an `idealyst mcp` started and that server has since exited (an update or a client restart leaves its sessions running) has no owner: this server stops it as its own, by signal as below, without `include_external`.
- A session a PERSON started, or one another RUNNING `idealyst mcp` started (another agent's), is stopped only with `include_external: true`. It is stopped by signal, not through its control endpoint, so this works on a session that has hung and no longer answers: SIGINT to its process group (or to it alone, if it does not lead one), 10 seconds for its own teardown (a terminal panel restores the terminal), then SIGKILL. Its registry entry is removed. The result says `Gracefully`, `Killed` or `AlreadyGone`.
- `all: true` stops this server's sessions and those an exited server left; with `include_external: true`, every session on the machine.
