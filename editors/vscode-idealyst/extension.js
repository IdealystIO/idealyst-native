// Idealyst VS Code extension — DSL-vocabulary completion for ui!/jsx!.
//
// rust-analyzer owns types and expressions (including inside the macros,
// via the macro's IDE-recovery expansion). What RA can NOT know is the
// DSL vocabulary: which tags exist, which props a tag takes, what they
// mean. That data lives in the idealyst catalog (`inventory`-registered
// components/primitives with docs), fetched here by shelling out to
// `idealyst catalog-json` once per workspace and cached in memory.
//
// Deliberately dependency-free plain JS: installable by symlinking this
// folder into ~/.vscode/extensions — no build step, no vsce.

const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const path = require("path");

// ---------------------------------------------------------------------------
// Catalog loading
// ---------------------------------------------------------------------------

/** workspaceFolder.uri.fsPath → { tags, propsByTag } */
const catalogs = new Map();
/** folders with a load in flight, so we don't spawn twice */
const loading = new Set();

function cliPath() {
    return vscode.workspace.getConfiguration("idealyst").get("cli") || "idealyst";
}

/** An idealyst project declares itself in Cargo metadata. */
function isIdealystProject(folder) {
    try {
        const manifest = fs.readFileSync(path.join(folder, "Cargo.toml"), "utf8");
        return manifest.includes("[package.metadata.idealyst");
    } catch {
        return false;
    }
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

    return { tags, propsByTag };
}

function loadCatalog(folder, { force = false } = {}) {
    if (!force && (catalogs.has(folder) || loading.has(folder))) return;
    if (!isIdealystProject(folder)) return;
    loading.add(folder);

    const status = vscode.window.setStatusBarMessage(
        "$(sync~spin) idealyst: loading catalog…"
    );
    // First run compiles the catalog wrapper — can take minutes cold.
    cp.execFile(
        cliPath(),
        ["catalog-json", "."],
        { cwd: folder, maxBuffer: 64 * 1024 * 1024, timeout: 10 * 60 * 1000 },
        (err, stdout) => {
            status.dispose();
            loading.delete(folder);
            if (err) {
                vscode.window.setStatusBarMessage(
                    "idealyst: catalog load failed (see `idealyst catalog-json`)",
                    8000
                );
                console.error("[idealyst] catalog-json failed:", err.message);
                return;
            }
            try {
                catalogs.set(folder, digest(JSON.parse(stdout)));
                vscode.window.setStatusBarMessage("idealyst: catalog ready", 4000);
            } catch (e) {
                console.error("[idealyst] catalog parse failed:", e.message);
            }
        }
    );
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
            return { tag: tag[1], written };
        }
    }
    return null;
}

// ---------------------------------------------------------------------------
// Completion provider
// ---------------------------------------------------------------------------

function mdDocs(item) {
    const md = new vscode.MarkdownString();
    if (item.type) md.appendCodeblock(item.type, "rust");
    if (item.docs) md.appendMarkdown(item.docs);
    return md;
}

const provider = {
    provideCompletionItems(document, position) {
        const folderUri = vscode.workspace.getWorkspaceFolder(document.uri);
        if (!folderUri) return undefined;
        const folder = folderUri.uri.fsPath;
        loadCatalog(folder); // lazy first load
        const catalog = catalogs.get(folder);
        if (!catalog) return undefined;

        const text = document.getText();
        const offset = document.offsetAt(position);
        if (!insideUiMacro(text, offset)) return undefined;

        const ctx = propContext(text, offset);
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
    },
};

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

function activate(context) {
    context.subscriptions.push(
        vscode.languages.registerCompletionItemProvider(
            { language: "rust" },
            provider,
            "(", // prop list opens
            ","  // next prop
        ),
        vscode.commands.registerCommand("idealyst.refreshCatalog", () => {
            for (const f of vscode.workspace.workspaceFolders || []) {
                catalogs.delete(f.uri.fsPath);
                loadCatalog(f.uri.fsPath, { force: true });
            }
        })
    );
    // Warm the catalog for already-open idealyst workspaces.
    for (const f of vscode.workspace.workspaceFolders || []) {
        loadCatalog(f.uri.fsPath);
    }
}

function deactivate() {}

module.exports = {
    activate,
    deactivate,
    // Pure helpers exposed for the node-side test harness (test.js).
    __test: { digest, insideUiMacro, propContext },
};
