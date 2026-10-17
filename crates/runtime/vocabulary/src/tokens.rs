//! Author-defined token sets: the machinery behind `#[derive(Tokens)]`.
//!
//! A **token** is a stable name with a type (`surface`, a `Color`). A
//! **theme** is one assignment of values to every token. Switching themes
//! writes new values under the same names, and everything that references
//! a token re-resolves. An app declares both with an ordinary struct:
//!
//! ```ignore
//! use runtime_core::{stylesheet, Color, Length, Tokens};
//!
//! #[derive(Tokens, Clone)]
//! pub struct Swatch { pub solid_bg: Color, pub soft_bg: Color }
//!
//! #[derive(Tokens, Clone)]
//! #[tokens(default = light)]
//! pub struct AppTheme {
//!     #[tokens(app_background)]
//!     pub background: Color,
//!     pub surface: Color,
//!     pub accent: Swatch,   // a nested token set is a namespace
//!     pub gap: Length,
//! }
//!
//! fn light() -> AppTheme { /* every value, once */ }
//! fn dark() -> AppTheme { light().with(|t| t.surface = "#111827".into()) }
//!
//! stylesheet! {
//!     pub Card<AppTheme> {
//!         base(t) { background: t.surface(), border_color: t.accent.soft_bg(), gap: t.gap() }
//!     }
//! }
//!
//! AppTheme::follow_system(light(), dark()); // inside `app()`
//! ```
//!
//! # Names
//!
//! A leaf's token name is its field path in kebab case: `surface` →
//! `surface`, `accent.soft_bg` → `accent-soft-bg` (and, with
//! `#[tokens(prefix = "app")]` on the theme, `app-accent-soft-bg`). The
//! names are `&'static str`s computed at compile time: the derive emits
//! each struct's field tree as a `const` [`TokenNode`] slice, and the
//! THEME (the root) flattens the whole tree into one name table by const
//! evaluation ([`__table`] / [`__split`]). The root is the only place the
//! full path of a leaf is known — a shape like `Swatch` may sit under
//! `accent` and `danger` at once — so the accessors of a nested shape are
//! generic over the root `R` and index `R::NAMES` at an offset fixed when
//! the root builds its accessor tree. Two paths that kebab to the same
//! name (`a_b` and `a.b`) fail that const evaluation, so a theme cannot
//! alias two tokens.
//!
//! # Fallbacks — the `default` theme supplies them
//!
//! Every `Tokenized::Token` carries a fallback. It is observed in exactly
//! three places, all *before or outside* an installed theme:
//!
//! 1. `Tokenized::resolve` before any theme is installed (it reads the
//!    registry, and an installed value always wins);
//! 2. the second argument of the web backend's `var(--name, fallback)`,
//!    which a `:root` definition shadows once tokens install — and the
//!    build-time (premint) CSS, which bakes the same `var()` with no theme
//!    in sight;
//! 3. the MCP catalog's displayed default, extracted from a binary that
//!    never runs the app.
//!
//! Case 3 rules out "take the fallback from whatever theme is installed"
//! (nothing is installed there), and case 2's premint bakes CSS whose
//! fallback must not depend on boot order. So a theme root names a
//! function returning its default instance — `#[tokens(default = light)]`
//! — and every fallback is read from that instance, cached once per
//! thread ([`__fallback`]). Values are written once, in the theme
//! functions; nothing restates them. This is the contract idea-theme's
//! vocabulary already lives by (its accessors read a thread-local
//! `light_theme()`). A struct without `default` is a **shape**: it can be
//! nested in a theme but cannot be installed or named as a stylesheet
//! vocabulary on its own — the compiler says so via [`TokenTheme`].
//!
//! # Install / swap
//!
//! The [`Tokens`] methods (`install`, `set`, `follow_system`,
//! `install_reactive`) flatten a theme into `TokenEntry`s and hand them to
//! the same `install_tokens` / `update_tokens` / `install_theme_palettes`
//! every theme goes through, so resolution, premint, the web `var()`
//! cascade and the native cohort re-apply are untouched. They must run
//! inside the mounted world (inside `app()`), like every theme install;
//! `set` is also safe from an event handler.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::{Color, ColorScheme, Length, ThemePalette, TokenEntry, TokenValue, Tokenized};

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Color {}
    impl Sealed for super::Length {}
    impl Sealed for f32 {}
}

/// One of the three value kinds the token registry stores — [`Color`],
/// [`Length`] and `f32` (a `TokenValue::Number`) — and therefore one of
/// the three `Tokenized<T>` a style property accepts. Sealed: a new kind
/// would need a registry variant and a backend lowering, not just an impl.
pub trait TokenPrimitive: Clone + 'static + sealed::Sealed {
    /// The catalog's name for the kind: `"Color"`, `"Length"`, `"Number"`.
    const KIND: &'static str;
    /// Wrap as the registry's value.
    fn into_token_value(self) -> TokenValue;
    /// Unwrap from the registry's value; `None` for another kind.
    fn from_token_value(value: &TokenValue) -> Option<Self>;
}

impl TokenPrimitive for Color {
    const KIND: &'static str = "Color";
    fn into_token_value(self) -> TokenValue {
        TokenValue::Color(self)
    }
    fn from_token_value(value: &TokenValue) -> Option<Self> {
        match value {
            TokenValue::Color(c) => Some(c.clone()),
            _ => None,
        }
    }
}

impl TokenPrimitive for Length {
    const KIND: &'static str = "Length";
    fn into_token_value(self) -> TokenValue {
        TokenValue::Length(self)
    }
    fn from_token_value(value: &TokenValue) -> Option<Self> {
        match value {
            TokenValue::Length(l) => Some(*l),
            _ => None,
        }
    }
}

impl TokenPrimitive for f32 {
    const KIND: &'static str = "Number";
    fn into_token_value(self) -> TokenValue {
        TokenValue::Number(self)
    }
    fn from_token_value(value: &TokenValue) -> Option<Self> {
        match value {
            TokenValue::Number(n) => Some(*n),
            _ => None,
        }
    }
}

/// A field type that IS one token. Implemented for [`Color`], [`Length`]
/// and `f32`; implement it for your own newtype to give a token a
/// domain type while it installs as one of the registry's kinds:
///
/// ```ignore
/// #[derive(Clone)]
/// pub struct Brand(pub Color);
/// impl TokenLeaf for Brand {
///     type Value = Color;
///     fn token_value(&self) -> Color { self.0.clone() }
/// }
/// ```
///
/// The accessor for such a field returns `Tokenized<Self::Value>`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a token value",
    label = "not a `TokenLeaf`",
    note = "a leaf token is a `Color`, a `Length`, an `f32`, or a type implementing `TokenLeaf`; a nested namespace is another `#[derive(Tokens)]` struct; a field that is not a token takes `#[tokens(skip)]`"
)]
pub trait TokenLeaf {
    /// The registry kind this leaf installs as.
    type Value: TokenPrimitive;
    /// This leaf's value.
    fn token_value(&self) -> Self::Value;
}

impl TokenLeaf for Color {
    type Value = Color;
    fn token_value(&self) -> Color {
        self.clone()
    }
}

impl TokenLeaf for Length {
    type Value = Length;
    fn token_value(&self) -> Length {
        *self
    }
}

impl TokenLeaf for f32 {
    type Value = f32;
    fn token_value(&self) -> f32 {
        *self
    }
}

/// One field of a token set, as compile-time data. The derive emits one
/// per field; the theme root walks the tree to build its name table.
#[derive(Debug, Clone, Copy)]
pub struct TokenNode {
    /// The field's token-name segment, kebab case (`soft-bg`).
    pub name: &'static str,
    /// The field's accessor segment, as written (`soft_bg`).
    pub path: &'static str,
    /// `Some(kind)` for a leaf (`TokenPrimitive::KIND`), `None` for a
    /// nested set.
    pub kind: Option<&'static str>,
    /// A nested set's fields; empty for a leaf.
    pub children: &'static [TokenNode],
}

/// What a leaf field holds in an accessor struct. A leaf is read by
/// calling its accessor — `t.surface()` — so this slot exists only
/// because an accessor's fields and methods share names (a nested set is
/// reached through the field: `t.accent.soft_bg()`).
#[derive(Default, Clone, Copy, Debug)]
pub struct LeafSlot;

/// A type that can be a field of a `#[derive(Tokens)]` struct: a
/// [`TokenLeaf`] (one token) or another derived struct (a namespace).
///
/// Implemented by the derive and, for every `TokenLeaf`, by the blanket
/// impl below; authors implement `TokenLeaf` or derive, never this.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a field of a `#[derive(Tokens)]` struct",
    label = "neither a token value nor a token set",
    note = "a leaf token is a `Color`, a `Length`, an `f32`, or a type implementing `TokenLeaf`; a nested namespace is another `#[derive(Tokens)]` struct; a field that is not a token takes `#[tokens(skip)]`"
)]
pub trait TokenField: 'static {
    /// How many tokens this field contributes (1 for a leaf).
    const LEAF_COUNT: usize;
    /// `Some(kind)` for a leaf, `None` for a set.
    const KIND: Option<&'static str>;
    /// A set's fields; empty for a leaf.
    const NODES: &'static [TokenNode];
    /// What the field's accessor METHOD returns under root `R`: the
    /// `Tokenized` reference for a leaf, the nested accessors for a set.
    type Read<R: TokenTheme>;
    /// What the field's accessor FIELD holds under root `R`: [`LeafSlot`]
    /// for a leaf, the nested accessors for a set.
    type Slot<R: TokenTheme>: Copy;
    /// The accessor-method value for the token(s) starting at `index` in
    /// `R`'s name table.
    fn read<R: TokenTheme>(index: usize) -> Self::Read<R>;
    /// The accessor-field value for the token(s) starting at `index`.
    fn slot<R: TokenTheme>(index: usize) -> Self::Slot<R>;
    /// Append this field's values, in name-table order.
    fn collect_values(&self, out: &mut Vec<TokenValue>);
}

impl<T: TokenLeaf + 'static> TokenField for T {
    const LEAF_COUNT: usize = 1;
    const KIND: Option<&'static str> = Some(<T::Value as TokenPrimitive>::KIND);
    const NODES: &'static [TokenNode] = &[];
    type Read<R: TokenTheme> = Tokenized<T::Value>;
    type Slot<R: TokenTheme> = LeafSlot;

    fn read<R: TokenTheme>(index: usize) -> Tokenized<T::Value> {
        Tokenized::token(R::NAMES[index], __fallback::<R, T::Value>(index))
    }

    fn slot<R: TokenTheme>(_index: usize) -> LeafSlot {
        LeafSlot
    }

    fn collect_values(&self, out: &mut Vec<TokenValue>) {
        out.push(self.token_value().into_token_value());
    }
}

/// A `#[derive(Tokens)]` struct — a token set. The derive implements it;
/// importing `runtime_core::Tokens` brings the derive and these methods
/// into scope together.
///
/// The install family is available on a THEME (a set with
/// `#[tokens(default = …)]`, see [`TokenTheme`]).
pub trait Tokens: TokenField + Clone {
    /// A modified copy: `light().with(|t| t.surface = "#111827".into())`.
    /// The way to state a variant theme as "the base, except …".
    fn with(&self, edit: impl FnOnce(&mut Self)) -> Self {
        let mut copy = self.clone();
        edit(&mut copy);
        copy
    }

    /// The theme as registry entries — name and value per token, in
    /// declaration order. What `install` / `set` publish.
    fn token_entries(&self) -> Vec<TokenEntry>
    where
        Self: TokenTheme,
    {
        let mut values = Vec::with_capacity(Self::LEAF_COUNT);
        self.collect_values(&mut values);
        debug_assert_eq!(values.len(), Self::NAMES.len(), "derive emitted mismatched name/value tables");
        Self::NAMES
            .iter()
            .zip(values)
            .map(|(name, value)| TokenEntry { name, value })
            .collect()
    }

    /// The token accessors outside a `stylesheet!` block — for a computed
    /// layer or a component that resolves a token itself:
    /// `AppTheme::tokens().accent.solid_bg().resolve()`.
    fn tokens() -> Self::Slot<Self>
    where
        Self: TokenTheme,
    {
        Self::slot::<Self>(0)
    }

    /// Describe this theme as one switchable palette for a static render
    /// (see `install_theme_palettes`) without installing it.
    fn palette(&self, name: &'static str, scheme: Option<ColorScheme>) -> ThemePalette
    where
        Self: TokenTheme,
    {
        ThemePalette { name, scheme, tokens: self.token_entries() }
    }

    /// Install `theme` as the active theme. Once, inside `app()`, before
    /// anything renders.
    fn install(theme: Self)
    where
        Self: TokenTheme,
    {
        let entries = theme.token_entries();
        crate::glue::install_tokens(&entries);
        publish_app_background::<Self>(&entries);
    }

    /// Swap the active theme: same names, new values. Everything that
    /// references a token re-resolves. Safe from an event handler.
    fn set(theme: Self)
    where
        Self: TokenTheme,
    {
        let entries = theme.token_entries();
        crate::glue::update_tokens(&entries);
        publish_app_background::<Self>(&entries);
    }

    /// Install whichever of `light` / `dark` matches the platform's
    /// reported color scheme, and declare BOTH as palettes (`"light"`,
    /// `"dark"`) so a static render paints the right one for each reader
    /// with no script. The choice is a one-shot read of the preference at
    /// mount (`color_scheme()` is not a live subscription); for a manual
    /// toggle, call [`set`](Self::set) or use
    /// [`install_reactive`](Self::install_reactive).
    fn follow_system(light: Self, dark: Self)
    where
        Self: TokenTheme,
    {
        let prefers_dark = matches!(runtime_shared::color_scheme(), ColorScheme::Dark);
        Self::install(if prefers_dark { dark.clone() } else { light.clone() });
        crate::glue::install_theme_palettes(&[
            light.palette("light", Some(ColorScheme::Light)),
            dark.palette("dark", Some(ColorScheme::Dark)),
        ]);
    }

    /// Install a theme chosen by `select`, re-run whenever a signal it
    /// reads changes: the first run installs, later runs [`set`](Self::set).
    ///
    /// ```ignore
    /// let dark = signal(false);
    /// AppTheme::install_reactive(move || if dark.get() { dark_theme() } else { light() });
    /// ```
    ///
    /// One subscription per theme type: calling it again replaces (and
    /// disposes) the previous one.
    fn install_reactive<F>(select: F)
    where
        Self: TokenTheme,
        F: FnMut() -> Self + 'static,
    {
        install_reactive_impl::<Self, F>(select);
    }
}

/// A token set that can be installed: the struct carries
/// `#[tokens(default = path::to::fn)]`. The derive implements it, along
/// with `TokenVocabulary` so `stylesheet! { X<Theme> { … } }` binds the
/// theme's accessors.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is a token shape, not a theme: it names no default values",
    label = "no `#[tokens(default = …)]`",
    note = "add `#[tokens(default = path::to::fn)]` to the struct (a fn returning the theme whose values are the fallbacks), or install the theme struct that contains this one"
)]
pub trait TokenTheme: Tokens {
    /// Every token name, in declaration order (kebab-case field paths).
    const NAMES: &'static [&'static str];
    /// Every accessor path, same order (`accent.soft_bg`).
    const PATHS: &'static [&'static str];
    /// The index of the `#[tokens(app_background)]` leaf, if any.
    const APP_BACKGROUND: Option<usize>;
    /// The theme type's name, as the catalog lists the vocabulary.
    const NAME: &'static str;
    /// The theme every fallback is read from.
    fn default_theme() -> Self;
}

// ---------------------------------------------------------------------------
// Runtime support
// ---------------------------------------------------------------------------

runtime_tls::thread_local! {
    /// Each theme type's default values, flattened in name-table order —
    /// built from `default_theme()` on the first accessor call and kept
    /// for the thread (the values never change; the INSTALLED values live
    /// in the registry). One map, not one static per theme: a generic fn
    /// cannot own a per-instantiation static.
    static DEFAULT_VALUES: RefCell<HashMap<TypeId, Rc<[TokenValue]>>> = RefCell::new(HashMap::new());

    /// The `install_reactive` subscription per theme type. Replacing one
    /// drops the previous `Subscription`, disposing its effect.
    static REACTIVE_KEEPALIVE: RefCell<HashMap<TypeId, crate::glue::Subscription>> =
        RefCell::new(HashMap::new());
}

fn default_values<R: TokenTheme>() -> Rc<[TokenValue]> {
    let key = TypeId::of::<R>();
    if let Some(values) = DEFAULT_VALUES.with(|m| m.borrow().get(&key).cloned()) {
        return values;
    }
    // Built outside the borrow: `default_theme` is author code.
    let mut values = Vec::with_capacity(R::LEAF_COUNT);
    R::default_theme().collect_values(&mut values);
    let values: Rc<[TokenValue]> = values.into();
    DEFAULT_VALUES.with(|m| m.borrow_mut().insert(key, values.clone()));
    values
}

/// The fallback for token `index` of theme `R`: its value in
/// `R::default_theme()`. See the module docs for why the default theme,
/// not the installed one.
#[doc(hidden)]
pub fn __fallback<R: TokenTheme, V: TokenPrimitive>(index: usize) -> V {
    let values = default_values::<R>();
    V::from_token_value(&values[index]).unwrap_or_else(|| {
        panic!(
            "token `{}` of `{}`: the default theme holds a {:?}, the accessor reads a {}",
            R::NAMES[index],
            R::NAME,
            values[index],
            V::KIND
        )
    })
}

/// Route the `#[tokens(app_background)]` token to the host surface (body,
/// window, canvas) on every install and swap — the same channel
/// idea-theme feeds from `color-background`. Native backends apply the
/// resolved value directly, so a swap must re-publish it.
fn publish_app_background<R: TokenTheme>(entries: &[TokenEntry]) {
    let Some(index) = R::APP_BACKGROUND else { return };
    if let Some(TokenEntry { name, value: TokenValue::Color(color) }) = entries.get(index) {
        crate::glue::set_app_background(Tokenized::token(name, color.clone()));
    }
}

fn install_reactive_impl<R, F>(select: F)
where
    R: TokenTheme,
    F: FnMut() -> R + 'static,
{
    let mut select = select;
    let mut primed = false;
    let sub = crate::glue::watch(move || {
        let theme = select();
        // Untracked: only what `select` reads should re-run this. The
        // install path touches theme state (the version signal, context)
        // that must not become a dependency of the selector.
        runtime_world::untrack(|| {
            if primed {
                R::set(theme);
            } else {
                primed = true;
                R::install(theme);
            }
        });
    });
    let previous = REACTIVE_KEEPALIVE.with(|k| k.borrow_mut().insert(TypeId::of::<R>(), sub));
    // Dropped outside the borrow: disposing the old effect may re-enter.
    drop(previous);
}

// ---------------------------------------------------------------------------
// Compile-time name tables
//
// `const fn`s the derive's emitted `const`s call. Concatenating strings at
// compile time needs a byte array whose length is itself a constant, so a
// theme's table is built in three steps, each a `const` item on the
// concrete theme type: the total length (`__table_len`), the bytes plus
// each name's end offset (`__table`), and the `&'static str` slices
// (`__split`).
// ---------------------------------------------------------------------------

/// Longest field path (in bytes) a name may have.
const MAX_PATH: usize = 512;

const fn segment(node: &TokenNode, path: bool) -> &'static str {
    if path {
        node.path
    } else {
        node.name
    }
}

const fn walk_len(nodes: &[TokenNode], parent_len: usize, path: bool) -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        let seg = segment(node, path).len();
        let len = if parent_len == 0 { seg } else { parent_len + 1 + seg };
        total += if node.kind.is_some() { len } else { walk_len(node.children, len, path) };
        i += 1;
    }
    total
}

/// Total bytes of every leaf's full name (`path = false`, kebab, joined
/// with `-`, after `prefix`) or accessor path (`path = true`, joined with
/// `.`; `prefix` ignored).
#[doc(hidden)]
pub const fn __table_len(nodes: &[TokenNode], prefix: &str, path: bool) -> usize {
    let parent = if path { 0 } else { prefix.len() };
    walk_len(nodes, parent, path)
}

struct Cursor<'a> {
    out: &'a mut [u8],
    pos: usize,
    ends: &'a mut [usize],
    count: usize,
}

const fn write_nodes(
    nodes: &[TokenNode],
    parent: &mut [u8; MAX_PATH],
    parent_len: usize,
    path: bool,
    cur: &mut Cursor<'_>,
) {
    let sep = if path { b'.' } else { b'-' };
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        let seg = segment(node, path).as_bytes();
        let mut len = parent_len;
        if len > 0 {
            if len >= MAX_PATH {
                panic!("a token path is longer than 512 bytes");
            }
            parent[len] = sep;
            len += 1;
        }
        let mut j = 0;
        while j < seg.len() {
            if len >= MAX_PATH {
                panic!("a token path is longer than 512 bytes");
            }
            parent[len] = seg[j];
            len += 1;
            j += 1;
        }
        if node.kind.is_some() {
            let mut k = 0;
            while k < len {
                cur.out[cur.pos] = parent[k];
                cur.pos += 1;
                k += 1;
            }
            cur.ends[cur.count] = cur.pos;
            cur.count += 1;
        } else {
            write_nodes(node.children, parent, len, path, cur);
        }
        i += 1;
    }
}

/// Every leaf's name (or path) concatenated, plus where each one ends.
/// `L` must be [`__table_len`] and `N` the root's `LEAF_COUNT`.
#[doc(hidden)]
pub const fn __table<const L: usize, const N: usize>(
    nodes: &[TokenNode],
    prefix: &str,
    path: bool,
) -> ([u8; L], [usize; N]) {
    let mut out = [0u8; L];
    let mut ends = [0usize; N];
    let mut parent = [0u8; MAX_PATH];
    let mut parent_len = 0;
    if !path {
        let p = prefix.as_bytes();
        if p.len() > MAX_PATH {
            panic!("a token prefix is longer than 512 bytes");
        }
        while parent_len < p.len() {
            parent[parent_len] = p[parent_len];
            parent_len += 1;
        }
    }
    let mut cur = Cursor { out: &mut out, pos: 0, ends: &mut ends, count: 0 };
    write_nodes(nodes, &mut parent, parent_len, path, &mut cur);
    if cur.pos != L || cur.count != N {
        panic!("token name table: length or count disagrees with the tree");
    }
    (out, ends)
}

/// Slice the concatenated table into one `&'static str` per token, and
/// reject a theme whose names collide.
#[doc(hidden)]
pub const fn __split<const N: usize>(bytes: &'static [u8], ends: &[usize; N]) -> [&'static str; N] {
    let mut out = [""; N];
    let mut start = 0;
    let mut i = 0;
    while i < N {
        let (_, rest) = bytes.split_at(start);
        let (name, _) = rest.split_at(ends[i] - start);
        out[i] = match core::str::from_utf8(name) {
            Ok(s) => s,
            Err(_) => panic!("token name is not UTF-8"),
        };
        start = ends[i];
        i += 1;
    }
    out
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Fail const evaluation when two leaves kebab to one name (`a_b` and
/// `a.b` are both `a-b`): they would install as ONE token, so the theme
/// could never give them different values.
#[doc(hidden)]
pub const fn __assert_unique(names: &[&str]) {
    let mut i = 0;
    while i < names.len() {
        let mut j = i + 1;
        while j < names.len() {
            if str_eq(names[i], names[j]) {
                panic!("two fields of this token set produce the same token name (e.g. `a_b` and `a.b` are both `a-b`); rename one");
            }
            j += 1;
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// Render a token value as the catalog displays a default (`#fff`,
/// `12px`, `0.5`).
fn display_value(value: &TokenValue) -> String {
    match value {
        TokenValue::Color(c) => c.0.clone(),
        TokenValue::Length(Length::Px(v)) => format!("{v}px"),
        TokenValue::Length(Length::Percent(v)) => format!("{v}%"),
        TokenValue::Length(Length::Auto) => "auto".to_string(),
        TokenValue::Length(Length::Full) => "full".to_string(),
        TokenValue::Number(n) => format!("{n}"),
    }
}

fn kind_of(value: &TokenValue) -> &'static str {
    match value {
        TokenValue::Color(_) => <Color as TokenPrimitive>::KIND,
        TokenValue::Length(_) => <Length as TokenPrimitive>::KIND,
        TokenValue::Number(_) => <f32 as TokenPrimitive>::KIND,
    }
}

/// One catalog row per token of `R`: name, accessor path, first path
/// segment, kind, and the default theme's value. Ungated so the shape of
/// the rows is testable without the catalog; [`__catalog_tokens`] maps
/// them onto the catalog's entry type.
#[doc(hidden)]
pub fn __catalog_rows<R: TokenTheme>() -> Vec<CatalogRow> {
    let values = default_values::<R>();
    R::NAMES
        .iter()
        .zip(R::PATHS)
        .zip(values.iter())
        .map(|((name, path), value)| CatalogRow {
            name,
            path,
            namespace: path.split('.').next().unwrap_or(path),
            kind: kind_of(value),
            default_value: display_value(value),
            vocabulary: R::NAME,
        })
        .collect()
}

/// A token as the catalog lists it — see [`__catalog_rows`].
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogRow {
    pub name: &'static str,
    pub path: &'static str,
    pub namespace: &'static str,
    pub kind: &'static str,
    pub default_value: String,
    pub vocabulary: &'static str,
}

/// The catalog entries of theme `R`, for `register_style_token_set!`.
#[cfg(feature = "catalog")]
#[doc(hidden)]
pub fn __catalog_tokens<R: TokenTheme>() -> Vec<mcp_catalog::StyleTokenEntry> {
    __catalog_rows::<R>()
        .into_iter()
        .map(|row| mcp_catalog::StyleTokenEntry {
            name: row.name,
            path: row.path,
            namespace: row.namespace,
            value_type: row.kind,
            default_value: mcp_catalog::TokenDefault::Value(Box::leak(row.default_value.into_boxed_str())),
            vocabulary: row.vocabulary,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAF_COLOR: TokenNode = TokenNode { name: "", path: "", kind: Some("Color"), children: &[] };

    const fn leaf(name: &'static str, path: &'static str) -> TokenNode {
        TokenNode { name, path, ..LEAF_COLOR }
    }

    const SWATCH: &[TokenNode] = &[leaf("solid-bg", "solid_bg"), leaf("soft-bg", "soft_bg")];
    const TREE: &[TokenNode] = &[
        leaf("surface", "surface"),
        TokenNode { name: "accent", path: "accent", kind: None, children: SWATCH },
        TokenNode { name: "danger", path: "danger", kind: None, children: SWATCH },
    ];

    const NAME_LEN: usize = __table_len(TREE, "", false);
    const NAME_TABLE: ([u8; NAME_LEN], [usize; 5]) = __table(TREE, "", false);
    const NAMES: [&str; 5] = __split(&NAME_TABLE.0, &NAME_TABLE.1);

    const PATH_LEN: usize = __table_len(TREE, "app", true);
    const PATH_TABLE: ([u8; PATH_LEN], [usize; 5]) = __table(TREE, "app", true);
    const PATHS: [&str; 5] = __split(&PATH_TABLE.0, &PATH_TABLE.1);

    const PREFIXED_LEN: usize = __table_len(TREE, "app", false);
    const PREFIXED_TABLE: ([u8; PREFIXED_LEN], [usize; 5]) = __table(TREE, "app", false);
    const PREFIXED: [&str; 5] = __split(&PREFIXED_TABLE.0, &PREFIXED_TABLE.1);

    /// A shape reused under two prefixes yields two disjoint name sets,
    /// both computed at compile time.
    #[test]
    fn names_are_kebab_field_paths_and_a_reused_shape_takes_each_prefix() {
        assert_eq!(
            NAMES,
            ["surface", "accent-solid-bg", "accent-soft-bg", "danger-solid-bg", "danger-soft-bg"]
        );
    }

    #[test]
    fn paths_keep_the_accessor_spelling_and_ignore_the_prefix() {
        assert_eq!(PATHS, ["surface", "accent.solid_bg", "accent.soft_bg", "danger.solid_bg", "danger.soft_bg"]);
    }

    #[test]
    fn a_prefix_leads_every_name() {
        assert_eq!(PREFIXED[0], "app-surface");
        assert_eq!(PREFIXED[4], "app-danger-soft-bg");
    }

    #[test]
    fn unique_names_pass_the_collision_check() {
        __assert_unique(&NAMES);
    }

    #[test]
    #[should_panic(expected = "same token name")]
    fn colliding_names_are_rejected() {
        __assert_unique(&["a-b", "c", "a-b"]);
    }

    #[test]
    fn primitives_round_trip_through_the_registry_value() {
        let c = Color("#123456".into());
        assert_eq!(Color::from_token_value(&c.clone().into_token_value()), Some(c));
        assert_eq!(Length::from_token_value(&Length::Px(4.0).into_token_value()), Some(Length::Px(4.0)));
        assert_eq!(f32::from_token_value(&0.5f32.into_token_value()), Some(0.5));
        assert_eq!(f32::from_token_value(&TokenValue::Length(Length::Auto)), None);
    }

    #[test]
    fn catalog_display_matches_the_idea_theme_spelling() {
        assert_eq!(display_value(&TokenValue::Length(Length::Px(12.0))), "12px");
        assert_eq!(display_value(&TokenValue::Color(Color("#fff".into()))), "#fff");
        assert_eq!(display_value(&TokenValue::Number(0.5)), "0.5");
    }
}
