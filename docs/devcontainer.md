# Dev containers — `idealyst configure devcontainer`

`idealyst configure` is the home for one-off "set up this aspect of the
project" actions. Its first subcommand, `configure devcontainer`, initializes
(or updates) a [Dev Container](https://containers.dev/) for a project and lets
you toggle optional add-ons: the sidecar services a real app needs — a
database, Redis, an S3-compatible object store — and AI coding agents (Claude
Code, Codex) installed inside the container with your host login carried over.

```
idealyst configure devcontainer            # interactive wizard
idealyst configure devcontainer --non-interactive --database=postgres --redis
idealyst configure devcontainer --non-interactive --claude --codex
```

## What it generates

The command owns exactly one file and appends one reference. Everything else
is yours to edit.

```
.devcontainer/
  devcontainer.json               # yours — we only ensure it lists the managed file
  docker-compose.yml              # yours — the base `dev` service; never rewritten
  docker-compose.idealyst.yml     # OURS — regenerated wholesale every run
```

- **`docker-compose.idealyst.yml`** holds every idealyst-managed service. It is
  regenerated in full on each run, so don't hand-edit it. Docker Compose merges
  the two compose files, so the managed services join the same network (each
  reachable by its service name) and a small `<dev>:` override merges connection
  env vars + `depends_on` into your base dev service.
- **`devcontainer.json`** is touched to make sure its `dockerComposeFile`
  array lists `docker-compose.idealyst.yml` (at most once), and — only when an
  AI agent is enabled — to add its `features` / keyed `postCreateCommand`
  entries (see [AI coding agents](#ai-coding-agents) for the ownership rules).
- If no devcontainer exists yet, a minimal compose-based one is **scaffolded**
  (a Rust dev container + the two compose files).

### Ownership — we never touch your services

Every managed service has an idealyst **canonical name** (`database`, `redis`,
`minio`) and lives only in the managed file. If you add your own `minio` (or
anything else) to `docker-compose.yml` under a different key, it is never in our
file and never touched. The managed file also carries an `x-idealyst` metadata
block (a `x-`-prefixed key Docker Compose ignores) recording which services are
enabled — that's how a re-run knows what to preselect.

## Services

| Canonical name | Variants | Injects into the dev service |
| --- | --- | --- |
| `database` | `postgres` (default), `mysql` | `DATABASE_URL` |
| `redis` | — | `REDIS_URL` |
| `minio` | — | `MINIO_ENDPOINT`, `MINIO_ACCESS_KEY`, `MINIO_SECRET_KEY` |

Each service also declares a named volume so its data survives container
rebuilds. No host ports are published — the dev container reaches each sidecar
by service name on the compose network.

## AI coding agents

Two more registry entries install an AI agent CLI *inside* the dev container
instead of running a sidecar:

| Canonical name | Installs via | Variants | Injects into the dev service |
| --- | --- | --- | --- |
| `claude` | The **native installer** (`curl -fsSL https://claude.ai/install.sh \| bash`) in a keyed `postCreateCommand` — no Node.js needed. Anthropic's devcontainer feature is deliberately *not* used: it still wraps the deprecated npm package and its Node auto-install fails on non-Node base images. | `host` (default), `volume` | `CLAUDE_CONFIG_DIR` + a config-dir mount |
| `codex` | `ghcr.io/devcontainers/features/node` + a keyed `postCreateCommand` running `npm install -g @openai/codex` (npm is Codex's official channel; OpenAI publishes no devcontainer feature) | `host` (default), `volume` | `CODEX_HOME` + a config-dir mount |

The variant picks where the CLI's config directory — credentials, settings,
history — lives:

- **`host`** (default): bind-mounts the host's `~/.claude` / `~/.codex` into
  the container, so an existing host login **carries over automatically** — no
  re-login inside the container. The mount lands at a fixed path
  (`/idealyst/agents/<id>`) because a compose mount target can't reference the
  container user's home; `CLAUDE_CONFIG_DIR` / `CODEX_HOME` point the CLI at
  it. Note for macOS Claude Code users: if your host login lives in the
  Keychain rather than `~/.claude/.credentials.json`, the first run in the
  container asks you to log in once; the credential is then written to the
  mounted dir and it's automatic from there. To skip even that one login you
  can seed the file from the Keychain yourself —
  `security find-generic-password -s "Claude Code-credentials" -w > ~/.claude/.credentials.json && chmod 600 ~/.claude/.credentials.json`
  — with the caveat that host (Keychain) and container (file) then hold the
  same grant and refresh it independently.
- **`volume`**: a named Docker volume — nothing shared with the host. Log in
  once per project; the login survives container rebuilds. A keyed
  `postCreateCommand` chowns the volume to the container user (fresh named
  volumes mount root-owned).

## The `idealyst` CLI

`idealyst-cli` installs the `idealyst` CLI itself into the container — an
idealyst project's devcontainer wants it for `idealyst dev` / `lint` /
`configure`. The only distribution channel today is a source build
(`cargo install --git`), which is slow cold, so the install root lives in a
named volume (`idealyst-cli-cache`) and the keyed `postCreateCommand` skips
the build when the cached binary already exists — you pay the compile once
per project, not per rebuild. A `/usr/local/bin/idealyst` symlink (refreshed
each create) puts it on PATH. Requires a Rust toolchain + git in the base
image (true for the scaffolded Rust base). To force-refresh a cached CLI, run
`cargo install --force --git https://github.com/IdealystIO/idealyst-native idealyst-cli --root /idealyst/cli`
in the container, or drop the volume.

### Ownership in `devcontainer.json`

Agent installs are the one case where idealyst edits more of your
`devcontainer.json` than the `dockerComposeFile` list: it adds `features`
entries and keyed `postCreateCommand` entries (object form, `idealyst-`
prefixed keys). Which keys idealyst added is recorded in the managed compose
file's `x-idealyst.devcontainer` block, and **removal only ever deletes those
keys**. If you already had the feature installed (any version tag) it is left
alone and stays yours; a pre-existing bare-string `postCreateCommand` is
converted to a keyed object entry (`main`) so ours can sit beside it —
semantics unchanged, and it's never removed.

## Interactive mode (default)

Running with no flags (and a real terminal) opens a wizard:

- **Already-configured services** get a per-service choice: *Leave as is*,
  *Reconfigure* (reset to a chosen/default variant), or *Remove*.
- **Not-yet-configured services** are offered in a multi-select to add.
- Services with variants (the database) prompt for the variant.

Only the changes you make are applied; anything left "as is" is untouched.

## Non-interactive mode

Pass `--non-interactive` (or run without a TTY, e.g. in CI). One optional-value
flag per service, plus a generic `--service` escape hatch:

| Flag | Effect |
| --- | --- |
| `--minio` | Enable (no-op + warning if already configured) |
| `--minio=remove` | Remove the service |
| `--minio=reconfigure` | Reset to the default variant (blank config) |
| `--database=mysql` | Ensure the `mysql` variant (adds or switches) |
| `--claude` | Claude Code in the container, sharing the host login (`host` variant) |
| `--claude=volume` | Claude Code with an isolated per-project login |
| `--codex` | Codex in the container, sharing the host `~/.codex` login |
| `--idealyst-cli` | The `idealyst` CLI in the container (volume-cached source build) |
| `--service <id>[=…]` | Same, for any registered service without a dedicated flag |

`remove` and `reconfigure` are reserved verbs; any other value is treated as a
variant id and validated against the service. Removing the last managed service
deletes `docker-compose.idealyst.yml` and de-references it. An empty invocation
(`idealyst configure devcontainer --non-interactive`) still ensures a base
devcontainer exists.

## Same engine from the MCP server

The logic lives in the shared `configure` crate (`crates/tools/configure`), so
an agent can do the same thing over MCP via the **`configure_devcontainer`**
tool — pass `dir` + a list of `{ id, variant?, action }` and get back the change
report. The CLI's interactive wizard is the only thing layered on top; both
front-ends drive the identical non-interactive `apply`.

## Extending — adding a service

The service set is a registry, so adding one is a two-step change with no edits
to the CLI or MCP wiring:

1. Implement `DevService` in a new
   `crates/tools/configure/src/devcontainer/services/<id>.rs` — its canonical
   id, label, optional variants, and a `fragment()` returning its
   contributions: an optional compose service mapping (sidecars), env vars +
   mounts for the dev service, named volumes, and any `devcontainer.json`
   `features` / keyed `postCreateCommand` entries (in-container tools like
   `claude`/`codex`).
2. Add it to `registry()` in
   `crates/tools/configure/src/devcontainer/service.rs`.

The wizard checklist, the generic `--service <id>` flag, and the MCP tool all
pick it up automatically. (A dedicated `--<id>` CLI flag is the only optional
extra — add one field in `cmd/configure/devcontainer.rs` if you want the
ergonomic shorthand.)
