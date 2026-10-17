# Selecting an app or crate

Two different arguments are both called `app`. Which one a tool takes depends on what it reads.

## Running-app selector (Robot and dev tools)

Robot tools (`find_element`, `click`, `screenshot`, …) act on a **running** app. Each running app registers a Robot bridge, and `list_apps` shows what is up. Each entry has a `selector` field. Pass that value as `app`.

- With exactly one app registered, `app` is optional.
- With several registered, it is required. The error lists the candidates.
- `selector` is the app `name`, or `name:platform` when one app runs on two platforms (`todo:web` vs `todo:macos`).
- `compare_native_parity` takes two selectors, `app_a` and `app_b`.

Apps are discovered from per-process registration files at `~/.idealyst/apps/<name>-<pid>.json` (see `list_apps`). With `--robot-port`, the server talks to that one bridge and ignores discovery. With `--no-robot`, every Robot tool returns an error.

## Crate filter (catalog tools)

Catalog tools (`list_components`, `describe_component`, …) read the project **catalog**. One catalog can span every crate in a workspace, so `app` here restricts results to components from ONE crate: an app, or a shared component library.

- Either spelling works: `my-app` or `my_app`.
- Each `list_*` result has an `app` field telling you what to pass.
- On a `describe_*` / `find_*` lookup, a short name that matches components in several crates (two apps each defining `AppRoot`) is reported as ambiguous, with its candidates. Pick one by passing `app` or a fully-qualified `name`.
