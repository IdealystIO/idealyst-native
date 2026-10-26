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
It swings: one day this week went 86 → 15 → 51 GiB free between releases, so a
number from an hour ago is not a number.

Also check `git rev-list --left-right --count origin/master...HEAD` after a
`git fetch`. A commit that is local-only still releases fine — the tarball
embeds its sha, not its presence on a remote — but push it WITH the release so
`releases.json` never points at a commit nobody else can see.

**If the tree is dirty with files you did not touch, stop and ask.** Several
sessions commit into this working tree at once. Do not `git add -A` someone's
in-progress work to get a clean tree, and do not release around it with
`--allow-dirty`. Say what the files are and let the user decide.

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
not silently skip the rest either. "One fix" has meant three commits every
time this week.

**Check each commit's footprint against its subject.** A commit made with
`git add -A` in a shared tree can carry another session's in-progress files
under a subject that never mentions them — this week a navigator fix rode into
two unrelated commits that way. If a `fix(layout)` touches `navigator/shared.rs`,
find out why. Those commits are usually already pushed, so do not rewrite them;
describe what the bumps actually carry in the release commit instead.

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
  - **How to check the bite without `git stash`.** If the test and the fix
    live in different files, `git checkout <fix-sha>^ -- <impl file>` puts the
    old implementation under the new test; run; `git checkout HEAD -- <file>`.
    If they share a file, copy it to the scratchpad, mutate the fixed arm back
    to the bug by hand, run, copy it back. Either way confirm `git status` is
    clean afterwards. Make the mutation the ACTUAL bug — half a mutation
    (one side of a coordinate comparison, say) can leave the test passing and
    tell you nothing.
  - A fix in platform-only code (`imp/` under `target_os`) with no host test
    is the documented §8 escape hatch when the commit says why no tighter test
    exists. A host test asserting "this method is empty" is theatre — do not
    add one to tick the box. Check for a CHANGELOG entry instead; that is the
    §2 gap those commits actually tend to have.
- **§2 — docs move with behavior.** A new public API or macro block form needs
  its doc section in the same change. Grep for doc comments that describe the
  *old* behavior; a stale doc that argues against the fix is worse than no doc.

Fix what you find in a separate commit **before** publishing, so the release
records source that is actually correct. Stage it by path (`git add <file>…`),
never `-A` — see §0.

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
  The one legitimate exception is a crate you named with `--force` yourself;
  the plan labels those `(--force)`.
- **`--bump <CRATE>=<LEVEL>`** raises a bump level the subjects under-called.
  The classifier reads the commit SUBJECT, so a branch that lands new public
  API under a free-form subject ("scroll_view can say when the reader has
  reached the end") cuts as a patch. **The standing rule (set 2026-09-10 on
  a0265630): additive public API is a minor** — new items, new methods. A new
  field on an all-pub struct is NOT additive; it is a major (next bullet).
  Check every planned crate:
  `git diff <last-release>..HEAD -- <crate> | grep -E '^\+\s*pub (fn|struct|enum|type|[a-z_]+:)'`
  A new `pub` item → `--bump <crate>=minor`. A private rename (`enum PanState`
  fields, say) is neither. Raise-only, and refuses a crate that is not
  already in the plan. Recomputes from the PUBLISHED version, so raising a
  patch to a minor cannot skip a number. Breaking changes need no grep: the
  API check below raises those to a major on its own.
- **The API check raises breaking changes to a major automatically.** For
  every crate that earned its own release, `plan` parses the library source
  (`syn`) at the crate's recorded release commit and at HEAD and diffs the
  exported items (`crates/tools/registry/src/api.rs`). A crate with a finding
  is printed `major … (API break)` with one indented line per finding. What
  counts:
  - **a field added to an exported `pub struct` whose fields are all `pub`
    and which is not `#[non_exhaustive]`** — a struct literal in another
    crate must name every field. Rule set 2026-10-20 after `table` 3.1.0
    added `pub column` to `TableCellProps` as a minor and broke a downstream
    literal. `#[non_exhaustive]` is no way out for props (it forbids
    literals even with `..Default::default()`, and `ui!` builds props by
    literal), so a new prop on an explicit `FooProps` struct, or a new
    parameter on an inline-props `#[component] fn Foo(..)` (its generated
    `FooProps`), is a major;
  - a variant added to an exported enum without `#[non_exhaustive]` (see the
    next bullet), or `#[non_exhaustive]` added to an enum or all-pub struct;
  - anything exported removed or made non-`pub`: items, `pub` fields,
    variants, inherent `pub` methods, trait items, `#[macro_export]` macros,
    `pub use` of another crate's item; a `pub` field or variant retyped; a
    required (no default) trait item added.

  "Exported" follows `pub mod` chains and `pub use` re-exports (named and
  glob); `pub(crate)` items and `pub` items in private modules that nothing
  re-exports are skipped. Macro-generated items other than `#[component]`
  props are invisible, and fn signature changes are not detected — read the
  diff for those. **Read every finding.** A real one is a major, and its
  library dependents come along (see "Only a major bump republishes
  dependents" below). For a false positive — a `pub` item the crate
  documents as internal, a sealed trait gaining a method — pass
  `--skip-api-check <CRATE>` on `plan` / `build` / `publish`: the crate keeps
  its subject level, the findings still print, labelled
  `(API check waived)`, and the release commit should say why.
- **A new variant on a `pub enum` without `#[non_exhaustive]` is a major**, even
  if it looks additive. Every published version of every crate that `match`es
  on the enum exhaustively stops compiling the moment `cargo update` pulls the
  new version under its caret requirement. Checking the current tree is not
  enough: if a crate in this same release has to handle the new variant, its
  *previous published version* is the one that breaks.
  `runtime-shared 2.4.0` shipped `TrackSize::FitContent` as a minor because "no
  consumer matches on TrackSize". But `runtime-layout` and `css` did, in the
  same commit, and `runtime-layout 2.0.0` / `css 2.0.0` (`runtime-shared ^2.0.0`)
  then failed with E0004 in CrewForge's lockfile. Find the matches with
  `grep -rn 'Enum::' --include=*.rs crates` over every crate that depends on
  the changed one. The API check catches the variant itself; this grep is
  how you explain the major in the release commit. Adding
  `#[non_exhaustive]` later is also a major, because dependents' exhaustive
  matches stop compiling without a `_` arm.
- **A planned crate whose diff is behaviour-free gets excluded.** A comment-
  only or doc-only change to a crate's source still changes its published
  bytes, so the planner wants a patch — and every consumer would rebuild that
  crate for nothing. Check with
  `git show <sha> -- <crate> | grep -E '^\+' | grep -vE '^\+\+|^\+\s*//|^\+\s*$' | wc -l`;
  zero means comment-only. Leave it out with `--only <crate>` for each crate
  you DO want (repeatable), on `publish`. Its recorded commit stays where it
  was and the comment rides into its next real release — `backend-web` was
  excluded this way four releases running. Say so in the release commit.
  `--only` is a `build`/`publish` flag; `plan` does not take it, so plan
  without and subtract in your head.
- **`--skip <CRATE>`** (repeatable, on `plan`/`build`/`publish`) leaves a crate
  out without reading its history. It is the answer when `plan` dies with "is
  <sha> still in this history?": that crate was released from a clone that
  never pushed, so its change detection cannot run here, and a release of it
  from here would lack the unpushed work. Find every such crate at once:
  check each `releases.json` commit with `git cat-file -e <sha>^{commit}`.
  If the plan must republish a skipped crate (a dependency of it goes major),
  it fails rather than leave it on the old major. Name the skip in the
  release commit; it ends once the other clone pushes.
- **`--force <CRATE>`** releases a crate at a patch bump although its directory
  is unchanged. Reach for it only when the *published manifest* is wrong and
  the source is right — an under-declared internal requirement is the case it
  exists for. It works on `plan`, `build` and `publish`, and never downgrades a
  crate that earned a bigger bump on its own.
- **Repairing a release cut at the wrong level** (idea-ui-nav 1.5.9 should
  have been 2.0.0): `--force X --bump X=major --only X` re-cuts it, then
  `cargo run -q -p registry -- yank X <bad version> --execute` (same env vars)
  stops new resolutions picking the bad one. Lockfiles naming it keep working.
- The bump is classified from the commit **subject**, per crate. A `feat:` that
  lands an API in one crate and only a test in another bumps both alike. That's
  accepted (publishing is the safer half of the trade), but say so in the
  release message.
- Only a **major** bump republishes dependents, and a dependent with a
  library target goes **major** with it (transitively; a library that earned
  a smaller bump of its own is raised). Its new release requires the new
  major, whose types are in its API, so a patch would let `cargo update` on
  the old line pull in a second copy of the dependency. That is the
  `idea-ui-nav 1.5.9` slip: idea-ui 5.0.0 dragged it in at a patch, and an
  app on idea-ui 4 got idea-ui 5 alongside. Bin-only dependents (the CLI,
  dev-tui) still go at a patch. See `drag_in_dependents_of_majors`.
- Which crates get published is separate from the requirement they carry: `apply_plan` moves the released
  crate's floor in `[workspace.dependencies]` for **every** bump, patch
  included, before anything is packaged — so a dependent in the same release
  records the sibling it was actually compiled against. Confirm with
  `grep -n '^<crate> ' Cargo.toml` after the plan applies; the floor should
  read the new full version.
  - This is the `gesture 1.5.3` bug: the floor used to move only on a major,
    so a `fix:` that added `Recognizer::drive` in runtime-shared **1.7.1**
    shipped alongside a `gesture` that called it while declaring
    `runtime-shared = "1.5"`. Consumers with 1.7.0 in their lock kept it and
    the build broke. If you ever see a floor below the version of a crate
    released in the same run, stop — that is the bug back.

## 4. Verify — per package, per target

**`cargo check --workspace` is never green in this repo.** Verify the crates the
plan names, on the targets they actually ship to:

```sh
cargo test -q -p runtime-shared
cargo test -q -p runtime-vocabulary
cargo test -q -p idea-ui
cargo build -q -p idea-ui --example design_sync --features style-dump,catalog
cargo check -q -p backend-web --target wasm32-unknown-unknown
# The runtime-server transport is feature-gated and nothing in the workspace
# builds it, yet a registry user's plain `idealyst dev --web` does: a
# `wire::Command` variant without an arm in `dev_transport::command_kind`
# shipped backend-web 4.1.0 uncompilable there.
cargo check -q -p backend-web --target wasm32-unknown-unknown --features runtime-server
cargo check -q -p backend-ios-mobile --target aarch64-apple-ios
```

There are no known non-signals: every command above is expected to pass. If
one fails, establish whether it is pre-existing **without
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
proof no dependent needed rewriting. When nothing high-level was released,
depend on an UNCHANGED `idea-ui` at its current version and confirm it now
resolves the new sibling — same proof, from the other side.

Then confirm the fix is in the bytes, not only in the plan:

```sh
T=<scratchpad>/tar; rm -rf $T; mkdir -p $T
curl -sL https://crates.idealyst.io/crates/<crate>/<version>/download | tar xz -C $T
grep -c '<something only the fix contains>' $T/<crate>-<version>/src/<file>
```

`releases.json` says which commit was cut; this says what shipped.

## 7. Commit the version bumps

`publish` rewrites the released crates' `Cargo.toml` versions plus `Cargo.lock`.
Commit them as `chore(release):` — `git add -A` is acceptable HERE ONLY after
you have confirmed `git status` shows nothing but those manifests — and say
what shipped and how it was checked:

- Which crates, from which version to which, and **why that bump level** —
  name the commit that earned a minor or major.
- Which crates were deliberately NOT republished, and that consumers keep their
  cached builds.
- Anything the plan did that looks surprising (a no-op bump, a crate at an old
  version finally moving), any crate you excluded with `--only` and why, and
  any `--bump` and why.
- What the bumps actually carry when a commit subject does not say (§1).
- The verification: which targets were checked, and what the external consumer
  resolved.

`CHANGELOG.md` has no "Unreleased" section to cut: crates version
independently, so each entry goes into the matching group (behaviour
changes / added / changed / fixed) above `## 1.0.1` and names the crates it
touches. Add entries for what this release carries if the commits didn't.

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
- **Versioned dev-deps order the publish.** `cargo package` resolves an
  internal dev-dependency that carries a version (e.g. `x = { workspace = true
  }`) from the registry, so the tool packages its target first when both are in
  the release — the 2026-09-24 run died packaging `wire` before the new
  `runtime-macros` existed, back when only `[dependencies]` ordered it. No
  `--only` pre-pass is needed any more. If `build`/`publish` refuses with
  "dependency cycle … versioned dev-dependencies in the cycle: A -> B", make
  that dev-dep path-only (no `version`, no `workspace = true`) — no order can
  package either side first.
- **Internal deps must name `registry = "idealyst"`.** Without it cargo resolves
  bare names like `css`, `net`, `table`, `wire` against crates.io, where they
  belong to unrelated packages. It fails loudly on a version mismatch and
  succeeds *silently* otherwise.
- **`denoise` cannot be published** (git dep on `deep_filter`); it is
  `publish = false` by design, not an oversight.
- **The CLI is a published crate.** `idealyst-cli` and the 35 tooling crates it
  builds on (`build-*`, `run-*`, `dev-reload`, `dev-http`, `wasm-carve`,
  `mcp-server`, `lint`, …) release like any other crate, and users get them
  through `idealyst update` / `cargo install idealyst-cli --index …`. A CLI
  change is not deployed until it is released. The runnable examples and the
  release tooling are still `publish = false`.
  - The CLI embeds `examples/welcome` and the Inspector bundle, both outside
    its directory. `[package.metadata.registry] also-watch` makes an edit there
    plan a CLI release, and `prepackage` stages them into
    `crates/tools/cli/package-assets/` before `cargo package` — that step
    builds the Inspector for the web, so it needs the wasm toolchain and adds
    about a minute. If it fails, the release stops; do not work around it.
  - Verify a CLI release from outside the workspace with
    `cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked --root <scratch>`,
    then `<scratch>/bin/idealyst new <dir>` (scaffold present) and
    `<scratch>/bin/idealyst inspect` (front end present, not "will serve no page").
  - If a guide in `crates/mcp/catalog/guides` described the old behaviour,
    fix the guide — that re-cuts `mcp-catalog`.
- **`build` writes the version bumps.** A local `build` rehearsal leaves the
  manifests modified, and the real `publish` then refuses the dirty tree. Do
  not rehearse before a real publish; if you must, `git checkout -- Cargo.toml
  Cargo.lock crates/**/Cargo.toml` between the two.

## If a publish fails partway

Some crates are live and some aren't. Do **not** commit a fix and re-run — that
changes every remaining tarball's embedded sha. Re-run `publish --execute` from
the **same commit**; already-uploaded crates whose bytes match are no-ops. If
the source genuinely has to change, the half-published versions were never
consumed by anyone, so clear them from the bucket and re-cut. `--only <crate>`
re-cuts a single crate without pulling its dependencies in.
