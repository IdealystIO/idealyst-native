+++
title = "Migrations & Versioning"
order = 90
tags = ["migration", "versioning", "meta"]
+++

# Migrations & Versioning

This is the index for Idealyst's upgrade guides and the policy behind them.
Every version bump ships **one migration guide** covering the jump from the
previous version — this page lists them and defines the rules authors follow
when writing a new one.

## Versioning policy

Idealyst is pre-1.0. The guiding principle is **one clean experience per
version — no legacy-support hacks**:

- **Breaking changes land in place.** When a design is wrong, we fix it at the
  root and update every call site in the same release. We do **not** keep a
  deprecated path alive alongside the new one, ship compatibility shims, or
  gate old behavior behind a feature flag. Pre-1.0 is exactly the window to get
  the foundation right without carrying weight.
- **Every bump is documented.** A breaking release is accompanied by a
  migration guide between the two consecutive versions (`X` → `Y`). No silent
  breaks.
- **Guides chain.** To jump several versions, read the guides in sequence
  (`0.0.1 → 0.1.0`, then `0.1.0 → 0.2.0`, …). Each guide only describes the
  delta it owns.

Once the framework reaches 1.0, this policy tightens to standard semver
(breaking changes gated to major bumps with a deprecation window). Until then,
the migration guide *is* the compatibility layer.

## Guides

| From → To | Guide |
| --- | --- |
| 0.0.1 → 0.1.0 | [[migration-0-0-1-to-0-1-0]] |

## Updating the dependency

Idealyst crates are pulled by git tag. Bump the tag across your `Cargo.toml`
dependency lines, then follow the guide for that jump:

```toml
# before
idealyst = { git = "https://github.com/.../idealyst-native", tag = "0.0.1" }
# after
idealyst = { git = "https://github.com/.../idealyst-native", tag = "0.1.0" }
```

For an immutable pin, use `rev = "<sha>"` — a tag can be force-moved upstream,
a commit SHA cannot.

## Authoring a new migration guide

When you cut a version, add one file — this is the foundation the whole system
rests on.

- **Filename:** `migration-<from>-to-<to>.md` (dots become dashes in the slug,
  e.g. `migration-0-1-0-to-0-2-0`).
- **Frontmatter:** `title = "Migrating 0.1.0 → 0.2.0"`, an `order` in the
  **900+ band** ascending by target version (so migrations cluster at the end
  of `list_guides` in timeline order), and `tags` including `"migration"` and
  the target version (`"0.2.0"`).
- **Add a row** to the Guides table above and a `[[link]]`.
- **One section per breaking change**, each with the same four beats:
  *What changed · Why · Migrate (before → after) · Status*. The **Status** line
  (`planned` / `landing` / `landed`) keeps the guide honest while the release is
  in flight — a guide for an unreleased version is a *living* document that
  fills in as changes land, never a claim that unbuilt code exists.
- Close with a **migration checklist** the reader can tick through.

### Template

```markdown
+++
title = "Migrating 0.1.0 → 0.2.0"
order = 901
tags = ["migration", "0.2.0", "breaking"]
+++

# Migrating 0.1.0 → 0.2.0

> Status: in development — this guide fills in as 0.2.0 breaking changes land.

## <Breaking change title>

**What changed.** …
**Why.** …
**Migrate.**

```rust
// before (0.1.0)
…
// after (0.2.0)
…
```

Status: planned | landing | landed

## Migration checklist

- [ ] …
```

See [[reactivity]], [[idiomatic-components]], and [[component-hygiene]] for the
concepts the guides reference.
