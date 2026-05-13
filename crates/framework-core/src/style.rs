//! Style declarations and theme infrastructure.
//!
//! The framework owns the data model — what a "style" looks like, what
//! variant axes exist, how the active theme propagates — but does **not**
//! own the rendering strategy. Each backend interprets a `StyleRules`
//! value however suits its platform:
//!
//! - **Web** can lazily mint CSS classes per unique rule set and swap
//!   `className` on the node when the style changes.
//! - **iOS** can update `CALayer` / `UIView` properties directly.
//! - **Android** can call `View` setters or apply theme attributes.
//!
//! # Themes
//!
//! Stylesheets are **closures** from the active theme to concrete
//! `StyleRules`. The stylesheet's `base(|theme: &MyTheme| StyleRules { ... })`
//! takes a typed reference to the app's theme and returns a rule set
//! with concrete values. There is no token enum, no per-property
//! indirection — just a function from theme to style.
//!
//! Theme changes flow through the existing reactive system: each styled
//! node's apply-style call lives inside an `Effect` that reads the
//! theme signal, so swapping the theme re-fires every styled effect
//! and re-applies with the new theme's values. No re-render.
//!
//! # Identity for caching
//!
//! The framework memoizes resolution per `(stylesheet pointer, variants,
//! theme pointer)` and returns an `Rc<StyleRules>`. Backends cache
//! their native form keyed on the rule set's content (a hash or
//! serialization), making caching immune to allocator-reuse hazards.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

// ----------------------------------------------------------------------------
// Values
// ----------------------------------------------------------------------------

/// Color value as a backend-portable string. Backends translate to their
/// native form (CSS string, UIColor, Android color int).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub String);

impl From<&str> for Color {
    fn from(s: &str) -> Self {
        Color(s.to_string())
    }
}

impl From<String> for Color {
    fn from(s: String) -> Self {
        Color(s)
    }
}

/// Length in logical pixels. Backends scale appropriately.
pub type Length = f32;

/// A border specification — width plus color. Backends translate to
/// their native form (CSS `border: 2px solid #abc`, iOS layer border,
/// Android stroke).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Border {
    pub width: u32, // u32 instead of f32 so Border is Hash/Eq
    pub color: Color,
}

impl Border {
    pub fn new(width: u32, color: impl Into<Color>) -> Self {
        Self { width, color: color.into() }
    }
}

// ----------------------------------------------------------------------------
// StyleRules — concrete property bag
// ----------------------------------------------------------------------------

/// A bag of style property values. Every field is optional so a rule set
/// only carries properties the author cared about. Values are concrete —
/// no tokens, no indirection. Stylesheets produce these by running their
/// theme-fed closure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleRules {
    pub background: Option<Color>,
    pub color: Option<Color>,
    pub padding: Option<Length>,
    pub font_size: Option<Length>,
    pub border_radius: Option<Length>,
    pub border: Option<Border>,
}

impl StyleRules {
    /// Layer `other` on top of `self`: properties set in `other` override
    /// the corresponding fields in `self`.
    pub fn merge(mut self, other: &StyleRules) -> Self {
        if other.background.is_some() {
            self.background = other.background.clone();
        }
        if other.color.is_some() {
            self.color = other.color.clone();
        }
        if other.padding.is_some() {
            self.padding = other.padding;
        }
        if other.font_size.is_some() {
            self.font_size = other.font_size;
        }
        if other.border_radius.is_some() {
            self.border_radius = other.border_radius;
        }
        if other.border.is_some() {
            self.border = other.border.clone();
        }
        self
    }

    /// Stable content key suitable for backend caches that should be
    /// immune to allocator-reuse hazards. `f32` is bit-cast to `u32`
    /// for hashing (NaN style values are not expected).
    pub fn content_key(&self) -> String {
        // Manual concat to avoid the `format!` monomorphization. The key
        // is opaque (only used as a hash map key + hashed into a class
        // name), so its exact spelling doesn't matter — only that it
        // distinguishes different content. We use the same labeled
        // layout as before for debuggability.
        let bg = self.background.as_ref().map(|c| c.0.as_str()).unwrap_or("");
        let fg = self.color.as_ref().map(|c| c.0.as_str()).unwrap_or("");
        let p = self.padding.map(|n| n.to_bits()).unwrap_or(0);
        let fs = self.font_size.map(|n| n.to_bits()).unwrap_or(0);
        let br = self.border_radius.map(|n| n.to_bits()).unwrap_or(0);
        let (bw, bc) = match &self.border {
            Some(b) => (b.width, b.color.0.as_str()),
            None => (0, ""),
        };
        let mut s = String::with_capacity(64);
        s.push_str("bg=");
        s.push_str(bg);
        s.push_str(";fg=");
        s.push_str(fg);
        s.push_str(";p=");
        push_u32_hex(&mut s, p);
        s.push_str(";fs=");
        push_u32_hex(&mut s, fs);
        s.push_str(";br=");
        push_u32_hex(&mut s, br);
        s.push_str(";bw=");
        push_u32_hex(&mut s, bw);
        s.push_str(";bc=");
        s.push_str(bc);
        s
    }
}

/// Writes the 8-char lowercase hex representation of `n` to `out`.
/// Used by `content_key` to encode `f32::to_bits()` results without
/// the `format!` machinery.
fn push_u32_hex(out: &mut String, n: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..8).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        out.push(HEX[nibble] as char);
    }
}

// ----------------------------------------------------------------------------
// StyleSheet — closures from theme to rules, with variants and compounds
// ----------------------------------------------------------------------------

type RulesFn = Box<dyn Fn(&dyn Any) -> StyleRules>;

pub type VariantAxis = String;
pub type VariantValue = String;

/// One axis of variants on a stylesheet — its declared values and the
/// optional default value used when the call site doesn't pick a value.
pub struct VariantAxisDef {
    /// The value treated as active when the call site omits this axis.
    pub default: Option<VariantValue>,
    /// Per-value overlay closures. Each runs against the theme.
    pub values: BTreeMap<VariantValue, RulesFn>,
}

/// A compound variant: only applied when *all* of `when`'s
/// axis=value pairs are active at apply time.
pub struct CompoundVariant {
    pub when: BTreeMap<VariantAxis, VariantValue>,
    pub rules: RulesFn,
}

/// A stylesheet declaration. Authors construct one of these once and
/// wrap it in `Rc` to pass around.
///
/// Each entry — `base`, every variant overlay, every compound variant —
/// is a closure that takes the active theme (typed as the app's theme)
/// and returns concrete `StyleRules`. There are no tokens; closures
/// are the only mechanism for theme-aware values.
///
/// # Resolution order
/// 1. `base`
/// 2. For each declared axis, layer the closure for the value selected
///    in the `VariantSet` (or the axis's default if unselected).
/// 3. For each declared compound variant, layer its closure iff every
///    `(axis, value)` in `when` matches the *effective* variant set
///    (defaults included).
/// 4. Any `StyleApplication::overrides` field.
pub struct StyleSheet {
    base: RulesFn,
    /// axis → axis definition (default + per-value closures)
    variants: BTreeMap<VariantAxis, VariantAxisDef>,
    /// Compound variants are stored as a list (order-preserving).
    compounds: Vec<CompoundVariant>,
}

impl StyleSheet {
    /// Constructs a stylesheet whose base rules are produced by `f`.
    pub fn new<Theme, F>(f: F) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        Self {
            base: wrap_theme_fn::<Theme, F>(f),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
        }
    }

    /// A stylesheet that doesn't read the theme.
    pub fn r#static(rules: StyleRules) -> Self {
        Self {
            base: Box::new(move |_any: &dyn Any| rules.clone()),
            variants: BTreeMap::new(),
            compounds: Vec::new(),
        }
    }

    /// Adds (or replaces) a variant overlay on the given axis-value.
    /// If the axis didn't exist yet it's created with no default.
    pub fn variant<Theme, F>(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
        f: F,
    ) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        let axis = axis.into();
        let value = value.into();
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.values.insert(value, wrap_theme_fn::<Theme, F>(f));
        self
    }

    /// Sets the default value for an axis. When a call site omits this
    /// axis from the `VariantSet`, the default value's overlay is
    /// applied. The default value must also be added via `.variant(...)`
    /// (or it will silently apply nothing — same as today).
    pub fn variant_default(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        let axis = axis.into();
        let value = value.into();
        let entry = self.variants.entry(axis).or_insert_with(|| VariantAxisDef {
            default: None,
            values: BTreeMap::new(),
        });
        entry.default = Some(value);
        self
    }

    /// Adds a compound variant: an overlay applied only when every
    /// `(axis, value)` pair in `when` is active at apply time.
    pub fn compound<Theme, F>(
        mut self,
        when: Vec<(impl Into<VariantAxis>, impl Into<VariantValue>)>,
        f: F,
    ) -> Self
    where
        Theme: Any + 'static,
        F: Fn(&Theme) -> StyleRules + 'static,
    {
        let when: BTreeMap<VariantAxis, VariantValue> =
            when.into_iter().map(|(a, v)| (a.into(), v.into())).collect();
        self.compounds.push(CompoundVariant {
            when,
            rules: wrap_theme_fn::<Theme, F>(f),
        });
        self
    }

    /// Returns the effective `VariantSet` for resolution — the call site's
    /// `VariantSet` overlaid with each axis's declared default (if any)
    /// for axes the call site didn't specify.
    fn effective_variants(&self, requested: &VariantSet) -> VariantSet {
        let mut out = requested.clone();
        for (axis, def) in &self.variants {
            if !out.0.contains_key(axis) {
                if let Some(default) = &def.default {
                    out.0.insert(axis.clone(), default.clone());
                }
            }
        }
        out
    }

    /// Resolves the stylesheet against the given variants and theme.
    pub fn resolve(&self, variants: &VariantSet, theme: &dyn Any) -> StyleRules {
        let effective_variants = self.effective_variants(variants);
        let mut effective = (self.base)(theme);

        // Per-axis variants.
        for (axis, def) in &self.variants {
            if let Some(value) = effective_variants.0.get(axis) {
                if let Some(f) = def.values.get(value) {
                    effective = effective.merge(&f(theme));
                }
            }
        }

        // Compound variants — apply when every (axis, value) matches.
        for c in &self.compounds {
            let matches = c
                .when
                .iter()
                .all(|(axis, val)| effective_variants.0.get(axis) == Some(val));
            if matches {
                effective = effective.merge(&(c.rules)(theme));
            }
        }

        effective
    }

    // -----------------------------------------------------------------
    // Introspection for pre-generation
    // -----------------------------------------------------------------

    /// Returns every (axis, value) pair declared on this stylesheet.
    /// The pre-generator can walk these to mint a class per single-axis
    /// selection.
    pub fn variant_keys(&self) -> Vec<(VariantAxis, VariantValue)> {
        let mut out = Vec::new();
        for (axis, def) in &self.variants {
            for value in def.values.keys() {
                out.push((axis.clone(), value.clone()));
            }
        }
        out
    }

    /// Returns the declared compound variants' match conditions.
    pub fn compound_keys(&self) -> Vec<BTreeMap<VariantAxis, VariantValue>> {
        self.compounds.iter().map(|c| c.when.clone()).collect()
    }

    /// Returns the default value declared for an axis, if any.
    pub fn axis_default(&self, axis: &str) -> Option<&VariantValue> {
        self.variants.get(axis).and_then(|d| d.default.as_ref())
    }
}

/// Wraps an `Fn(&Theme) -> StyleRules` in the `Fn(&dyn Any) -> StyleRules`
/// shape we store inside `StyleSheet`. The downcast happens once per
/// call at the closure boundary — not per property.
fn wrap_theme_fn<Theme: Any + 'static, F: Fn(&Theme) -> StyleRules + 'static>(f: F) -> RulesFn {
    Box::new(move |any: &dyn Any| {
        let theme = any
            .downcast_ref::<Theme>()
            .expect("theme type mismatch — stylesheet expected a different theme type");
        f(theme)
    })
}

// ----------------------------------------------------------------------------
// VariantSet & StyleApplication
// ----------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct VariantSet(pub BTreeMap<VariantAxis, VariantValue>);

impl VariantSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        self.0.insert(axis.into(), value.into());
        self
    }
}

/// The value passed from author code to the framework. The framework
/// resolves it against the active theme into an `Rc<StyleRules>` before
/// handing off to the backend.
///
/// Resolution order (each layer overrides the previous for any
/// `Some(...)` property):
///
/// 1. **Base**: the stylesheet's `new(|theme| ...)` closure output.
/// 2. **Variants**: each active variant's overlay closure output.
/// 3. **Overrides**: per-call-site continuous values (this struct's
///    `overrides` field). Used for values that can't be enumerated as
///    discrete variants — e.g. a user-controlled font scale.
///
/// The backend sees the merged result; it doesn't know which layer
/// contributed what. Backend caches (web CSS classes, etc.) key on the
/// resolved content so each unique combination still gets its own
/// entry — overrides preserve cacheability without inline styles.
#[derive(Clone)]
pub struct StyleApplication {
    pub sheet: Rc<StyleSheet>,
    pub variants: VariantSet,
    pub overrides: StyleRules,
}

impl StyleApplication {
    pub fn new(sheet: Rc<StyleSheet>) -> Self {
        Self {
            sheet,
            variants: VariantSet::new(),
            overrides: StyleRules::default(),
        }
    }

    pub fn with(
        mut self,
        axis: impl Into<VariantAxis>,
        value: impl Into<VariantValue>,
    ) -> Self {
        self.variants.0.insert(axis.into(), value.into());
        self
    }

    /// Override the background color with a per-call-site value.
    pub fn override_background(mut self, c: impl Into<Color>) -> Self {
        self.overrides.background = Some(c.into());
        self
    }

    /// Override the foreground color with a per-call-site value.
    pub fn override_color(mut self, c: impl Into<Color>) -> Self {
        self.overrides.color = Some(c.into());
        self
    }

    /// Override padding with a per-call-site value.
    pub fn override_padding(mut self, v: Length) -> Self {
        self.overrides.padding = Some(v);
        self
    }

    /// Override font size with a per-call-site value. Useful for cases
    /// like user-controlled zoom where the value is continuous.
    pub fn override_font_size(mut self, v: Length) -> Self {
        self.overrides.font_size = Some(v);
        self
    }

    /// Override border radius with a per-call-site value.
    pub fn override_border_radius(mut self, v: Length) -> Self {
        self.overrides.border_radius = Some(v);
        self
    }
}

// ----------------------------------------------------------------------------
// Global active theme & resolution cache
// ----------------------------------------------------------------------------

thread_local! {
    /// The active theme. Wrapped in a `Signal<Rc<dyn Any>>` so effects
    /// subscribe via the existing reactivity system and re-apply on swap.
    static ACTIVE_THEME: RefCell<Option<crate::Signal<Rc<dyn Any>>>> = const { RefCell::new(None) };

    /// Memoization: `(stylesheet pointer, variants, theme pointer,
    /// override content)` → `Weak<StyleRules>`. Strong refs are held by
    /// `REGISTRATIONS` for pre-generated styles, and transiently by the
    /// caller of `resolve(...)` for dynamic ones. When the last strong
    /// ref drops, the Weak in this cache fails to upgrade and the entry
    /// is opportunistically swept on the next insert.
    ///
    /// Cleared on theme change because old entries reference the old
    /// theme pointer and would never be reused.
    static RESOLUTION_CACHE: RefCell<HashMap<ResolutionKey, std::rc::Weak<StyleRules>>> =
        RefCell::new(HashMap::new());

    /// Each currently-registered `(stylesheet, theme)` pair, with the
    /// rules that were pre-generated for it and a `Weak<StyleSheet>`
    /// used to detect when the stylesheet has been dropped by all
    /// holders. The framework calls `Backend::register_stylesheet`
    /// exactly once per pair and tracks the rules so we can later call
    /// `unregister_stylesheet` to free backend-side state.
    static REGISTRATIONS: RefCell<HashMap<RegKey, Registration>> =
        RefCell::new(HashMap::new());

    /// Rule sets queued for `unregister_stylesheet` calls. Populated by
    /// `set_theme` (moves all current registrations here) and by the
    /// sweep-dead-stylesheets pass (moves dead entries here). Drained
    /// by `ensure_registered_with`, which has the backend in scope.
    static PENDING_UNREGISTER: RefCell<Vec<Vec<Rc<StyleRules>>>> =
        RefCell::new(Vec::new());
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct RegKey {
    sheet: *const StyleSheet,
    theme: *const (),
}

struct Registration {
    weak: std::rc::Weak<StyleSheet>,
    rules: Vec<Rc<StyleRules>>,
}

#[derive(PartialEq, Eq, Hash)]
struct ResolutionKey {
    sheet: *const StyleSheet,
    variants: VariantSet,
    theme: *const (),
    /// Overrides are part of the cache key — same sheet + variants +
    /// theme but different override values yield different rules and
    /// must be cached separately. Serialized to a content key so we
    /// have a comparable form.
    overrides: String,
}

/// Install the initial active theme. Call once at app startup before
/// rendering.
pub fn install_theme<Theme: Any + 'static>(theme: Theme) {
    let rc: Rc<dyn Any> = Rc::new(theme);
    let sig = crate::Signal::new(rc);
    ACTIVE_THEME.with(|t| *t.borrow_mut() = Some(sig));
}

/// Swap the active theme. Every styled component subscribed via the
/// reactive renderer re-fires its apply-style effect and re-applies
/// with the new theme's values.
///
/// All currently-registered `(stylesheet, theme)` pairs are queued for
/// `unregister_stylesheet`; the backend hears about them on the next
/// `ensure_registered_with` call (which has it in scope).
pub fn set_theme<Theme: Any + 'static>(theme: Theme) {
    let rc: Rc<dyn Any> = Rc::new(theme);
    RESOLUTION_CACHE.with(|c| c.borrow_mut().clear());

    // Move every current registration into the pending-unregister queue.
    // The next styled effect that fires will flush it with the backend
    // in scope.
    REGISTRATIONS.with(|r| {
        let mut regs = r.borrow_mut();
        PENDING_UNREGISTER.with(|p| {
            let mut pending = p.borrow_mut();
            for (_, reg) in regs.drain() {
                pending.push(reg.rules);
            }
        });
    });

    ACTIVE_THEME.with(|t| {
        if let Some(sig) = t.borrow().as_ref() {
            sig.set(rc);
        } else {
            let new_sig = crate::Signal::new(rc);
            *t.borrow_mut() = Some(new_sig);
        }
    });
}

/// Ensures the backend has been asked to pre-generate state for this
/// stylesheet against the active theme. Calls `register` with the
/// resolved rules exactly once per `(sheet, theme)` pair.
///
/// Also opportunistically:
/// - Flushes the pending-unregister queue, calling `unregister` for
///   each rule set queued by `set_theme` or a dead-stylesheet sweep.
/// - Sweeps registrations whose `Weak<StyleSheet>` no longer upgrades
///   into the pending-unregister queue.
pub fn ensure_registered_with<R, U>(sheet: &Rc<StyleSheet>, register: R, unregister: U)
where
    R: FnOnce(&[Rc<StyleRules>]),
    U: Fn(&[Rc<StyleRules>]),
{
    let theme = active_theme();
    let sheet_ptr = Rc::as_ptr(sheet);
    let theme_ptr = Rc::as_ptr(&theme) as *const ();
    let key = RegKey { sheet: sheet_ptr, theme: theme_ptr };

    // Sweep dead registrations (Weak no longer upgrades). They go to
    // the pending-unregister queue.
    REGISTRATIONS.with(|r| {
        let mut regs = r.borrow_mut();
        let dead_keys: Vec<RegKey> = regs
            .iter()
            .filter_map(|(k, reg)| {
                if reg.weak.upgrade().is_none() {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        if !dead_keys.is_empty() {
            PENDING_UNREGISTER.with(|p| {
                let mut pending = p.borrow_mut();
                for k in dead_keys {
                    if let Some(reg) = regs.remove(&k) {
                        pending.push(reg.rules);
                    }
                }
            });
        }
    });

    // Flush pending unregistrations now that the backend is in scope.
    let pending: Vec<Vec<Rc<StyleRules>>> =
        PENDING_UNREGISTER.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for rules in &pending {
        unregister(rules);
    }

    // Already registered? Done.
    let already = REGISTRATIONS.with(|r| r.borrow().contains_key(&key));
    if already {
        return;
    }

    // Register fresh.
    let rules = pregenerate_for_theme(sheet, &*theme);
    register(&rules);
    REGISTRATIONS.with(|r| {
        r.borrow_mut().insert(
            key,
            Registration {
                weak: Rc::downgrade(sheet),
                rules,
            },
        );
    });
}

/// Read the active theme. Subscribes the current effect (if any) to
/// theme changes — that's how reactive style application works.
pub fn active_theme() -> Rc<dyn Any> {
    ACTIVE_THEME.with(|t| {
        t.borrow()
            .as_ref()
            .expect("no theme installed; call install_theme(...) before rendering")
            .get()
    })
}

/// Returns the set of pre-resolvable `StyleRules` for a stylesheet
/// against a given theme. Includes:
/// - The base rules (no variants active).
/// - One entry per declared (axis, value) — variant overlay layered on
///   base.
/// - One entry per declared compound variant — the matched compound
///   layered on the base + the compound's `when` clause's variants.
///
/// Continuous overrides are NOT pre-generatable and aren't included.
/// Backends like the web backend use this to mint CSS classes ahead of
/// time so `apply_style` is a cache hit.
pub fn pregenerate_for_theme(sheet: &StyleSheet, theme: &dyn Any) -> Vec<Rc<StyleRules>> {
    let mut out: Vec<Rc<StyleRules>> = Vec::new();

    // 1. Base.
    out.push(Rc::new(sheet.resolve(&VariantSet::new(), theme)));

    // 2. Each (axis, value) — every single-axis variant selection.
    for (axis, value) in sheet.variant_keys() {
        let variants = VariantSet::new().with(axis, value);
        out.push(Rc::new(sheet.resolve(&variants, theme)));
    }

    // 3. Each compound — the compound's `when` clause defines the
    //    minimum variant selection that triggers it.
    for compound_keys in sheet.compound_keys() {
        let mut variants = VariantSet::new();
        for (axis, value) in compound_keys {
            variants.0.insert(axis, value);
        }
        out.push(Rc::new(sheet.resolve(&variants, theme)));
    }

    out
}

/// Resolve a style application against the current active theme.
/// Memoized; reads the theme signal so changes re-fire the caller's effect.
///
/// Cache entries are `Weak<StyleRules>`. Pre-generated styles have
/// long-lived strong refs held by `REGISTRATIONS`; dynamic
/// (override-bearing) styles have only the transient strong ref
/// returned to the caller. When that drops, the Weak becomes dead
/// and the slot is opportunistically swept on the next insert.
pub fn resolve(app: &StyleApplication) -> Rc<StyleRules> {
    let theme = active_theme();
    let theme_ptr = Rc::as_ptr(&theme) as *const ();
    let key = ResolutionKey {
        sheet: Rc::as_ptr(&app.sheet),
        variants: app.variants.clone(),
        theme: theme_ptr,
        overrides: app.overrides.content_key(),
    };

    // Cache hit? Try upgrading the Weak.
    if let Some(rc) = RESOLUTION_CACHE.with(|c| c.borrow().get(&key).and_then(|w| w.upgrade())) {
        return rc;
    }

    // Miss (or dead Weak). Resolve fresh.
    let base_and_variants = app.sheet.resolve(&app.variants, &*theme);
    let final_rules = base_and_variants.merge(&app.overrides);
    let resolved = Rc::new(final_rules);

    // Insert as Weak. Also opportunistically sweep dead entries to
    // keep the cache bounded over time.
    RESOLUTION_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.retain(|_, w| w.strong_count() > 0);
        cache.insert(key, Rc::downgrade(&resolved));
    });

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTheme {
        surface: String,
        medium: f32,
    }

    fn light() -> TestTheme {
        TestTheme { surface: "#fff".into(), medium: 16.0 }
    }

    fn dark() -> TestTheme {
        TestTheme { surface: "#111".into(), medium: 24.0 }
    }

    #[test]
    fn closure_stylesheet_reads_theme() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding: Some(t.medium),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new(), &l);
        assert_eq!(r.background, Some(Color("#fff".into())));
        assert_eq!(r.padding, Some(16.0));
    }

    #[test]
    fn static_stylesheet_ignores_theme() {
        let sheet = StyleSheet::r#static(StyleRules {
            background: Some(Color("#abc".into())),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new(), &l);
        assert_eq!(r.background, Some(Color("#abc".into())));
    }

    #[test]
    fn variant_overlays_layer_on_top_of_base() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding: Some(t.medium),
            ..Default::default()
        })
        .variant("size", "large", |t: &TestTheme| StyleRules {
            padding: Some(t.medium * 2.0),
            ..Default::default()
        });
        let l = light();
        let r = sheet.resolve(&VariantSet::new().with("size", "large"), &l);
        assert_eq!(r.background, Some(Color("#fff".into())));
        assert_eq!(r.padding, Some(32.0));
    }

    #[test]
    fn theme_swap_changes_resolution() {
        install_theme(light());
        let sheet = Rc::new(StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);

        let r1 = resolve(&app);
        assert_eq!(r1.background, Some(Color("#fff".into())));

        set_theme(dark());
        let r2 = resolve(&app);
        assert_eq!(r2.background, Some(Color("#111".into())));
    }

    #[test]
    fn overrides_layer_on_top_of_base_and_variants() {
        install_theme(light());
        let sheet = Rc::new(
            StyleSheet::new(|t: &TestTheme| StyleRules {
                background: Some(Color(t.surface.clone())),
                font_size: Some(14.0),
                padding: Some(t.medium),
                ..Default::default()
            })
            .variant("size", "large", |_t: &TestTheme| StyleRules {
                font_size: Some(20.0),
                ..Default::default()
            }),
        );

        // Base only: background from theme, font 14, padding from theme.
        let r1 = resolve(&StyleApplication::new(sheet.clone()));
        assert_eq!(r1.font_size, Some(14.0));

        // With variant: font becomes 20.
        let r2 = resolve(&StyleApplication::new(sheet.clone()).with("size", "large"));
        assert_eq!(r2.font_size, Some(20.0));

        // With variant + override: override wins.
        let r3 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(17.5),
        );
        assert_eq!(r3.font_size, Some(17.5));
        // Other properties unaffected by the override.
        assert_eq!(r3.padding, Some(16.0));

        // Different override values produce distinct cache entries.
        let r4 = resolve(
            &StyleApplication::new(sheet.clone())
                .with("size", "large")
                .override_font_size(99.0),
        );
        assert_eq!(r4.font_size, Some(99.0));
        assert!(!Rc::ptr_eq(&r3, &r4));
    }

    #[test]
    fn variant_default_applies_when_axis_unselected() {
        let sheet = StyleSheet::new(|t: &TestTheme| StyleRules {
            background: Some(Color(t.surface.clone())),
            padding: Some(8.0),
            ..Default::default()
        })
        .variant("size", "small", |_t: &TestTheme| StyleRules {
            padding: Some(4.0),
            ..Default::default()
        })
        .variant("size", "large", |_t: &TestTheme| StyleRules {
            padding: Some(16.0),
            ..Default::default()
        })
        .variant_default("size", "large");

        // Call site omits `size` → default "large" applies → padding 16.
        let r = sheet.resolve(&VariantSet::new(), &light());
        assert_eq!(r.padding, Some(16.0));

        // Call site picks "small" → padding 4.
        let r2 = sheet.resolve(&VariantSet::new().with("size", "small"), &light());
        assert_eq!(r2.padding, Some(4.0));
    }

    #[test]
    fn compound_variant_applies_only_when_all_match() {
        let sheet = StyleSheet::new(|_t: &TestTheme| StyleRules::default())
            .variant("size", "large", |_t: &TestTheme| StyleRules {
                padding: Some(16.0),
                ..Default::default()
            })
            .variant("kind", "primary", |_t: &TestTheme| StyleRules {
                background: Some(Color("primary-bg".into())),
                ..Default::default()
            })
            .compound::<TestTheme, _>(
                vec![("size", "large"), ("kind", "primary")],
                |_t| StyleRules {
                    font_size: Some(24.0),
                    ..Default::default()
                },
            );

        // Only size=large → compound NOT applied.
        let r1 = sheet.resolve(&VariantSet::new().with("size", "large"), &light());
        assert_eq!(r1.padding, Some(16.0));
        assert_eq!(r1.font_size, None);

        // Both axes match → compound APPLIED.
        let r2 = sheet.resolve(
            &VariantSet::new().with("size", "large").with("kind", "primary"),
            &light(),
        );
        assert_eq!(r2.padding, Some(16.0));
        assert_eq!(r2.background, Some(Color("primary-bg".into())));
        assert_eq!(r2.font_size, Some(24.0));
    }

    #[test]
    fn variant_keys_lists_every_axis_value() {
        let sheet = StyleSheet::new(|_t: &TestTheme| StyleRules::default())
            .variant("size", "small", |_t: &TestTheme| StyleRules::default())
            .variant("size", "large", |_t: &TestTheme| StyleRules::default())
            .variant("kind", "primary", |_t: &TestTheme| StyleRules::default());
        let mut keys = sheet.variant_keys();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ("kind".to_string(), "primary".to_string()),
                ("size".to_string(), "large".to_string()),
                ("size".to_string(), "small".to_string()),
            ]
        );
    }

    #[test]
    fn resolve_memoizes_same_inputs() {
        install_theme(light());
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Color("#abc".into())),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);
        let r1 = resolve(&app);
        let r2 = resolve(&app);
        assert!(Rc::ptr_eq(&r1, &r2));
    }
}
