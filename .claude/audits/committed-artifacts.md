---
name: committed-artifacts
description: Committed files must not include build artifacts, environment files, secrets, or local IDE/OS detritus.
targets:
  - .
severity: high
---

# Committed artifacts

## Background

Repos accumulate cruft over time — a stray `.env`, a checked-in
`target/` directory, an `.DS_Store`, a developer's `keystore.jks` — that
either bloats the tree, leaks secrets, or breaks reproducibility for
other contributors. This audit sweeps the actual git index (not the
working tree, not `.gitignore`) so we catch files that *are already
tracked* even if `.gitignore` has since been updated.

The check operates on `git ls-files` output at HEAD. A `.gitignore`
entry does **not** retroactively untrack a previously committed file —
the file must be explicitly `git rm --cached`'d. That's why the index
itself is the source of truth.

## Checklist

Run from repo root. Use `git ls-files` (NOT `find`, NOT `ls`) so only
tracked files are inspected. For each pattern below, flag every match.

### Build artifacts

- [ ] **Rust target dirs** — any path containing `/target/` or
      starting with `target/`. Includes nested `target/` in subcrates.
- [ ] **Node build output** — `node_modules/`, `dist/`, `build/` (when
      it's a JS/TS build output, not a Rust crate named `build`),
      `.next/`, `.nuxt/`, `out/`, `coverage/`.
- [ ] **wasm-pack output** — `pkg/` directories that contain
      `package.json` + `*.wasm` (the generated bundle, not a source
      crate named `pkg`).
- [ ] **Compiled binaries** — `*.o`, `*.a`, `*.so`, `*.dylib`, `*.dll`,
      `*.exe`, `*.wasm` (unless it's a fixture asset under
      `tests/`/`fixtures/`), `*.rlib`, `*.rmeta`.
- [ ] **Apple build outputs** — `*.dSYM/`, `*.xcarchive/`, `*.ipa`,
      `*.app/` (unless it's a documented sample bundle),
      `DerivedData/`, `Pods/` (CocoaPods install output — `Podfile`
      and `Podfile.lock` are fine).
- [ ] **Android build outputs** — `*.apk`, `*.aab`, `*.aar` (unless
      vendored as a dependency in a known `libs/` directory), `*.dex`,
      `app/build/`, `.gradle/`, `local.properties`.

### Environment / secrets

- [ ] **Dotenv files** — `.env`, `.env.*` (except documented
      `.env.example` / `.env.template`). Treat any `.env` variant
      without `example`/`template`/`sample` in the name as **high
      severity**.
- [ ] **Private keys** — `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`,
      `*.p12`, `*.pfx`, `*.jks`, `*.keystore`. Flag every one; some
      may be intentional test fixtures, but each needs a justification.
- [ ] **Apple provisioning** — `*.mobileprovision`,
      `*.provisionprofile`, `*.cer` (unless it's a public root
      certificate fixture).
- [ ] **Service account JSON** — files matching
      `google-services.json`, `GoogleService-Info.plist`,
      `service-account*.json`, `firebase-adminsdk*.json`. These
      typically embed project secrets.
- [ ] **AWS/GCP/Azure config** — `.aws/credentials`,
      `gcloud/*credentials*`, `azure*credentials*`.
- [ ] **Token-shaped strings in committed files** — grep tracked text
      files for high-entropy patterns commonly used by secret scanners:
      `AKIA[0-9A-Z]{16}` (AWS access key), `ghp_[A-Za-z0-9]{30,}`
      (GitHub PAT), `sk-[A-Za-z0-9]{20,}` (OpenAI/Anthropic-style),
      `xox[bp]-` (Slack token), `-----BEGIN .*PRIVATE KEY-----`. Each
      hit is **high severity** unless inside a doc page explicitly
      teaching the format (then medium).

### Local / IDE / OS detritus

- [ ] **macOS** — `.DS_Store`, `__MACOSX/`, `._*`.
- [ ] **Windows** — `Thumbs.db`, `Desktop.ini`.
- [ ] **JetBrains** — `.idea/` (some teams commit a curated subset;
      flag the whole directory and let reviewers triage).
- [ ] **VS Code** — `.vscode/` with `settings.json` containing
      machine-local paths; `.vscode/launch.json` and
      `.vscode/extensions.json` are usually fine.
- [ ] **Xcode user state** — `xcuserdata/`, `*.xcuserstate`,
      `*.xcuserdatad/`.
- [ ] **Backup / swap files** — `*~`, `*.swp`, `*.swo`, `*.orig`,
      `*.rej`, `*.bak`.
- [ ] **Editor history** — `.history/`, `.local-history/`,
      `*.code-workspace` (usually personal).

### Logs / caches / runtime state

- [ ] **Log files** — `*.log`, `npm-debug.log*`, `yarn-debug.log*`,
      `yarn-error.log*`.
- [ ] **Caches** — `.cache/`, `.parcel-cache/`, `.turbo/`,
      `.eslintcache`, `.stylelintcache`.
- [ ] **Coverage** — `coverage/`, `.nyc_output/`, `lcov.info`,
      `*.profraw`, `*.profdata`.
- [ ] **Local databases** — `*.sqlite`, `*.sqlite3`, `*.db` (unless
      it's a documented test fixture).

### Size sanity

- [ ] **Oversized files** — any tracked file > 1 MB. List size with
      finding. Large binaries should be justified (asset, fixture)
      or moved to LFS / external storage.
- [ ] **Total `target/` weight** — if any `target/` paths are
      tracked, separately report the cumulative size of all `target/`
      entries.

## How to run the checks

Useful commands (read-only):

```bash
# All tracked files
git ls-files

# Tracked files matching a pattern
git ls-files | grep -E '(^|/)target/'
git ls-files | grep -E '\.env($|\.)' | grep -vE '\.(example|template|sample)$'
git ls-files | grep -E '\.(p12|pfx|jks|keystore|mobileprovision|pem|key)$'

# File sizes for tracked files
git ls-files -z | xargs -0 -I{} sh -c 'printf "%s\t%s\n" "$(wc -c < "{}" 2>/dev/null || echo 0)" "{}"' | sort -rn | head -50

# Secret-shaped strings in tracked text files
git grep -nE 'AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{30,}|sk-[A-Za-z0-9]{20,}|xox[bp]-|-----BEGIN .*PRIVATE KEY-----'
```

Do not run `git rm` or modify the index. This audit only reports.

## Output format

Report findings as a Markdown list. Group by category (Build /
Secrets / IDE-OS / Logs-caches / Size). For each finding include:

- **Severity**: low / medium / high
- **Location**: tracked path(s). For large groups (e.g. dozens of
  `target/` files), list a count + a few representative paths rather
  than every entry.
- **Issue**: one-line description (what kind of file, why it shouldn't
  be committed).
- **Why**: brief reasoning — leaks secret? bloats clone? non-portable?
- **Suggested fix**: usually `git rm --cached <path>` + add to
  `.gitignore`. For secrets, also note rotation is required.

Severity guidance:
- **High**: any real secret material (private keys, service-account
  JSON, populated `.env`), any tracked `target/`, any token-shaped
  string match.
- **Medium**: build outputs that aren't secrets (compiled binaries,
  `dist/`, `node_modules/`), IDE state likely to cause merge churn.
- **Low**: single `.DS_Store`, an `.orig` or `*~` file, an
  uncontroversial `.vscode/` entry.

End with a one-line summary: `Result: N high, M medium, K low findings.`
