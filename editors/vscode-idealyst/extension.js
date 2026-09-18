// Idealyst VS Code extension — DSL-vocabulary completion for ui!/jsx!
// and theme-token completion for stylesheet!.
//
// rust-analyzer owns types and expressions (including inside the macros,
// via the macro's IDE-recovery expansion). What RA can NOT know is the
// DSL vocabulary: which tags exist, which props a tag takes, what they
// mean. That data lives in the idealyst catalog (`inventory`-registered
// components/primitives with docs), fetched here by shelling out to
// `idealyst catalog-json` once per workspace and cached in memory.
//
// Theme tokens are the same story one level down. Inside `stylesheet!`,
// `t.spacing.md()` is a real typed accessor, so RA *can* complete it —
// but only when the macro body still parses, which mid-typing (`t.`) it
// does not. We own that: the catalog's `style_tokens` slice carries every
// token's accessor path, registry name, type, and base value, so the
// completion works on a half-written body and shows the token name and
// default the accessor resolves to.
//
// Deliberately dependency-free plain JS: installable by symlinking this
// folder into ~/.vscode/extensions — no build step, no vsce.

const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const path = require("path");

// ---------------------------------------------------------------------------
// Project resolution + catalog loading
// ---------------------------------------------------------------------------

/**
 * Catalogs keyed by the directory `idealyst catalog-json` was pointed
 * at — a project crate, or a workspace root (see `projectFor`).
 * Value: `{ tags, propsByTag, tokensByPrefix }`.
 */
const catalogs = new Map();
/** directories with a load in flight, so we don't spawn twice */
const loading = new Set();
/** "Idealyst" output channel — where the CLI's stderr and our own logs go. */
let output = null;

function log(line) {
    if (output) output.appendLine(`[${new Date().toISOString().slice(11, 19)}] ${line}`);
}

function cliPath() {
    return vscode.workspace.getConfiguration("idealyst").get("cli") || "idealyst";
}

/** Read a Cargo.toml; `null` when there is none. */
function manifestAt(dir) {
    try {
        return fs.readFileSync(path.join(dir, "Cargo.toml"), "utf8");
    } catch {
        return null;
    }
}

/**
 * The framework's own marker for "this crate is an idealyst project" —
 * the same key `idealyst build`/`mcp` read.
 */
function isIdealystManifest(manifest) {
    return manifest.includes("[package.metadata.idealyst");
}

/**
 * Decide which directory the catalog for `filePath` comes from.
 *
 * Walk up from the file to the workspace folder looking for the nearest
 * crate whose Cargo.toml is an idealyst project; that crate's catalog
 * (its own components + every component library it depends on) is
 * exactly the vocabulary usable from that file. This is what makes a
 * monorepo work: in `crates/app-main/src/*.rs` the answer is
 * `crates/app-main`, not the workspace root.
 *
 * A file that belongs to no idealyst crate — a shared component library
 * inside the workspace — falls back to the workspace root, which the
 * CLI expands into every idealyst member (`resolve_project_roots`) so
 * the library's components still show, merged. That fallback can be a
 * big build in a large workspace, so it's only ever taken lazily (on a
 * completion attempt or an explicit refresh — never on activation).
 *
 * Returns `{ dir, exact }` or `null` when nothing here is idealyst.
 * The old gate checked only the workspace root's manifest and so was
 * silently inert in every workspace-shaped project.
 */
function projectFor(filePath, folder) {
    let dir = path.dirname(filePath);
    const stop = path.resolve(folder);
    for (;;) {
        const manifest = manifestAt(dir);
        if (manifest && isIdealystManifest(manifest)) return { dir, exact: true };
        if (path.resolve(dir) === stop) break;
        const parent = path.dirname(dir);
        if (parent === dir) break;
        dir = parent;
    }
    const rootManifest = manifestAt(stop);
    if (rootManifest && rootManifest.includes("[workspace]")) {
        return { dir: stop, exact: false };
    }
    return null;
}

/**
 * Run `idealyst catalog-json DIR`, digest, cache. stderr (cargo build
 * chatter, wrapper errors) streams to the output channel so a failed
 * or slow first load is diagnosable instead of a vanishing status-bar
 * message.
 */
function loadCatalog(dir, { force = false } = {}) {
    if (!force && (catalogs.has(dir) || loading.has(dir))) return;
    loading.add(dir);

    const cli = cliPath();
    log(`loading catalog: ${cli} catalog-json ${dir}`);
    const status = vscode.window.setStatusBarMessage(
        "$(sync~spin) idealyst: building catalog… (first run compiles the project)"
    );
    const started = Date.now();
    const child = cp.spawn(cli, ["catalog-json", dir], { cwd: dir });
    const stdout = [];
    child.stdout.on("data", (b) => stdout.push(b));
    child.stderr.on("data", (b) => {
        for (const line of b.toString().split("\n")) if (line.trim()) log(`  ${line}`);
    });
    const finish = (err) => {
        status.dispose();
        loading.delete(dir);
        const secs = ((Date.now() - started) / 1000).toFixed(1);
        if (err) {
            log(`catalog load FAILED for ${dir} after ${secs}s: ${err}`);
            vscode.window.setStatusBarMessage(
                "idealyst: catalog load failed — see Output ▸ Idealyst",
                8000
            );
            return;
        }
        try {
            const json = JSON.parse(Buffer.concat(stdout).toString());
            const cat = digest(json);
            catalogs.set(dir, cat);
            log(
                `catalog ready for ${dir} in ${secs}s: ` +
                `${(json.components || []).length} components, ` +
                `${(json.primitives || []).length} primitives, ` +
                `${(json.style_tokens || []).length} tokens`
            );
            vscode.window.setStatusBarMessage("idealyst: catalog ready", 4000);
        } catch (e) {
            log(`catalog parse FAILED for ${dir}: ${e.message}`);
        }
    };
    child.on("error", (e) => finish(`cannot run ${cli}: ${e.message}`));
    child.on("close", (code) => finish(code === 0 ? null : `exit code ${code}`));
}

/**
 * Digest the raw catalog JSON into completion-shaped data.
 *
 * Tags: primitives (snake_case, lowercase-only in ui!) + components
 * (PascalCase). Props per tag:
 * - primitives carry `props` directly;
 * - explicit-props components have one `props` param whose `schema`
 *   holds the fields (name/type/doc);
 * - inline-props components' `params` ARE the props.
 */
function digest(json) {
    const tags = [];
    const propsByTag = new Map();

    for (const p of json.primitives || []) {
        tags.push({
            name: p.name,
            kind: vscode.CompletionItemKind.Function,
            detail: `primitive · ${p.category || ""}`,
            docs: p.docs || "",
        });
        propsByTag.set(
            p.name,
            (p.props || []).map((f) => ({
                name: f.name,
                type: f.type || "",
                docs: f.doc || "",
            }))
        );
    }

    for (const c of json.components || []) {
        // Tags are PascalCase at the call site; the catalog stores the fn
        // name, which is PascalCase by convention (strict-naming).
        tags.push({
            name: c.name,
            kind: vscode.CompletionItemKind.Class,
            detail: `component · ${c.module_path || ""}`,
            docs: c.docs || "",
        });
        const params = c.params || [];
        let props = [];
        if (params.length === 1 && Array.isArray(params[0].schema)) {
            props = params[0].schema.map((f) => ({
                name: f.name,
                type: f.type || "",
                docs: [f.doc, f.constraint && `constraint: ${f.constraint}`]
                    .filter(Boolean)
                    .join("\n\n"),
            }));
        } else if (!(params.length === 1 && params[0].name === "props")) {
            // Inline-props component: the fn params are the props.
            props = params.map((f) => ({
                name: f.name,
                type: f.type || "",
                docs: "",
            }));
        }
        propsByTag.set(c.name, props);
    }

    // Theme tokens, grouped by their accessor-path prefix so a
    // completion at `t.` offers namespaces and at `t.spacing.` offers
    // that namespace's leaves. `path` is what gets inserted; `name` is
    // the registry key the author is really choosing.
    //
    // Grouping is derived from the paths themselves rather than a fixed
    // list of namespaces, so a design system with its own vocabulary
    // (or extra nesting, like `intent.primary.solid_bg`) works with no
    // change here.
    const tokensByPrefix = new Map();
    const addToken = (prefix, item) => {
        if (!tokensByPrefix.has(prefix)) tokensByPrefix.set(prefix, []);
        const bucket = tokensByPrefix.get(prefix);
        if (!bucket.some((x) => x.segment === item.segment)) bucket.push(item);
    };
    for (const t of json.style_tokens || []) {
        const segs = (t.path || "").split(".").filter(Boolean);
        if (!segs.length) continue;
        for (let i = 0; i < segs.length; i++) {
            const prefix = segs.slice(0, i).join(".");
            const isLeaf = i === segs.length - 1;
            addToken(prefix, {
                segment: segs[i],
                leaf: isLeaf,
                // Leaves are calls; groups are plain path segments.
                insert: isLeaf ? `${segs[i]}()` : segs[i],
                name: isLeaf ? t.name : "",
                valueType: isLeaf ? t.value_type || "" : "",
                defaultValue: isLeaf ? t.default_value || "" : "",
                vocabulary: t.vocabulary || "",
            });
        }
    }

    // Prop VALUES. Three sources answer "what can I write after
    // `tone = `?":
    // - `values`: open-set markers registered with
    //   `#[schema(value_of = "ToneRef", via = "tone")]` — keyed by the
    //   prop type they coerce into, already spelled as written at a
    //   call site (`tone::Primary`);
    // - `types` with an enum shape: closed sets, spelled `Enum::Variant`;
    // - `icon_sets`: every icon constant, for `IconData` props.
    const valuesByTarget = new Map();
    for (const v of json.values || []) {
        if (!valuesByTarget.has(v.value_of)) valuesByTarget.set(v.value_of, []);
        valuesByTarget.get(v.value_of).push({
            spelled: v.spelled || v.short_name,
            docs: v.docs || "",
            modulePath: v.module_path || "",
        });
    }
    const enumsByName = new Map();
    for (const t of json.types || []) {
        const shape = t.shape || {};
        if (shape.kind !== "enum" || !Array.isArray(shape.variants)) continue;
        const name = t.short_name || (t.fqn || "").split("::").pop();
        if (!name) continue;
        enumsByName.set(name, {
            modulePath: t.module_path || "",
            variants: shape.variants.map((v) => ({
                name: v.name,
                docs: v.docs || "",
                // Payload-carrying variants need arguments the author
                // must fill in; a unit variant is complete as written.
                unit: !(v.payload && v.payload.length),
            })),
        });
    }
    const icons = [];
    for (const set of json.icon_sets || []) {
        const prefix = set.import_path || (set.name || "").replace(/-/g, "_");
        for (const i of set.icons || []) {
            icons.push({ spelled: `${prefix}::${i.ident}`, name: i.name, set: set.title || set.name });
        }
    }

    // Authoring hints: the reactive/component vocabulary an author types
    // in a `#[component]` body (or at item level), from the catalog's
    // macro + utility tables — each carries an insertable `snippet` in
    // LSP syntax plus docs. `itemLevel` marks the ones that declare an
    // item (a component fn, a props struct, a stylesheet) rather than
    // a statement.
    const authoring = [];
    const itemLevel = new Set(["component", "props", "stylesheet"]);
    for (const m of json.macros || []) {
        if (!m.snippet) continue;
        const attribute = (m.invocation || "").startsWith("#[");
        authoring.push({
            label: attribute ? m.invocation : `${m.name}!`,
            insert: m.snippet,
            detail: `idealyst macro · ${m.kind || ""}`,
            docs: [m.docs, m.expansion && `Expands to: \`${m.expansion}\``].filter(Boolean).join("\n\n"),
            itemLevel: itemLevel.has(m.name),
        });
    }
    for (const u of json.utilities || []) {
        if (!u.snippet) continue;
        authoring.push({
            label: u.name,
            insert: u.snippet,
            detail: `idealyst · ${u.category || ""} · ${u.return_type || ""}`,
            docs: u.docs || "",
            itemLevel: false,
        });
    }

    return { tags, propsByTag, tokensByPrefix, valuesByTarget, enumsByName, icons, authoring };
}

// ---------------------------------------------------------------------------
// Cursor-context detection (text heuristics — deliberately simple)
// ---------------------------------------------------------------------------

/** How far back we look for the enclosing macro. */
const LOOKBACK = 6000;

/**
 * Blank out comment and string-literal CONTENTS (offset-preserving —
 * every replaced char becomes a space, newlines survive) so the
 * brace/paren scanners below never trip over prose. This bug was found
 * the fun way: the test project's own doc comment says "type `Button(`
 * …", and the unmatched paren in that PROSE convinced the scanner the
 * cursor was inside a prop list. Handles line comments, block comments,
 * double-quoted strings with escapes, and char literals (best-effort on
 * raw strings; lifetimes like `'a` are left alone).
 */
function sanitize(s) {
    const out = s.split("");
    const blank = (i) => {
        if (s[i] !== "\n") out[i] = " ";
    };
    let i = 0;
    const n = s.length;
    while (i < n) {
        const c = s[i];
        const d = i + 1 < n ? s[i + 1] : "";
        if (c === "/" && d === "/") {
            while (i < n && s[i] !== "\n") blank(i++);
        } else if (c === "/" && d === "*") {
            blank(i++);
            blank(i++);
            while (i < n && !(s[i] === "*" && s[i + 1] === "/")) blank(i++);
            if (i < n) {
                blank(i++);
                blank(i++);
            }
        } else if (c === '"') {
            blank(i++);
            while (i < n && s[i] !== '"') {
                if (s[i] === "\\") blank(i++);
                if (i < n) blank(i++);
            }
            if (i < n) blank(i++);
        } else if (c === "'" && (d === "\\" || (i + 2 < n && s[i + 2] === "'"))) {
            // char literal (not a lifetime)
            blank(i++);
            while (i < n && s[i] !== "'") {
                if (s[i] === "\\") blank(i++);
                if (i < n) blank(i++);
            }
            if (i < n) blank(i++);
        } else {
            i++;
        }
    }
    return out.join("");
}

/**
 * True when `offset` sits inside a `ui! { … }` / `jsx! { … }` block:
 * find the last macro opener before the cursor and check its braces
 * never close back to zero before the cursor.
 */
function insideUiMacro(text, offset) {
    const start = Math.max(0, offset - LOOKBACK);
    const slice = sanitize(text.slice(start, offset));
    const re = /\b(?:ui|jsx)!\s*\{/g;
    let opener = -1;
    let m;
    while ((m = re.exec(slice)) !== null) opener = m.index + m[0].length;
    if (opener === -1) return false;
    let depth = 1;
    for (let i = opener; i < slice.length; i++) {
        const ch = slice[i];
        if (ch === "{") depth++;
        else if (ch === "}") depth--;
        if (depth === 0) return false;
    }
    return true;
}

/**
 * True when `offset` sits inside a `stylesheet! { … }` block. Same
 * unmatched-opener walk as `insideUiMacro`; kept separate because the
 * two macros offer completely different vocabularies.
 */
function insideStylesheetMacro(text, offset) {
    const start = Math.max(0, offset - LOOKBACK);
    const slice = sanitize(text.slice(start, offset));
    const re = /\bstylesheet!\s*\{/g;
    let opener = -1;
    let m;
    while ((m = re.exec(slice)) !== null) opener = m.index + m[0].length;
    if (opener === -1) return false;
    let depth = 1;
    for (let i = opener; i < slice.length; i++) {
        const ch = slice[i];
        if (ch === "{") depth++;
        else if (ch === "}") depth--;
        if (depth === 0) return false;
    }
    return true;
}

/**
 * Where the cursor sits relative to Rust items: `"fn"` inside a function
 * body (any depth — a closure or block within one still counts),
 * `"item"` at item level (module root, or inside an `impl`/`mod` block
 * with no fn around it). Walks the unmatched `{` openers before the
 * cursor and checks the header text in front of each for `fn name(`.
 * Runs over sanitized text so a brace in a string or comment doesn't
 * count.
 */
function rustContext(text, offset) {
    const start = Math.max(0, offset - LOOKBACK);
    const slice = sanitize(text.slice(start, offset));
    const openers = [];
    for (let i = 0; i < slice.length; i++) {
        const ch = slice[i];
        if (ch === "{") openers.push(i);
        else if (ch === "}") openers.pop();
    }
    for (const at of openers) {
        // The header runs back to the previous statement/item boundary.
        const head = slice.slice(0, at);
        const boundary = Math.max(head.lastIndexOf("{"), head.lastIndexOf("}"), head.lastIndexOf(";"));
        const header = head.slice(boundary + 1);
        if (/\bfn\s+[A-Za-z_][A-Za-z0-9_]*\s*[<(]/.test(header)) return "fn";
    }
    return "item";
}

/**
 * If the cursor sits on a token path rooted at the enclosing block's
 * binding — `base(t) { padding: t.spacing.│ }` — return the path prefix
 * already typed (`"spacing"`, or `""` at `t.`). `null` when the cursor
 * isn't on such a path.
 *
 * The binding is read from the nearest block header rather than assumed
 * to be `t`: `base(theme)` is just as valid, and a sheet that opted out
 * with `_t` must NOT complete (that spelling means "no vocabulary here",
 * and the macro doesn't bind it).
 */
function tokenPathContext(text, offset) {
    const start = Math.max(0, offset - LOOKBACK);
    const slice = sanitize(text.slice(start, offset));

    // The dotted path immediately before the cursor.
    const chain = slice.match(/([A-Za-z_][A-Za-z0-9_]*)((?:\s*\.\s*[A-Za-z0-9_]*)+)$/);
    if (!chain) return null;
    const root = chain[1];
    if (root.startsWith("_")) return null;

    // The enclosing block header must bind exactly this identifier.
    // Scan back for the last `name(binding) {` before the cursor.
    const headers = [...slice.matchAll(/\b[A-Za-z_][A-Za-z0-9_]*\s*\(\s*(_?[A-Za-z_][A-Za-z0-9_]*)\s*\)\s*\{/g)];
    if (!headers.length) return null;
    const binding = headers[headers.length - 1][1];
    if (binding !== root) return null;

    // Path segments between the root and the cursor, dropping the
    // partially-typed final segment (VS Code filters on that itself).
    const segs = chain[2].split(".").map((x) => x.trim());
    segs.shift(); // text before the first dot belongs to the root
    segs.pop();   // the in-progress segment
    return segs.join(".");
}

/**
 * If the cursor is inside a tag's prop parens — `Tag(…│…)` — return
 * { tag, written } where `written` is the set of prop names already
 * assigned in the list. Walk backwards counting parens to find the
 * unmatched opener, then read the identifier before it.
 */
function propContext(text, offset) {
    const start = Math.max(0, offset - LOOKBACK);
    const slice = sanitize(text.slice(start, offset));
    let depth = 0;
    for (let i = slice.length - 1; i >= 0; i--) {
        const ch = slice[i];
        if (ch === ")") depth++;
        else if (ch === "(") {
            if (depth > 0) {
                depth--;
                continue;
            }
            // Unmatched opener — the identifier before it is the tag.
            const head = slice.slice(0, i);
            const tag = head.match(/([A-Za-z_][A-Za-z0-9_]*)\s*$/);
            if (!tag) return null;
            const inside = slice.slice(i + 1);
            const written = new Set(
                [...inside.matchAll(/([A-Za-z_][A-Za-z0-9_]*)\s*=/g)].map((x) => x[1])
            );
            return { tag: tag[1], written, inside };
        }
    }
    return null;
}

/**
 * If the cursor sits in a prop's VALUE position — `Tag(…, tone = │)`
 * or `Tag(tone = Pri│)` — return `{ tag, prop, typed }` where `typed`
 * is the partial value so far. `null` anywhere else, including inside
 * a nested expression (`on_click = Rc::new(│`, `count = compute(a, │`):
 * there the value is arbitrary Rust and rust-analyzer owns it.
 *
 * Walk the prop list forward tracking bracket depth so a comma inside
 * a nested call doesn't split the current prop; the current prop is
 * whatever follows the last depth-0 comma, and it must look exactly
 * like `name = <bare path chars>`.
 */
function propValueContext(text, offset) {
    const ctx = propContext(text, offset);
    if (!ctx) return null;
    const inside = ctx.inside;
    let depth = 0;
    let start = 0;
    for (let i = 0; i < inside.length; i++) {
        const ch = inside[i];
        if (ch === "(" || ch === "[" || ch === "{") depth++;
        else if (ch === ")" || ch === "]" || ch === "}") depth--;
        else if (ch === "," && depth === 0) start = i + 1;
    }
    if (depth !== 0) return null;
    const m = inside.slice(start).match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=(?![=>])\s*([A-Za-z0-9_:.]*)$/);
    if (!m) return null;
    return { tag: ctx.tag, prop: m[1], typed: m[2] };
}

/**
 * Normalize a catalog type string (`:: runtime_vocabulary :: glue ::
 * Reactive < Option < ToneRef > >`) into `{ inner, optional }`:
 * `Reactive<…>` is transparent (the macro's `.into()` coerces a plain
 * value into it), `Option<…>` is remembered so values get wrapped in
 * `Some(…)`. Anything else — `Signal<T>`, `ReadSignal<T>`, `Ref<H>` —
 * needs a live handle, not a literal, so it stays as-is and matches
 * nothing below.
 */
function unwrapPropType(typeStr) {
    let t = (typeStr || "").replace(/\s+/g, "");
    t = t.replace(/^&(mut)?/, "");
    t = t.replace(/^(::)?(runtime_vocabulary::glue::|runtime_core::|std::rc::)/, "");
    let optional = false;
    for (;;) {
        let m = t.match(/^Reactive<(.*)>$/);
        if (m) {
            t = m[1];
            continue;
        }
        m = t.match(/^Option<(.*)>$/);
        if (m) {
            optional = true;
            t = m[1];
            continue;
        }
        break;
    }
    t = t.replace(/^(::)?(runtime_vocabulary::glue::|runtime_core::|std::rc::)/, "");
    return { inner: t, optional };
}

/**
 * Candidate values for a prop type, as `{ label, insert, snippet, detail,
 * docs, kind }`. `insert` is the text (or snippet body when `snippet`)
 * placed at the cursor — already `Some(…)`-wrapped for `Option` props,
 * with the `.into()` an `Option<Ref>` needs (the macro's coercion
 * doesn't reach through `Option`, so `Some(tone::Danger.into())` is
 * the idiom).
 */
function valuesForType(catalog, typeStr) {
    const { inner, optional } = unwrapPropType(typeStr);
    const short = inner.split("<")[0].split("::").pop();
    const out = [];
    const push = (label, insert, extra) => out.push({ label, insert, ...extra });
    const wrap = (x, coerce) => (optional ? `Some(${x}${coerce ? ".into()" : ""})` : x);

    if (inner === "bool") {
        push(wrap("true"), wrap("true"), { kind: "Keyword" });
        push(wrap("false"), wrap("false"), { kind: "Keyword" });
    }

    // `Rc<dyn Fn(A, B) -> R>` → a closure the author fills in. Bare
    // closures don't `.into()` an `Rc<dyn Fn>`, so `Rc::new` is spelled.
    const fnm = inner.match(/^Rc<dynFn\((.*?)\)(->.*)?>$/);
    if (fnm) {
        const args = fnm[1] ? fnm[1].split(",").length : 0;
        const params = Array.from({ length: args }, (_, i) => `\${${i + 1}:arg${i + 1}}`).join(", ");
        const body = `Rc::new(move |${params}| { $${args + 1} })`;
        push(
            optional ? "Some(Rc::new(move |…| { … }))" : "Rc::new(move |…| { … })",
            optional ? `Some(${body})` : body,
            { snippet: true, kind: "Snippet", detail: inner }
        );
    }

    for (const v of catalog.valuesByTarget.get(short) || []) {
        push(wrap(v.spelled, true), wrap(v.spelled, true), {
            kind: "EnumMember",
            detail: `${short} · ${v.modulePath}`,
            docs: v.docs,
        });
    }

    const en = catalog.enumsByName.get(short);
    if (en) {
        for (const v of en.variants) {
            const x = `${short}::${v.name}`;
            push(wrap(x), v.unit ? wrap(x) : wrap(`${x}($1)`), {
                snippet: !v.unit,
                kind: "EnumMember",
                detail: `${short} · ${en.modulePath}`,
                docs: v.docs,
            });
        }
    }

    if (short === "IconData") {
        for (const i of catalog.icons) {
            push(wrap(i.spelled), wrap(i.spelled), { kind: "Constant", detail: `icon · ${i.set}` });
        }
    }

    if (optional && out.length) {
        push("None", "None", { kind: "Keyword" });
    }
    return out;
}

// ---------------------------------------------------------------------------
// Completion provider
// ---------------------------------------------------------------------------

/**
 * Completion items for the reactive/component vocabulary. At item level
 * only the declaration skeletons make sense (`#[component]`, `#[props]`,
 * `stylesheet!`); inside a fn body everything but the item-level ones.
 */
function authoringItems(catalog, where) {
    const items = (catalog.authoring || []).filter((a) =>
        where === "fn" ? !a.itemLevel || a.label === "stylesheet!" : a.itemLevel
    );
    return items.map((a) => {
        const it = new vscode.CompletionItem(a.label, vscode.CompletionItemKind.Snippet);
        it.insertText = new vscode.SnippetString(a.insert);
        it.detail = a.detail;
        it.documentation = mdDocs({ docs: a.docs });
        // `#[component]` must still match when the author types `comp`.
        it.filterText = a.label.replace(/^#\[|\]$/g, "");
        it.sortText = `0_${a.label}`;
        return it;
    });
}

function mdDocs(item) {
    const md = new vscode.MarkdownString();
    if (item.type) md.appendCodeblock(item.type, "rust");
    if (item.docs) md.appendMarkdown(item.docs);
    return md;
}

const provider = {
    provideCompletionItems(document, position) {
        // A throw here is swallowed by VS Code (logged only to the
        // extension-host log as "provider FAILED") and the author just
        // sees an empty popup. Surface it where they'll look.
        try {
            return completeAt(document, position);
        } catch (e) {
            log(`completion FAILED: ${e.stack || e}`);
            throw e;
        }
    },
};

function completeAt(document, position) {
    const folderUri = vscode.workspace.getWorkspaceFolder(document.uri);
    if (!folderUri) return undefined;
    const project = projectFor(document.uri.fsPath, folderUri.uri.fsPath);
    if (!project) return undefined;

    const text = document.getText();
    const offset = document.offsetAt(position);
    const inStylesheet = insideStylesheetMacro(text, offset);
    const inUi = !inStylesheet && insideUiMacro(text, offset);

    // Lazy first load. A completion inside one of our macros always
    // starts it; plain-Rust authoring hints only start it for an exact
    // project, so typing in a big workspace's shared library never
    // kicks off the merged multi-app build by itself.
    if (inStylesheet || inUi || project.exact) loadCatalog(project.dir);
    const catalog = catalogs.get(project.dir);
    if (!catalog) return undefined;

    if (!inStylesheet && !inUi) {
        return authoringItems(catalog, rustContext(text, offset));
    }

    // stylesheet! — theme tokens off the block binding.
    if (inStylesheet) {
        const prefix = tokenPathContext(text, offset);
        if (prefix === null) return undefined;
        const bucket = (catalog.tokensByPrefix || new Map()).get(prefix);
        if (!bucket || !bucket.length) return undefined;
        return bucket.map((t) => {
            const it = new vscode.CompletionItem(
                t.segment,
                t.leaf
                    ? vscode.CompletionItemKind.Constant
                    : vscode.CompletionItemKind.Module
            );
            it.insertText = t.insert;
            if (t.leaf) {
                // The registry name is what the author is really
                // choosing; the default is what they'll see before a
                // theme installs.
                it.detail = `${t.name} · ${t.defaultValue}`;
                const md = new vscode.MarkdownString();
                md.appendCodeblock(
                    `Tokenized<${t.valueType}>  //  ${t.name} = ${t.defaultValue}`,
                    "rust"
                );
                md.appendMarkdown(
                    `Theme token \`${t.name}\`. Resolves from the installed ` +
                    `theme at render time; \`${t.defaultValue}\` is the ` +
                    `${t.vocabulary} base value shown before a theme installs.`
                );
                it.documentation = md;
            } else {
                it.detail = "token namespace";
            }
            it.sortText = `0_${t.segment}`; // above RA's inherent-method noise
            return it;
        });
    }

    // Prop VALUE completion: `Tag(tone = │)`.
    const vctx = propValueContext(text, offset);
    if (vctx && catalog.propsByTag.has(vctx.tag)) {
        const prop = catalog.propsByTag.get(vctx.tag).find((p) => p.name === vctx.prop);
        if (!prop) return undefined;
        const values = valuesForType(catalog, prop.type);
        if (!values.length) return undefined;
        return values.map((v, i) => {
            const it = new vscode.CompletionItem(
                v.label,
                vscode.CompletionItemKind[v.kind] || vscode.CompletionItemKind.Value
            );
            it.insertText = v.snippet ? new vscode.SnippetString(v.insert) : v.insert;
            it.detail = v.detail || prop.type;
            if (v.docs) it.documentation = mdDocs({ docs: v.docs });
            // Keep catalog order (values before enums before icons, None
            // last) and float the lot above RA's grab-bag.
            it.sortText = `0_${String(i).padStart(5, "0")}`;
            return it;
        });
    }

    const ctx = propContext(text, offset);
    // Inside a nested call within a prop value — a handler body
    // (`on_click = Rc::new(move || { │ })`), a constructor — the author
    // is writing plain Rust, so offer the reactive vocabulary rather
    // than tags.
    if (ctx && !catalog.propsByTag.has(ctx.tag)) {
        return authoringItems(catalog, "fn");
    }
    if (ctx && catalog.propsByTag.has(ctx.tag)) {
        // Prop-name completion for the enclosing tag.
        return catalog.propsByTag
            .get(ctx.tag)
            .filter((p) => !ctx.written.has(p.name))
            .map((p) => {
                const it = new vscode.CompletionItem(
                    p.name,
                    vscode.CompletionItemKind.Field
                );
                it.detail = p.type;
                it.documentation = mdDocs(p);
                it.insertText = new vscode.SnippetString(`${p.name} = $0`);
                it.sortText = `0_${p.name}`; // float props above RA's noise
                return it;
            });
    }

    // Tag completion (child position).
    return catalog.tags.map((t) => {
        const it = new vscode.CompletionItem(t.name, t.kind);
        it.detail = t.detail;
        it.documentation = mdDocs(t);
        return it;
    });
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/**
 * Warm the catalog for the crate the active editor sits in — but only
 * when that crate is itself an idealyst project. The workspace-root
 * fallback (`exact: false`) can wrap dozens of members in a big
 * monorepo, so it waits for an actual completion request.
 */
function warmFor(editor) {
    if (!editor || editor.document.languageId !== "rust") return;
    const folderUri = vscode.workspace.getWorkspaceFolder(editor.document.uri);
    if (!folderUri) return;
    const project = projectFor(editor.document.uri.fsPath, folderUri.uri.fsPath);
    if (project && project.exact) loadCatalog(project.dir);
}

function activate(context) {
    output = vscode.window.createOutputChannel("Idealyst");
    context.subscriptions.push(
        output,
        vscode.languages.registerCompletionItemProvider(
            { language: "rust" },
            provider,
            "(", // prop list opens
            ",", // next prop
            "=", // prop value position
            "."  // token path segment
        ),
        vscode.commands.registerCommand("idealyst.refreshCatalog", () => {
            const editor = vscode.window.activeTextEditor;
            const folderUri =
                editor && vscode.workspace.getWorkspaceFolder(editor.document.uri);
            const project =
                folderUri && projectFor(editor.document.uri.fsPath, folderUri.uri.fsPath);
            catalogs.clear();
            if (project) {
                loadCatalog(project.dir, { force: true });
            } else {
                log("refresh: the active editor is not inside an idealyst project");
                vscode.window.setStatusBarMessage(
                    "idealyst: open a file inside an idealyst crate, then refresh",
                    6000
                );
            }
        }),
        vscode.window.onDidChangeActiveTextEditor(warmFor)
    );
    warmFor(vscode.window.activeTextEditor);
}

function deactivate() {}

module.exports = {
    activate,
    deactivate,
    // Pure helpers exposed for the node-side test harness (test.js).
    __test: {
        digest,
        projectFor,
        loadCatalog,
        catalogs,
        insideUiMacro,
        propContext,
        propValueContext,
        unwrapPropType,
        valuesForType,
        rustContext,
        authoringItems,
        insideStylesheetMacro,
        tokenPathContext,
    },
};
