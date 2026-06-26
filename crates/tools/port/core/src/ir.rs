//! Porter IR — a normalized representation of a React component,
//! sitting between the TSX frontend and the idealyst Rust emitter.
//!
//! The IR captures *just enough* structure to drive deterministic
//! lowering to `#[component]` + `signal!` + `jsx!`. Anything we
//! can't (or shouldn't) translate mechanically is kept around
//! verbatim as a [`SourceSnippet`] and emitted by the backend as a
//! [`todo!("port: …")`] hole with the original code inline.
//!
//! Design notes:
//!
//! - **Holes are first-class.** A successful port is one that
//!   compiles and lists every hole loudly — *not* one that silently
//!   guesses at semantics it doesn't understand. See [`Hole`].
//! - **No JS semantics live in the IR.** Hook calls are reduced to
//!   the framework primitive they map to ([`Reactive::State`] /
//!   [`Reactive::Effect`] / …). Custom hook calls become holes.
//! - **JSX is preserved structurally**, not semantically. We don't
//!   try to know which element names are idealyst primitives vs.
//!   user components — the emitter passes everything through to
//!   `jsx! { … }` and lets the framework's existing macro dispatch
//!   resolve it.

use std::fmt;

/// A verbatim slice of the original source, used both to seed
/// holes and to attribute diagnostics back to user code.
#[derive(Debug, Clone)]
pub struct SourceSnippet {
    pub text: String,
    /// 1-based line in the original file. `None` when the snippet
    /// was synthesized (e.g., a hole inserted by a lowering pass
    /// that has no single source span).
    pub line: Option<u32>,
}

impl SourceSnippet {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), line: None }
    }

    pub fn at(text: impl Into<String>, line: u32) -> Self {
        Self { text: text.into(), line: Some(line) }
    }
}

/// A spot the mechanical porter refused to translate. The emitter
/// turns this into a `todo!("port: <reason> — <snippet>")` call so
/// the resulting file still type-checks structurally while listing
/// every unresolved spot loudly.
///
/// The `kind` exists so a later pass can categorize / count / route
/// holes (e.g., "33 unknown hooks, 12 third-party calls").
#[derive(Debug, Clone)]
pub struct Hole {
    pub kind: HoleKind,
    pub reason: String,
    pub original: SourceSnippet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoleKind {
    /// A handler/effect body we can't translate (`axios.get(…)`,
    /// `lodash.debounce(…)`, dynamic property access, …).
    HandlerBody,
    /// A JSX attribute value we can't translate (spread props,
    /// dynamic key expression, …).
    AttributeValue,
    /// A prop type that doesn't map cleanly to a Rust type
    /// (union, intersection, function type, etc.).
    PropType,
    /// Catch-all for top-level syntax the IR doesn't model yet
    /// (class components, decorators, generators, exotic
    /// destructure patterns, ...).
    Unsupported,
}

impl fmt::Display for HoleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HoleKind::HandlerBody => "handler-body",
            HoleKind::AttributeValue => "attribute-value",
            HoleKind::PropType => "prop-type",
            HoleKind::Unsupported => "unsupported",
        })
    }
}

// =============================================================================
// Top-level
// =============================================================================

/// A single source file, post-lowering.
#[derive(Debug, Clone, Default)]
pub struct Module {
    /// Name of the porter tool that produced this module
    /// (e.g. `port-react`). Surfaces in the emitted header
    /// comment so generated files identify their provenance.
    pub source_tool: String,
    /// Bare-name idealyst items the emitter should `use` from
    /// `runtime_core` (always at least `signal`, `jsx`, `component`,
    /// `Element`). The lowering pass appends as it discovers usage.
    pub imports: Vec<String>,
    /// Every `#[component] fn …` in the file.
    pub components: Vec<Component>,
    /// Top-level non-component items the porter wants to forward
    /// verbatim — type aliases, const, etc., one snippet per item.
    pub passthroughs: Vec<SourceSnippet>,
    /// Every TS interface declared in this file, lowered to a
    /// `PropsType` (the same shape used for component prop
    /// structs). Not emitted directly — used by the project-level
    /// driver to build a cross-file registry so
    /// `createContext<ImportedT>(...)` aliases referencing types
    /// defined in *other* files can still produce a matching
    /// struct passthrough.
    pub local_interfaces: std::collections::HashMap<String, PropsType>,
    /// `createContext` aliases whose type argument wasn't an
    /// interface defined in this file. The project-level driver
    /// resolves these by consulting the global interface registry
    /// it built from all files' `local_interfaces`, then appends
    /// the corresponding struct to `passthroughs` before emit.
    /// Each entry is `(alias_name, referenced_type_name)`.
    pub unresolved_context_aliases: Vec<(String, String)>,
}

/// One React function-component, lowered.
#[derive(Debug, Clone)]
pub struct Component {
    /// Original PascalCase name. The emitter snake-cases it for
    /// the function and re-PascalCases it for the props struct.
    pub name: String,
    pub props: PropsType,
    pub body: ComponentBody,
}

/// The shape of the component's first parameter.
#[derive(Debug, Clone, Default)]
pub struct PropsType {
    pub fields: Vec<PropField>,
}

#[derive(Debug, Clone)]
pub struct PropField {
    pub name: String,
    /// Rust type expression as a string. The lowering pass is
    /// responsible for mapping TS types here; unmappable types
    /// land as a [`Hole`] in [`Component::body.preamble`].
    pub ty: String,
    pub optional: bool,
    /// `Some(expr)` when the React source declared a default via
    /// destructuring or default parameter (e.g.,
    /// `{ initial = 0 }`). Lowers to `#[component(default(...))]`.
    pub default: Option<String>,
}

/// The body of one component: reactive declarations, then the
/// returned JSX tree.
#[derive(Debug, Clone)]
pub struct ComponentBody {
    /// Declarations that run before the return — hook calls,
    /// derived signals, effect registrations, plus any holes the
    /// frontend decided belonged in preamble position.
    pub preamble: Vec<Reactive>,
    /// The single returned tree (or a hole if the component's
    /// return statement isn't translatable).
    pub returns: ReturnExpr,
}

#[derive(Debug, Clone)]
pub enum ReturnExpr {
    Jsx(JsxNode),
    Hole(Hole),
}

// =============================================================================
// Reactive primitives
// =============================================================================

/// One line of the component's preamble — either a recognized
/// reactive primitive or an explicit hole.
#[derive(Debug, Clone)]
pub enum Reactive {
    /// `let count = signal!(<init>);`
    /// Lowered from `const [count, setCount] = useState(init)`.
    /// We track `setter` so the JSX/handler walker can rewrite
    /// `setCount(x)` to `count.set(x)`.
    State {
        name: String,
        setter: String,
        init: String,
    },
    /// `effect!({ … });`
    /// Lowered from `useEffect(fn, deps)`. The body is kept as
    /// a snippet (typically a hole) because we can't translate
    /// arbitrary JS imperative code mechanically.
    Effect {
        body: SourceSnippet,
        /// `Some` when the React source supplied a dep array.
        /// idealyst signals are auto-tracking so this is purely
        /// informational; the emitter renders it as a comment so
        /// the AI / human reviewer can sanity-check.
        deps: Option<Vec<String>>,
    },
    /// `let foo = <expr>;` — a plain `let` binding. Used both
    /// for memoized derived values the frontend rewrote and for
    /// unknown function calls preserved as-is (`let result =
    /// useControls(spec);`). The porter does not invent a hooks
    /// runtime; custom hooks port as regular function calls and
    /// link against user-written or hand-ported Rust functions.
    Let { name: String, expr: String },
    /// `<expr>;` — a plain expression statement. Used for
    /// unknown function calls that appear at statement position
    /// (`useImperativeHandle(ref, …);`). Same philosophy as
    /// `Let`: preserve, don't invent runtime.
    Stmt(String),
    /// An explicit hole. Emitted as a top-of-body
    /// `todo!("port: …")`.
    Hole(Hole),
}

// =============================================================================
// JSX
// =============================================================================

/// A node in the returned JSX tree. Preserved structurally and
/// emitted into a `jsx! { … }` block — the framework macro handles
/// the actual primitive vs. user-component dispatch.
#[derive(Debug, Clone)]
pub enum JsxNode {
    /// `<Name attr=… >children</Name>` or `<Name … />`.
    Element {
        name: String,
        attrs: Vec<JsxAttr>,
        /// `None` => self-closing in the source.
        children: Option<Vec<JsxNode>>,
    },
    /// Plain text content. The emitter wraps in `<Text>"…"</Text>`
    /// since `jsx!` does not allow bare strings between tags.
    Text(String),
    /// `{expression}` between children — a braced child.
    Expr(String),
    /// A hole that took the place of a child node we couldn't
    /// translate.
    Hole(Hole),
}

#[derive(Debug, Clone)]
pub struct JsxAttr {
    pub name: String,
    pub value: JsxAttrValue,
}

#[derive(Debug, Clone)]
pub enum JsxAttrValue {
    /// `attr="literal"` — a bare string in `jsx!`.
    StringLit(String),
    /// `attr={expr}` — a braced Rust expression. The lowering pass
    /// is responsible for the JS→Rust translation; if it can't,
    /// it emits [`Self::Hole`] instead.
    Expr(String),
    Hole(Hole),
}

// =============================================================================
// Diagnostics
// =============================================================================

/// What the porter found while lowering a file. Returned alongside
/// the [`Module`] so the CLI can summarize without re-walking.
#[derive(Debug, Default)]
pub struct PortReport {
    pub holes: Vec<Hole>,
    /// Number of components the lifter actually emitted into the
    /// IR. Zero means "no exported components found" — for the
    /// project-level porter this is a Skipped, not an Error.
    pub component_count: usize,
}

impl PortReport {
    pub fn record(&mut self, hole: Hole) {
        self.holes.push(hole);
    }

    pub fn by_kind(&self, kind: HoleKind) -> usize {
        self.holes.iter().filter(|h| h.kind == kind).count()
    }
}
