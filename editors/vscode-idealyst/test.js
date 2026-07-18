// Node-side harness for the extension's pure logic — no VS Code needed.
// Run: node test.js /path/to/catalog.json
//
// Injects a minimal `vscode` mock into the require cache (the real
// module only exists inside the editor host), then exercises `digest`,
// `insideUiMacro`, and `propContext` against a REAL catalog dump and
// representative cursor states.

const path = require("path");
const Module = require("module");

// --- vscode mock, installed before extension.js loads ---
const mock = {
    CompletionItemKind: { Function: 2, Field: 4, Class: 6 },
    CompletionItem: class {
        constructor(label, kind) {
            this.label = label;
            this.kind = kind;
        }
    },
    SnippetString: class {
        constructor(value) {
            this.value = value;
        }
    },
    MarkdownString: class {
        appendCodeblock() {}
        appendMarkdown() {}
    },
    window: { setStatusBarMessage: () => ({ dispose() {} }) },
    workspace: {
        getConfiguration: () => ({ get: () => "idealyst" }),
        workspaceFolders: [],
    },
    languages: { registerCompletionItemProvider: () => ({}) },
    commands: { registerCommand: () => ({}) },
};
const origResolve = Module._resolveFilename;
Module._resolveFilename = function (request, ...rest) {
    if (request === "vscode") return "vscode";
    return origResolve.call(this, request, ...rest);
};
require.cache["vscode"] = { id: "vscode", filename: "vscode", loaded: true, exports: mock };

const { __test } = require(path.join(__dirname, "extension.js"));
const { digest, insideUiMacro, propContext } = __test;

let failures = 0;
function check(name, cond, extra) {
    if (cond) {
        console.log(`ok   ${name}`);
    } else {
        failures++;
        console.log(`FAIL ${name}${extra ? ` — ${extra}` : ""}`);
    }
}

// --- digest against a real catalog dump ---
const catalogPath = process.argv[2];
if (!catalogPath) {
    console.error("usage: node test.js /path/to/catalog.json");
    process.exit(2);
}
const cat = digest(require(catalogPath.startsWith("/") ? catalogPath : path.resolve(catalogPath)));

check("digest: primitives present as tags", cat.tags.some((t) => t.name === "view"));
check("digest: components present as tags", cat.tags.some((t) => t.name === "Button"));
const buttonProps = (cat.propsByTag.get("Button") || []).map((p) => p.name);
check(
    "digest: explicit-props component fields inlined",
    buttonProps.includes("label") && buttonProps.includes("on_click"),
    `got ${buttonProps.slice(0, 5)}`
);
const counterProps = (cat.propsByTag.get("Counter") || []).map((p) => p.name);
check(
    "digest: inline-props component params are props",
    counterProps.includes("start"),
    `got ${counterProps}`
);
const textProps = (cat.propsByTag.get("text") || []).map((p) => p.name);
check("digest: primitive props present", textProps.length > 0, "text has no props");

// --- cursor-context detection ---
// Mid-typing state: the cursor sits after `toz` inside Button's
// (auto-closed) parens. `toz` chosen to appear exactly once.
const src = `
fn app() -> Element {
    let outside = compute();
    ui! {
        view() {
            Button(label = "hi", toz)
            text { "x" }
        }
    }
}
let after = 1;
`;
const at = (needle) => src.indexOf(needle) + needle.length;

check("insideUiMacro: inside block", insideUiMacro(src, at("toz")));
check("insideUiMacro: before block", !insideUiMacro(src, at("compute()")));
check("insideUiMacro: after block", !insideUiMacro(src, at("let after = 1;")));

const ctx = propContext(src, at("toz"));
check("propContext: finds enclosing tag", ctx && ctx.tag === "Button", ctx && ctx.tag);
check("propContext: collects written props", ctx && ctx.written.has("label"));

// Child position: every paren before the cursor is balanced, so there
// is no enclosing prop list.
const childCtx = propContext(src, at('text { "x'));
check(
    "propContext: child position has no prop context (or a non-tag)",
    !childCtx || !cat.propsByTag.has(childCtx.tag),
    childCtx && childCtx.tag
);

// nested parens inside a prop value must not confuse the tag scan
const nested = `ui! { Badge(count = compute(a, b), la`;
const nctx = propContext(nested, nested.length);
check("propContext: nested call parens skipped", nctx && nctx.tag === "Badge", nctx && nctx.tag);

// REGRESSION: prose in comments/strings must not leak into the scans.
// The original bug: a doc comment saying "type \`Button(\` …" above the
// block left an unmatched paren, so a bare tag-position cursor was
// misread as being inside Button's prop list (empty popup for the user).
const prose = `
// Try this: type \`Button(\` to see prop completion.
fn f() -> Element {
    ui! {
        view() {
            text { "unbalanced :) and { brace in a string" }
            But
        }
    }
}`;
const pOff = prose.indexOf("But\n") + 3;
check("sanitize: comment prose doesn't fake a prop context", propContext(prose, pOff) === null,
    JSON.stringify(propContext(prose, pOff)));
check("sanitize: string braces don't break block detection", insideUiMacro(prose, pOff));

// A commented-out `ui! {` opener must not count as a block.
const commented = `// ui! {\nlet x = 1;\n`;
check("sanitize: commented ui! opener ignored", !insideUiMacro(commented, commented.length));

process.exit(failures ? 1 : 0);
