---
name: release
description: Cut a release of the framework crates to the self-hosted sparse registry at crates.idealyst.io — review what landed, plan the per-crate semver bumps, verify per-package/per-target, publish, verify from outside the workspace, then commit the version bumps. Use when asked to release, deploy, publish, or ship changes to the registry, or when asked what a release would contain.
---

# Cutting a release

Releases are **manual** — there is no CI. `.github/workflows/release.yml` is
`workflow_dispatch`-only and not wired up. `crates/tools/registry` does what
`cargo publish` would: a static sparse registry has no `api` endpoint, which
`cargo publish` requires.

Authoritative long-form reference: [`docs/registry.md`](../../../docs/registry.md).
Read it when something here is not enough — this skill is the procedure, that
file is the design.

## The one number that matters

A new version **discards every consumer's cached build of that crate.** The
whole point of the registry migration was to stop republishing crates that
didn't change: measured over 25 releases, git tags rebuilt 38/38 framework
crates every time; per-crate versions give a median of 2/38. So a plan that
wants to publish more crates than the diff touched is a **bug to investigate**,
not a number to accept.

---

## 0. Preflight

```sh
git status --short        # must be clean — `build` refuses a dirty tree
df -h /                   # target/ runs to ~80 GB; a cold verify needs room
```

The tree must be clean because a release records the commit it was cut from,
and a dirty tree records a commit that doesn't describe the published bytes.
(`--allow-dirty` exists for rehearsals only.)

Disk is a real failure mode here, not a formality — a full disk mid-publish
leaves the harness unable to write its own tool output. Check before, not after.

## 1. See what's unreleased

```sh
git log --oneline -10
```

Everything since the last `chore(release):` commit is in scope. **Read the
footprint of each one** — the user's framing ("a single navigator fix") is
often narrower than what actually landed, and commits can arrive mid-session:

```sh
git show --stat --format='%s%n%n%b' <sha>
```

Report the true set. Do not silently release more than the user named, and do
not silently skip the rest either.

## 2. Review before publishing

**A published version is immutable.** Review is not optional politeness; it is
the last point at which a mistake is cheap. Read the actual diffs, not just the
commit messages.

The two rules that most often go unmet, both from `CLAUDE.md`:

- **§8 — every bug fix lands with a regression test.** Check the test *bites*:
  break the fix and confirm the test fails. A test that passes against the buggy
  code is not a regression test. Watch for tautological tests (asserting on data
  the test itself just wrote, never calling production code) and vacuous
  assertions (`assert!(html.contains("<"))` where the wrapper always supplies
  one).
  - For backend code that can't be unit-tested, this repo's established pattern
    is a pure `*_policy.rs` module beside the platform code, un-gated so it runs
    from any host — see `crates/backend/ios/mobile/src/portal_policy.rs`.
- **§2 — docs move with behavior.** A new public API or macro block form needs
  its doc section in the same change. Grep for doc comments that describe the
  *old* behavior; a stale doc that argues against the fix is worse than no doc.

Fix what you find in a separate commit **before** publishing, so the release
records source that is actually correct.

## 3. Plan (read-only, touches nothing remote)

```sh
AWS_PROFILE=idealyst \
IDEALYST_REGISTRY_BUCKET=idealyst-crates \
IDEALYST_REGISTRY_DISTRIBUTION=EWTO387ZA9GEV \
cargo run -q -p registry -- plan
```

Then **sanity-check the mapping yourself**: for each crate in the plan, does a
commit actually touch its directory, and does the bump level match the commit
subject?

- `feat:` → minor, `fix:`/`chore:`/anything unrecognised → patch, `!` or
  `BREAKING CHANGE:` → major. Strongest signal across the crate's commits wins.
- A crate in the plan whose directory the diff never touched means the change
  detection is wrong. That has happened: a release's own version-bump commit
  used to re-trigger every crate it had bumped. Chase it, don't publish it.
- The bump is classified from the commit **subject**, per crate. A `feat:` that
  lands an API in one crate and only a test in another bumps both alike. That's
  accepted (publishing is the safer half of the trade), but say so in the
  release message.
- Only a **major** bump republishes dependents. Internal requirements are
  `^1.5` in `[workspace.dependencies]`, which admits 1.6.0 and 1.7.0 — minors
  and patches need no rewriting. Confirm with `grep -n '^<crate>' Cargo.toml`.

## 4. Verify — per package, per target

**`cargo check --workspace` is never green in this repo.** Verify the crates the
plan names, on the targets they actually ship to:

```sh
cargo test -q -p runtime-shared
cargo test -q -p runtime-vocabulary
cargo test -q -p idea-ui --features catalog          # see below
cargo check -q -p backend-web --target wasm32-unknown-unknown
cargo check -q -p backend-ios-mobile --target aarch64-apple-ios
```

Known non-signals — do not chase these, and do not report them as caused by the
release:

| Symptom | Cause |
| --- | --- |
| `cannot find __mcp in runtime_core`, missing `idea_ui::recipes::*` | `examples/design_sync.rs` needs `--features catalog`. Bare `cargo test -p idea-ui` cannot build it. |
| 4 × `newcore::tests` fail with `class NSScreen could not be found` (`perf_trace.rs`) | Host test binary doesn't link AppKit. Pre-existing. |

If something else fails, establish whether it is pre-existing **without
`git stash`** (`CLAUDE.md` §0) — read the diff of the relevant files, or check
whether any unreleased commit touches them:

```sh
git log --oneline <last-release-sha>..HEAD -- <path>
```

## 5. Publish

```sh
AWS_PROFILE=idealyst \
IDEALYST_REGISTRY_BUCKET=idealyst-crates \
IDEALYST_REGISTRY_DISTRIBUTION=EWTO387ZA9GEV \
cargo run -q -p registry -- publish --execute
```

Without `--execute` it stages and stops, having touched nothing remote.

Publishing is **incremental** — each crate's tarball and index entry upload
before the next is packaged, because `cargo package` resolves deps as if they
were already published. That is why a partial failure is awkward: see
"If a publish fails partway" below.

## 6. Verify from outside the workspace

Publishing "succeeded" is the tool's opinion. Check the registry:

```sh
curl -s https://crates.idealyst.io/releases.json | python3 -c "
import json,sys
d=json.load(sys.stdin)['crates']
for k in ['<crate>', '<crate>']:
    print(f\"  {k:22} {d[k]['version']:8} @ {d[k]['commit'][:8]}\")
print(f'  total: {len(d)}')"
```

Every released crate should show the new version at HEAD's sha; every other
crate should still show its old version and old sha. Then the tarballs — note
the URL shape comes from `config.json`'s `dl` template, **not** the bucket
layout:

```sh
curl -sL -o /dev/null -w '%{http_code} %{size_download}\n' \
  https://crates.idealyst.io/crates/<crate>/<version>/download
```

Then the real check — a clean consumer with no lockfile, outside the workspace:

```sh
D=<scratchpad>/relcheck; rm -rf $D; mkdir -p $D/src $D/.cargo
printf '[registries.idealyst]\nindex = "sparse+https://crates.idealyst.io/index/"\n' > $D/.cargo/config.toml
cat > $D/Cargo.toml <<'EOF'
[package]
name = "relcheck"
version = "0.0.0"
edition = "2021"
[dependencies]
<top-level crate> = { version = "<new>", registry = "idealyst" }
EOF
echo 'fn main(){}' > $D/src/main.rs
cargo fetch --manifest-path $D/Cargo.toml
grep -A1 'name = "<crate>"' $D/Cargo.lock
```

Depend on the highest-level crate released (usually `idea-ui`) and confirm it
pulls the others through their **unchanged** caret requirements — that is the
proof no dependent needed rewriting.

## 7. Commit the version bumps

`publish` rewrites the released crates' `Cargo.toml` versions plus `Cargo.lock`.
Commit them as `chore(release):` and say what shipped and how it was checked:

- Which crates, from which version to which, and **why that bump level** —
  name the commit that earned a minor or major.
- Which crates were deliberately NOT republished, and that consumers keep their
  cached builds.
- Anything the plan did that looks surprising (a no-op bump, a crate at an old
  version finally moving).
- The verification: which targets were checked, and what the external consumer
  resolved.

Do **not** add `Co-Authored-By: Claude` or any AI attribution trailer
(`CLAUDE.md` §0a).

---

## Traps

- **The recorded commit predates the bump commit.** `build` records HEAD from a
  clean tree, *then* writes the versions; the bumps are committed afterwards.
  The planner compensates by ignoring a commit whose entire footprint in a crate
  is that crate's own `version` key. If you ever see the previous release's crate
  set reappear in a plan, that guard has regressed — see
  `version::is_own_version_bump` and its regression test.
- **Resume is scoped to ONE commit.** Cargo embeds `.cargo_vcs_info.json` (the
  sha) in every `.crate`, so an interrupted publish can only be resumed from the
  same commit. Committing a fix and retrying invalidates every already-uploaded
  tarball.
- **A published version is immutable.** Re-publishing the same version is a
  no-op when the bytes match and a hard error when they don't.
- **Internal deps must name `registry = "idealyst"`.** Without it cargo resolves
  bare names like `css`, `net`, `table`, `wire` against crates.io, where they
  belong to unrelated packages. It fails loudly on a version mismatch and
  succeeds *silently* otherwise.
- **`denoise` cannot be published** (git dep on `deep_filter`); it is
  `publish = false` by design, not an oversight.
- **Tooling crates publish nothing.** The CLI, `dev-reload`, `build-web`, the
  MCP server and the 31 runnable examples are all `publish = false`. A commit
  touching only those releases nothing — say so rather than looking for a crate
  to bump.

## If a publish fails partway

Some crates are live and some aren't. Do **not** commit a fix and re-run — that
changes every remaining tarball's embedded sha. Re-run `publish --execute` from
the **same commit**; already-uploaded crates whose bytes match are no-ops. If
the source genuinely has to change, the half-published versions were never
consumed by anyone, so clear them from the bucket and re-cut. `--only <crate>`
re-cuts a single crate without pulling its dependencies in.
