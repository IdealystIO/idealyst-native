# Dev containers — `idealyst configure devcontainer`

`idealyst configure` is the home for one-off "set up this aspect of the
project" actions. Its first subcommand, `configure devcontainer`, initializes
(or updates) a [Dev Container](https://containers.dev/) for a project and lets
you toggle the optional sidecar services a real app needs — a database, Redis,
an S3-compatible object store — that run alongside the main dev container.

```
idealyst configure devcontainer            # interactive wizard
idealyst configure devcontainer --non-interactive --database=postgres --redis
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
- **`devcontainer.json`** is only touched to make sure its `dockerComposeFile`
  array lists `docker-compose.idealyst.yml`. That edit happens at most once.
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
   id, label, optional variants, and a `fragment()` returning the compose
   service mapping + the env vars to inject + any named volumes.
2. Add it to `registry()` in
   `crates/tools/configure/src/devcontainer/service.rs`.

The wizard checklist, the generic `--service <id>` flag, and the MCP tool all
pick it up automatically. (A dedicated `--<id>` CLI flag is the only optional
extra — add one field in `cmd/configure/devcontainer.rs` if you want the
ergonomic shorthand.)
