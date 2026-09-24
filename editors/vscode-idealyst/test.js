// Node-side harness for the extension's pure logic — no VS Code needed.
// Run: node test.js /path/to/catalog.json
//
// Injects a minimal `vscode` mock into the require cache (the real
// module only exists inside the editor host), then exercises `digest`,
// `insideUiMacro`, and `propContext` against a REAL catalog dump and
// representative cursor states.

const path = require("path");
const fs = require("fs");
const os = require("os");
const Module = require("module");

// --- vscode mock, installed before extension.js loads ---
const mock = {
    CompletionItemKind: { Function: 2, Field: 4, Class: 6, Module: 8, Constant: 20 },
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
        getConfiguration: () => ({ get: () => mock.__cli }),
        workspaceFolders: [],
    },
    /** what `idealyst.cli` resolves to; the loader test points it at a stub */
    __cli: "idealyst",
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
const {
    digest,
    projectFor,
    loadCatalog,
    catalogs,
    insideUiMacro,
    propContext,
    propValueContext,
    unwrapPropType,
    valuesForType,
    importPlan,
    crateNameFor,
    hoverAt,
    mergeScans,
    scanCrate,
    manifestDeps,
    rustContext,
    authoringItems,
    onAssignmentRhs,
    stripBinding,
    insideStylesheetMacro,
    tokenPathContext,
} = __test;

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
const raw = require(catalogPath.startsWith("/") ? catalogPath : path.resolve(catalogPath));
const cat = digest(raw);

check("digest: primitives present as tags", cat.tags.some((t) => t.name === "view"));
check("digest: components present as tags", cat.tags.some((t) => t.name === "Button"));
const buttonProps = (cat.propsByTag.get("Button") || []).map((p) => p.name);
check(
    "digest: explicit-props component fields inlined",
    buttonProps.includes("label") && buttonProps.includes("on_click"),
    `got ${buttonProps.slice(0, 5)}`
);
// Any inline-props component in the dump (fn params that aren't a lone
// `props: &Xxx` with a schema) must surface those params as its props.
const inline = (raw.components || []).find((c) => {
    const ps = c.params || [];
    return ps.length > 0 && !(ps.length === 1 && (ps[0].name === "props" || Array.isArray(ps[0].schema)));
});
check(
    "digest: inline-props component params are props",
    inline && inline.params.every((p) => (cat.propsByTag.get(inline.name) || []).some((q) => q.name === p.name)),
    inline ? `${inline.name}: got ${(cat.propsByTag.get(inline.name) || []).map((p) => p.name)}` : "no inline-props component in this dump"
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

// --- stylesheet! theme-token completion ---------------------------------

check(
    "digest: style tokens grouped by path prefix",
    (cat.tokensByPrefix.get("") || []).some((t) => t.segment === "spacing")
);
const spacing = cat.tokensByPrefix.get("spacing") || [];
check("digest: spacing namespace has leaves", spacing.some((t) => t.segment === "md"));
const md = spacing.find((t) => t.segment === "md");
check("digest: leaf inserts a call", md && md.insert === "md()", md && md.insert);
check("digest: leaf carries its registry name", md && md.name === "spacing-md", md && md.name);
check("digest: leaf carries a base value", md && /px$/.test(md.defaultValue), md && md.defaultValue);
check(
    "digest: namespace segment is not a leaf",
    (cat.tokensByPrefix.get("") || []).find((t) => t.segment === "spacing").leaf === false
);
check(
    "digest: nested intent path groups three deep",
    (cat.tokensByPrefix.get("intent.primary") || []).some((t) => t.segment === "solid_bg")
);

const sheet = `
stylesheet! {
    pub Sidebar<IdeaThemeRef> {
        base(t) {
            padding: t.spacing.
        }
    }
}`;
const atNs = sheet.indexOf("t.spacing.") + "t.spacing.".length;
check("stylesheet: inside the macro", insideStylesheetMacro(sheet, atNs));
check("stylesheet: not a ui! block", !insideUiMacro(sheet, atNs));
check("token path: namespace prefix resolved", tokenPathContext(sheet, atNs) === "spacing",
    JSON.stringify(tokenPathContext(sheet, atNs)));

const atRoot = sheet.indexOf("t.spacing.") + 2;
check("token path: bare `t.` yields the root prefix", tokenPathContext(sheet, atRoot) === "",
    JSON.stringify(tokenPathContext(sheet, atRoot)));

// A three-segment path (intent) must resolve its two-segment prefix.
const intentSheet = `
stylesheet! {
    pub S<IdeaThemeRef> {
        base(t) {
            background: t.intent.primary.
        }
    }
}`;
const atIntent = intentSheet.indexOf("primary.") + "primary.".length;
check("token path: nested prefix resolved", tokenPathContext(intentSheet, atIntent) === "intent.primary",
    JSON.stringify(tokenPathContext(intentSheet, atIntent)));

// `_t` is the opt-out spelling — the macro doesn't bind it, so offering
// tokens there would suggest code that doesn't compile.
const optedOut = `
stylesheet! {
    pub S<()> {
        base(_t) {
            padding: _t.
        }
    }
}`;
const atOptOut = optedOut.indexOf("_t.") + 3 + optedOut.slice(optedOut.indexOf("_t.") + 3).indexOf("");
check("token path: `_t` binding offers nothing",
    tokenPathContext(optedOut, optedOut.lastIndexOf("_t.") + 3) === null);

// A dotted chain whose root ISN'T the block binding is someone else's
// receiver — RA owns that completion, we must not hijack it.
const otherRecv = `
stylesheet! {
    pub S<IdeaThemeRef> {
        base(t) {
            padding: other.
        }
    }
}`;
check("token path: unrelated receiver ignored",
    tokenPathContext(otherRecv, otherRecv.indexOf("other.") + 6) === null);

// Outside any stylesheet!, a `t.` chain must not offer tokens.
const plain = `fn f() { let t = thing(); t. }`;
check("stylesheet: plain code is not a sheet", !insideStylesheetMacro(plain, plain.indexOf("t. ") + 2));

// --- prop VALUE completion ---
// Context detection: only a bare `name = <path chars>` at depth 0 of
// the prop list counts; nested expressions belong to rust-analyzer.
const vsrc = `ui! { Button(label = "Save", tone = ) }`;
const vAt = vsrc.indexOf("tone = ") + 7;
const v1 = propValueContext(vsrc, vAt);
check("propValueContext: empty value after `=`", v1 && v1.tag === "Button" && v1.prop === "tone" && v1.typed === "",
    JSON.stringify(v1));
const vsrc2 = `ui! { Button(label = "Save", tone = tone::Pri`;
const v2 = propValueContext(vsrc2, vsrc2.length);
check("propValueContext: partial path value", v2 && v2.prop === "tone" && v2.typed === "tone::Pri", JSON.stringify(v2));
const vsrc3 = `ui! { Button(on_click = Rc::new(move || { let x = `;
check("propValueContext: nested expression is not a value position", propValueContext(vsrc3, vsrc3.length) === null);
const vsrc4 = `ui! { Badge(count = compute(a, `;
check("propValueContext: inside a nested call is not a value position", propValueContext(vsrc4, vsrc4.length) === null);
const vsrc5 = `ui! { Button(label = "Save", `;
check("propValueContext: prop-name position is not a value position", propValueContext(vsrc5, vsrc5.length) === null);
const vsrc6 = `ui! { Toggle(value = v, on_change = move |x| x == `;
check("propValueContext: `==` inside a closure is not a value position", propValueContext(vsrc6, vsrc6.length) === null);

// Type normalization: Reactive is transparent, Option is remembered,
// the vocabulary prefix is stripped.
check("unwrapPropType: Reactive<Option<ToneRef>> with glue prefix",
    JSON.stringify(unwrapPropType(":: runtime_vocabulary :: glue :: Reactive < Option < ToneRef > >")) ===
        JSON.stringify({ inner: "ToneRef", optional: true }));
check("unwrapPropType: Signal stays opaque",
    unwrapPropType("Signal<bool>").inner === "Signal<bool>");

// Values, against the real dump plus a synthesized `values` slice (the
// registry-pinned dump predates the slice; the shape is what
// `ValueEntry::to_json` emits).
const withValues = Object.assign({}, raw, {
    values: [
        { short_name: "Primary", module_path: "idea_theme::extensible::tone", docs: "Built-in semantic tone.",
          value_of: "ToneRef", via: "tone", spelled: "tone::Primary" },
        { short_name: "Danger", module_path: "idea_theme::extensible::tone", docs: "",
          value_of: "ToneRef", via: "tone", spelled: "tone::Danger" },
    ],
});
const vcat = digest(withValues);
const labels = (t) => valuesForType(vcat, t).map((v) => v.label);
check("values: open-set markers by prop type", labels("Reactive<ToneRef>").join() === "tone::Primary,tone::Danger",
    labels("Reactive<ToneRef>").join());
check("values: Option<Ref> wraps with Some(..into()) and adds None",
    labels(":: runtime_vocabulary :: glue :: Reactive < Option < ToneRef > >").join() ===
        "Some(tone::Primary.into()),Some(tone::Danger.into()),None",
    labels("Reactive<Option<ToneRef>>").join());
check("values: bool", labels("bool").join() === "true,false");
check("values: Reactive<bool>", labels("::runtime_vocabulary::glue::Reactive<bool>").join() === "true,false");
check("values: Signal<bool> offers nothing (needs a handle)", labels("Signal<bool>").length === 0);
check("values: String offers nothing", labels("String").length === 0);
const fn0 = valuesForType(vcat, "Rc<dyn Fn()>");
check("values: Rc<dyn Fn()> closure snippet", fn0.length === 1 && fn0[0].snippet && fn0[0].insert === "Rc::new(move || { $1 })", JSON.stringify(fn0));
const fn2 = valuesForType(vcat, "Rc<dyn Fn(String, u32) -> bool>");
check("values: Fn(A, B) gets two params", fn2[0].insert === "Rc::new(move |${1:arg1}, ${2:arg2}| { $3 })", fn2[0].insert);
const fnOpt = valuesForType(vcat, "Option<Rc<dyn Fn()>>");
check("values: Option<Rc<dyn Fn()>>", fnOpt.map((v) => v.label).join() === "Some(Rc::new(move |…| { … })),None", fnOpt.map((v) => v.label).join());
// A real IdealystSchema enum from the dump.
const enumName = [...vcat.enumsByName.keys()][0];
if (enumName) {
    const ev = labels(`Reactive<${enumName}>`);
    check(`values: enum variants (${enumName})`, ev.length > 0 && ev.every((l) => l.startsWith(`${enumName}::`)), ev.join());
} else {
    check("values: enum variants", false, "no enum in dump");
}
if ((raw.icon_sets || []).length) {
    const iconVals = valuesForType(vcat, "::runtime_vocabulary::glue::Reactive<Option<IconData>>");
    check("values: IconData offers icon constants wrapped in Some", iconVals.length > 1 && iconVals[0].label.startsWith("Some(icons_lucide::") && iconVals[iconVals.length - 1].label === "None",
        iconVals.slice(0, 2).map((v) => v.label).join());
} else {
    check("values: IconData offers nothing when the dump has no icon sets",
        valuesForType(vcat, "Reactive<Option<IconData>>").length === 0);
}

// --- auto-import for completed values ---
// REGRESSION: accepting `typography_kind::Body` inserted the value but not
// `use idea_ui::typography_kind;`, the file stopped compiling, and with
// it the catalog build that compiles the file.
const scratch = `//! A scratch component.
//!
//! Mount it anywhere.

use idea_ui::{Typography, TypographyKindRef};
use runtime_core::{component, ui, Element};

#[component]
pub fn ScratchText(props: &ScratchTextProps) -> Element {
    ui! { Typography(kind = typography_kind::Body) }
}
`;
const plan = importPlan(scratch, "idea_ui::typography_kind", "crewforge_ui_shared");
check("importPlan: missing module → `use` after the last top-level use",
    plan && plan.text === "\nuse idea_ui::typography_kind;" && scratch.slice(0, plan.offset).endsWith("use runtime_core::{component, ui, Element};"),
    JSON.stringify(plan));
check("importPlan: already imported by name → nothing",
    importPlan(scratch.replace("TypographyKindRef}", "TypographyKindRef, typography_kind}"), "idea_ui::typography_kind", "") === null);
check("importPlan: already imported as its own line → nothing",
    importPlan(scratch + "\nuse idea_ui::typography_kind;\n", "idea_ui::typography_kind", "") === null);
check("importPlan: parent glob → nothing", importPlan("use idea_ui::*;\n", "idea_ui::tone", "") === null);
check("importPlan: a local `mod tone` → nothing", importPlan("mod tone { pub use idea_ui::tone::*; }\n", "idea_ui::tone", "") === null);
check("importPlan: prose mentioning the name doesn't count",
    importPlan("// use idea_ui::tone here\nfn f() {}\n", "idea_ui::tone", "") !== null);
check("importPlan: inside the defining crate the path is crate::",
    (importPlan("fn f() {}\n", "my_app::theme", "my_app") || {}).text === "use crate::theme;\n");
const noUse = importPlan(scratch.replace(/^use .*\n/gm, ""), "idea_ui::tone", "");
check("importPlan: no `use` lines → after the leading //! block",
    noUse && noUse.text === "use idea_ui::tone;\n\n" && scratch.replace(/^use .*\n/gm, "").slice(0, noUse.offset).trim().endsWith("Mount it anywhere."),
    JSON.stringify(noUse));
check("importPlan: empty import path → nothing", importPlan(scratch, "", "") === null);
check("importPlan: a component already imported from the crate root is left alone",
    importPlan("use idea_ui::{Button, Card};\n", "idea_ui::components::button::Button", "") === null);
check("importPlan: a component from another crate gets its module path",
    (importPlan("use runtime_core::ui;\n", "idea_ui::components::button::Button", "my_app") || {}).text === "\nuse idea_ui::components::button::Button;");
check("importPlan: a component of this crate imports through crate::",
    (importPlan("use runtime_core::ui;\n", "my_app::scratch::ScratchText", "my_app") || {}).text === "\nuse crate::scratch::ScratchText;");
check("importPlan: enum type import", (importPlan("fn f() {}", "idea_ui::components::modal::ModalPresentation", "") || {}).text === "use idea_ui::components::modal::ModalPresentation;\n");
if ((raw.values || []).some((v) => v.import)) {
    const kindVals = valuesForType(cat, "Reactive<TypographyKindRef>");
    check("values: carry the catalog import", kindVals.length > 0 && kindVals.every((v) => v.imp === "idea_ui::typography_kind"), JSON.stringify(kindVals[0]));
    const cardVal = valuesForType(cat, "Reactive<VariantRef>").find((v) => v.label.startsWith("card::"));
    check("values: brace-form via imports the first spelled segment", cardVal && cardVal.imp === "idea_ui::components::card", cardVal && cardVal.imp);
}
{
    const tmpc = fs.mkdtempSync(path.join(os.tmpdir(), "idealyst-crate-"));
    fs.mkdirSync(path.join(tmpc, "src"));
    fs.writeFileSync(path.join(tmpc, "Cargo.toml"), '[package]\nname = "crewforge-ui-shared"\nversion = "0.1.0"\n');
    check("crateNameFor: nearest [package] name, dashes to underscores", crateNameFor(path.join(tmpc, "src", "scratch.rs")) === "crewforge_ui_shared");
    fs.rmSync(tmpc, { recursive: true, force: true });
}

// --- hover: component docs + props, prop docs, value docs ---
{
    const hsrc = `
fn f() -> Element {
    let kind = 1;
    ui! {
        Typography(content = "x", kind = typography_kind::Body, tone = Some(tone::Danger.into()))
        text { "y" }
    }
}`;
    const at = (needle, plus = 0) => hsrc.indexOf(needle) + plus;
    const tagHover = hoverAt(cat, hsrc, at("Typography("), "Typography");
    check("hover: tag shows component docs and a props list",
        tagHover && tagHover.startsWith("**Typography** · component") && tagHover.includes("**Props**") && /- `kind`: `[^`]+`/.test(tagHover),
        tagHover && tagHover.slice(0, 120));
    const primHover = hoverAt(cat, hsrc, at("text {"), "text");
    check("hover: primitive tag", primHover && primHover.startsWith("**text** · primitive"), primHover && primHover.slice(0, 60));
    const propHover = hoverAt(cat, hsrc, at("kind = typography"), "kind");
    check("hover: prop name shows its type and doc",
        propHover && propHover.startsWith("**Typography.kind** · `") && propHover.length > 40, propHover && propHover.slice(0, 80));
    if ((raw.values || []).length) {
        const valHover = hoverAt(cat, hsrc, at("Body,"), "Body");
        check("hover: value shows docs and its import",
            valHover && valHover.startsWith("**typography_kind::Body** · value of `TypographyKindRef`") && valHover.includes("use idea_ui::typography_kind;"),
            valHover && valHover.slice(0, 100));
        const optVal = hoverAt(cat, hsrc, at("Danger.into"), "Danger");
        check("hover: value inside Some(…)", optVal && optVal.startsWith("**tone::Danger**"), optVal && optVal.slice(0, 60));
    }
    check("hover: a plain identifier in a handler is not ours", hoverAt(cat, hsrc, at("let kind", 4), "kind") === null);
    check("hover: a prop-value that is a local variable is not ours",
        hoverAt(cat, `ui! { Typography(content = label) }`, 27, "label") === null);
    check("hover: outside ui! nothing", hoverAt(cat, `fn f() { Typography(x) }`, 9, "Typography") === null);
}

// --- source scans merge over the compiled catalog ---
{
    const base = digest({
        primitives: [{ name: "view", props: [] }],
        components: [
            { name: "Old", module_path: "my_app::widgets", params: [] },
            { name: "Button", module_path: "idea_ui::components::button", params: [] },
        ],
        values: [{ short_name: "Hype", module_path: "my_app::theme", value_of: "ToneRef", via: "", spelled: "theme::Hype", import: "my_app::theme" }],
        types: [{ short_name: "Mode", module_path: "my_app", shape: { kind: "enum", variants: [{ name: "A", payload: [] }] } }],
    });
    const scan = {
        crate: "my_app",
        cat: digest({
            components: [{ name: "New", module_path: "my_app::widgets", params: [{ name: "label", type: "String" }] }],
            values: [{ short_name: "Calm", module_path: "my_app::theme", value_of: "ToneRef", via: "", spelled: "theme::Calm", import: "my_app::theme" }],
            types: [{ short_name: "Mode", module_path: "my_app", shape: { kind: "enum", variants: [{ name: "A", payload: [] }, { name: "B", payload: [] }] } }],
        }),
    };
    const merged = mergeScans(base, [scan]);
    const names = merged.tags.map((t) => t.name);
    check("mergeScans: the scanned crate's compiled components are replaced by the scan's",
        names.includes("New") && !names.includes("Old"), names.join());
    check("mergeScans: other crates and primitives are untouched", names.includes("Button") && names.includes("view"));
    check("mergeScans: scanned props are reachable", (merged.propsByTag.get("New") || []).map((p) => p.name).join() === "label");
    check("mergeScans: values of the crate are replaced", (merged.valuesByTarget.get("ToneRef") || []).map((v) => v.spelled).join() === "theme::Calm");
    check("mergeScans: enums of the crate are replaced", merged.enumsByName.get("Mode").variants.length === 2);
    check("mergeScans: tagsByName follows", merged.tagsByName.has("New") && !merged.tagsByName.has("Old"));
    check("mergeScans: no scans → base unchanged", mergeScans(base, []).tags.length === base.tags.length);
}

{
    const tmpd = fs.mkdtempSync(path.join(os.tmpdir(), "idealyst-deps-"));
    fs.writeFileSync(path.join(tmpd, "Cargo.toml"), `[package]
name = "app-main"

[dependencies]
runtime-core = { workspace = true }
crewforge-ui-shared = { workspace = true }
serde = "1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
video = { workspace = true }

[dev-dependencies]
insta = "1"

[features]
default = []
`);
    const deps = manifestDeps(tmpd);
    check("manifestDeps: dependency tables, dashes to underscores, features/dev excluded",
        deps.has("crewforge_ui_shared") && deps.has("video") && deps.has("serde") && !deps.has("insta") && !deps.has("default"),
        [...deps].join());
    fs.rmSync(tmpd, { recursive: true, force: true });
}

// --- authoring hints (signals, effects, … in #[component] bodies) ---
const body = `
use runtime_core::*;

/// Doc with a brace { in prose.
#[component]
fn Counter(start: i32) -> Element {
    let count = signal(start);
    let s = "a { string";
    let handler = Rc::new(move || {
        cou
    });
    ui! { text { "x" } }
}

impl Foo {
    fn bar(&self) { sig }
}

mod inner {
    // item level again
    xyz
}
`;
check("rustContext: inside a fn body", rustContext(body, body.indexOf("let count")) === "fn");
check("rustContext: inside a closure inside a fn body", rustContext(body, body.indexOf("cou\n") + 3) === "fn");
check("rustContext: inside an impl method", rustContext(body, body.indexOf("sig }") + 3) === "fn");
check("rustContext: module root is item level", rustContext(body, body.indexOf("#[component]")) === "item");
check("rustContext: inside an inline mod with no fn is item level", rustContext(body, body.indexOf("xyz")) === "item");
check("rustContext: braces in strings and comments don't count", rustContext(body, body.length - 1) === "item");

if ((raw.macros || []).some((m) => m.snippet)) {
    const fnItems = authoringItems(cat, "fn").map((i) => i.label);
    const itemItems = authoringItems(cat, "item").map((i) => i.label);
    check("authoring: fn body offers the reactive vocabulary",
        ["signal", "effect!", "memo", "spawn_then", "rx!", "watch", "on_scope_drop", "after_ms_scoped"].every((l) => fnItems.includes(l)),
        fnItems.join());
    check("authoring: fn body does not offer item skeletons", !fnItems.includes("#[component]") && !fnItems.includes("#[props]"));
    check("authoring: item level offers the skeletons only",
        itemItems.includes("#[component]") && itemItems.includes("#[props]") && itemItems.includes("stylesheet!") && !itemItems.includes("signal"),
        itemItems.join());
    const sig = authoringItems(cat, "fn").find((i) => i.label === "signal");
    check("authoring: inserts the catalog snippet", sig && sig.insertText && sig.insertText.value === "let ${1:name} = signal(${2:value});",
        sig && JSON.stringify(sig.insertText));
    // REGRESSION: `let name = sig│` accepted the full snippet and produced
    // `let name = let name = signal(value)`. On an assignment's right-hand
    // side the snippet's own binding is dropped.
    const onRhs = authoringItems(cat, "fn", "    let count = ").find((i) => i.label === "signal");
    check("authoring: on a `let` RHS the snippet drops its own binding",
        onRhs && onRhs.insertText.value === "signal(${2:value});", onRhs && onRhs.insertText.value);
    const red = authoringItems(cat, "fn", "let (s, d) = ").find((i) => i.label === "reducer");
    check("authoring: reducer's tuple binding is dropped too",
        red && red.insertText.value.startsWith("reducer(${3:initial}"), red && red.insertText.value);
    const reassign = authoringItems(cat, "fn", "    total = ").find((i) => i.label === "memo");
    check("authoring: a plain reassignment counts as RHS", reassign && reassign.insertText.value === "memo(move || ${2:expr});");
    const bare = authoringItems(cat, "fn", "    ").find((i) => i.label === "signal");
    check("authoring: statement position keeps the binding", bare && bare.insertText.value.startsWith("let ${1:name} = signal("));
    check("onAssignmentRhs: `==` is a comparison, not an assignment", !onAssignmentRhs("if a == "));
    check("onAssignmentRhs: `=>` is an arrow", !onAssignmentRhs("Some(x) => "));
    check("onAssignmentRhs: `!=`/`<=`/`>=` are comparisons", !onAssignmentRhs("a != ") && !onAssignmentRhs("a <= ") && !onAssignmentRhs("a >= "));
    check("stripBinding: type ascription", stripBinding("let ${1:x}: Ref<H> = node_ref!();") === "node_ref!();");
    check("stripBinding: no binding is untouched", stripBinding("effect!({\n\t$0\n});") === "effect!({\n\t$0\n});");
    const comp = authoringItems(cat, "item").find((i) => i.label === "#[component]");
    check("authoring: attribute labels filter on the bare word", comp && comp.filterText === "component");
} else {
    check("authoring: empty when the dump predates snippets", authoringItems(cat, "fn").length === 0);
}

// --- project resolution (REGRESSION: workspace-shaped repos) ---
// The old gate read only the workspace root's Cargo.toml, so a monorepo
// whose apps live under crates/ never loaded a catalog — the extension
// was silently inert in every real project. Build a temp tree:
//
//   ws/Cargo.toml                 [workspace]
//   ws/crates/app/Cargo.toml      [package.metadata.idealyst]
//   ws/crates/lib/Cargo.toml      plain library member
//   ws/crates/app/nested/Cargo.toml  a plain (non-idealyst) inner crate
//   plain/Cargo.toml              unrelated non-idealyst package
//   single/Cargo.toml             a bare idealyst project (root IS the app)
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "idealyst-ext-"));
const mk = (rel, body) => {
    const full = path.join(tmp, rel);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, body);
    return full;
};
mk("ws/Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n');
mk("ws/crates/app/Cargo.toml", '[package]\nname = "app"\n[package.metadata.idealyst]\nbundle_id = "x"\n');
mk("ws/crates/lib/Cargo.toml", '[package]\nname = "lib"\n[dependencies]\nruntime-core = { workspace = true }\n');
mk("ws/crates/util/Cargo.toml", '[package]\nname = "util"\n[dependencies]\nserde = "1"\n');
mk("ws/crates/app/nested/Cargo.toml", '[package]\nname = "nested"\n');
mk("plain/Cargo.toml", '[package]\nname = "plain"\n');
mk("single/Cargo.toml", '[package]\nname = "single"\n[package.metadata.idealyst]\n');
const appFile = mk("ws/crates/app/src/lib.rs", "");
const libFile = mk("ws/crates/lib/src/lib.rs", "");
const nestedFile = mk("ws/crates/app/nested/src/lib.rs", "");
const utilFile = mk("ws/crates/util/src/lib.rs", "");
const plainFile = mk("plain/src/lib.rs", "");
const singleFile = mk("single/src/lib.rs", "");
const ws = path.join(tmp, "ws");

const appProj = projectFor(appFile, ws);
check("projectFor: workspace member app resolves to the member, exactly",
    appProj && appProj.dir === path.join(ws, "crates/app") && appProj.exact === true,
    JSON.stringify(appProj));
const libProj = projectFor(libFile, ws);
check("projectFor: plain lib member resolves to its own crate (the CLI picks a dependent app), inexact",
    libProj && libProj.dir === path.join(ws, "crates/lib") && libProj.exact === false,
    JSON.stringify(libProj));
const looseFile = mk("ws/tools/loose.rs", "");
check("projectFor: a file outside any crate is nobody's", projectFor(looseFile, ws) === null);
check("projectFor: a member with no framework dependency is not ours (no CLI spawn in foreign Rust)",
    projectFor(utilFile, ws) === null);
const nestedProj = projectFor(nestedFile, ws);
check("projectFor: non-idealyst inner crate walks up to the enclosing idealyst crate",
    nestedProj && nestedProj.dir === path.join(ws, "crates/app") && nestedProj.exact === true,
    JSON.stringify(nestedProj));
check("projectFor: unrelated non-idealyst package is not a project",
    projectFor(plainFile, path.join(tmp, "plain")) === null);
const singleProj = projectFor(singleFile, path.join(tmp, "single"));
check("projectFor: bare project root still resolves to itself",
    singleProj && singleProj.dir === path.join(tmp, "single") && singleProj.exact === true,
    JSON.stringify(singleProj));
// The walk must stop at the workspace folder, never climbing into
// whatever happens to sit above it.
mk("Cargo.toml", '[package]\nname = "above"\n[package.metadata.idealyst]\n');
check("projectFor: never climbs above the workspace folder",
    projectFor(plainFile, path.join(tmp, "plain")) === null);
fs.rmSync(tmp, { recursive: true, force: true });

// --- loadCatalog end to end, through a stub CLI ---
// REGRESSION: a refactor left two `loadCatalog` definitions in the file;
// the surviving one referenced a deleted helper and threw
// `ReferenceError` on every completion request — invisible to the
// pure-function checks above because nothing called the loader. Run it
// for real: `idealyst.cli` points at a script that prints the dump.
const stubDir = fs.mkdtempSync(path.join(os.tmpdir(), "idealyst-ext-cli-"));
const stub = path.join(stubDir, "idealyst");
fs.writeFileSync(
    stub,
    `#!/bin/sh
case "$1" in
  catalog-json) [ "$2" = "--deps-only" ] || exit 3; echo "stub: building $3" >&2; cat "${path.resolve(catalogPath)}" ;;
  catalog-scan) echo '{"scanned_crate":"stub_app","components":[{"name":"Fresh","module_path":"stub_app","params":[]}],"types":[],"values":[]}' ;;
  *) exit 2 ;;
esac
`
);
fs.chmodSync(stub, 0o755);
// --- cliPath: a committed laptop path must not break another machine ---
{
    const { cliPath } = __test;
    mock.__cli = "idealyst";
    check("cliPath: a PATH-style name is used as is", cliPath() === "idealyst");
    mock.__cli = process.execPath;
    check("cliPath: an existing path is used", cliPath() === process.execPath);
    mock.__cli = "/Users/someone/Desktop/idealyst-native/target/debug/idealyst";
    check(
        "regression cliPath: a configured path missing here falls back to PATH",
        cliPath() === "idealyst"
    );
    mock.__cli = "";
    check("cliPath: an empty setting means PATH", cliPath() === "idealyst");
}
mock.__cli = stub;
loadCatalog(stubDir);
scanCrate(stubDir);
(function waitForLoad(tries) {
    const loaded = catalogs.get(stubDir);
    if (loaded && loaded.tags.some((t) => t.name === "Fresh") && loaded.tags.length > 1) {
        check("loadCatalog: runs the CLI and caches the digested catalog",
            loaded.tags.length === cat.tags.length + 1);
        check("scanCrate: a scanned component is merged over the compiled catalog",
            loaded.tagsByName.has("Fresh") && loaded.tagsByName.has("view"));
        fs.rmSync(stubDir, { recursive: true, force: true });
        process.exit(failures ? 1 : 0);
    }
    if (tries === 0) {
        check("loadCatalog: runs the CLI and caches the digested catalog", false, "timed out");
        fs.rmSync(stubDir, { recursive: true, force: true });
        process.exit(1);
    }
    setTimeout(() => waitForLoad(tries - 1), 50);
})(100);
