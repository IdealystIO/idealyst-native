//! Runtime-free macOS NSToolbar machinery.
//!
//! Everything AppKit lives here: the delegate class, NSToolbar
//! construction, the wipe+repopulate item update, the 0-size in-tree
//! placeholder view, and the imperative ops. What does NOT live here is
//! anything reactive — `macos.rs` (the scene handler) owns:
//!
//! - the reactive items subscription (`runtime_world::effect`), and
//! - the click-callback discipline ([`plan_items`]'s `wrap_click`
//!   parameter: a `backend_macos::newcore::schedule_flush` wrapper —
//!   NSToolbar target-action fires outside the runtime's wrapped
//!   dispatch sites, so unwrapped clicks would stage signal writes
//!   forever).
//!
//! The split point is data, not props structs: [`build_native_toolbar`]
//! takes the initial `visible` bool + an optional host window, and
//! [`apply_items`] takes an [`ItemPlan`] produced by the pure
//! [`plan_items`] translation. This module never names `ToolbarProps`;
//! [`ToolbarItem`](crate::ToolbarItem) itself is runtime-free
//! (`src/items.rs`) and shared.
//!
//! NSToolbar + NSToolbarItem + the delegate are reached at the Obj-C
//! runtime layer via `class!()` + `msg_send` rather than typed
//! objc2-app-kit bindings — `objc2-app-kit` 0.2 doesn't ship an
//! `NSToolbar` feature, and bumping to 0.3 would conflict with
//! `backend-macos`'s objc2 0.5 major. Same posture `maps-ios` takes
//! for MKMapView.
//!
//! # Lifetime model
//!
//! - `NSToolbar` is retained by `NSWindow.setToolbar:`. Once attached
//!   it survives until the window itself drops, which is the app's
//!   lifetime.
//! - The Rust-side `ToolbarDelegate` holds the click callbacks and
//!   item metadata. NSToolbar holds its delegate weakly, so the
//!   scene handler must keep the returned [`NativeToolbar`] (which
//!   owns a `Retained<ToolbarDelegate>`) alive — both legs do this by
//!   capturing it in their reactive items closure, which the framework
//!   scope owns for as long as the primitive is mounted. Same trick
//!   `webview-ios` uses to keep its delegate alive past
//!   `build_web_view`.
//! - If the primitive ever unmounts (user toggles
//!   `if condition { Toolbar(...) }` off), the scope drops, the
//!   reactive closure drops, the delegate's last Retained drops, and
//!   the toolbar's weak delegate slot goes nil. NSToolbar then renders
//!   empty rather than crashing — which is the right degradation for
//!   an unmounted toolbar that the host hasn't separately detached. A
//!   future improvement would `window.setToolbar:nil` on placeholder
//!   drop, but the framework doesn't yet expose a per-node drop hook
//!   that fires before the NSView teardown.
//!
//! # Item-update strategy
//!
//! On every reactive re-fire the wrapper calls [`plan_items`] +
//! [`apply_items`]: we walk the new `Vec<ToolbarItem>`, rewrite the
//! delegate's ivar records, then update the visible item list.
//! NSToolbar's mutation API is `removeItemAtIndex:` /
//! `insertItemWithItemIdentifier:atIndex:`; we wipe + repopulate so
//! the diff is trivially correct. For dozens of items this is
//! imperceptible; if it ever shows up as a perf hit we can switch to
//! a longest-common-subsequence diff.

use crate::items::ToolbarItem;
use crate::ToolbarOps;
use backend_macos::MacosNode;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::NSView;
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSString};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) static OPS: &dyn ToolbarOps = &MacosToolbarOps;

/// Stable toolbar identifier. NSWindow persists toolbar state (visible
/// items, customization) keyed by this string; using a fixed value
/// means the user's customization survives across launches.
const TOOLBAR_IDENTIFIER: &str = "com.idealyst.toolbar";

/// Identifier prefix for our app-defined items. Suffix is the item's
/// stable index within the most recent `items()` evaluation.
const ITEM_IDENTIFIER_PREFIX: &str = "com.idealyst.toolbar.item.";

// =========================================================================
// Delegate class — vends NSToolbarItems on demand + receives click
// actions. NSToolbar's delegate protocol is selector-presence checked
// at runtime, so we declare an NSObject subclass with the required
// methods rather than a formal protocol conformance.
// =========================================================================

/// One row of the delegate's item table. `identifier` is the stable
/// NSString that the toolbar uses to refer to this item across calls
/// to `toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:` and
/// click action dispatch.
struct ItemRecord {
    identifier: Retained<NSString>,
    /// Kind dictates which NSToolbarItem we vend:
    /// - `Button { label, icon, tooltip, on_click }` → a real
    ///   NSToolbarItem with target/action wired back to the delegate.
    /// - `Separator` / `Space` → no item record needed; the visible
    ///   identifier list points directly at `NSToolbarSpaceItemIdentifier`.
    /// - `FlexibleSpace` → ditto with the flexible-space identifier.
    /// Only `Button` records make it into the delegate's `items` Vec;
    /// the space/separator identifiers go straight into the visible
    /// identifier list.
    label: String,
    icon: Option<String>,
    tooltip: Option<String>,
    on_click: Option<Rc<dyn Fn()>>,
}

pub(crate) struct ToolbarDelegateIvars {
    /// User-defined button items, keyed by NSString identifier. The
    /// `toolbar:itemForItemIdentifier:...` delegate method looks up
    /// the matching record and constructs an NSToolbarItem from it.
    items: RefCell<Vec<ItemRecord>>,
    /// The ordered list of identifiers currently visible in the
    /// toolbar — mixes button identifiers with the system space /
    /// flexible-space identifiers. Returned verbatim from
    /// `toolbarDefaultItemIdentifiers:` and
    /// `toolbarAllowedItemIdentifiers:`.
    visible_identifiers: RefCell<Vec<Retained<NSString>>>,
}

declare_class!(
    pub(crate) struct ToolbarDelegate;

    unsafe impl ClassType for ToolbarDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystToolbarDelegate";
    }

    impl DeclaredClass for ToolbarDelegate {
        type Ivars = ToolbarDelegateIvars;
    }

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl ToolbarDelegate {
        // ---- NSToolbarDelegate -----------------------------------------

        #[method_id(toolbarDefaultItemIdentifiers:)]
        fn toolbar_default_item_identifiers(
            &self,
            _toolbar: &NSObject,
        ) -> Retained<NSArray<NSString>> {
            let ids = self.ivars().visible_identifiers.borrow();
            // NSArray::from_vec consumes a Vec<Retained<NSString>>;
            // clone each Retained so the delegate's source-of-truth
            // ivar isn't disturbed.
            let cloned: Vec<Retained<NSString>> = ids.iter().cloned().collect();
            NSArray::from_vec(cloned)
        }

        #[method_id(toolbarAllowedItemIdentifiers:)]
        fn toolbar_allowed_item_identifiers(
            &self,
            _toolbar: &NSObject,
        ) -> Retained<NSArray<NSString>> {
            // Allowed = default for v1 — we don't expose customization
            // yet. Surfacing the extra item identifiers needed for
            // the "Customize Toolbar..." palette is a follow-up.
            let ids = self.ivars().visible_identifiers.borrow();
            let cloned: Vec<Retained<NSString>> = ids.iter().cloned().collect();
            NSArray::from_vec(cloned)
        }

        #[method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:)]
        fn toolbar_item_for_identifier(
            &self,
            _toolbar: &NSObject,
            identifier: &NSString,
            _will_be_inserted: bool,
        ) -> Option<Retained<NSObject>> {
            // Body in a helper to keep early-return + question-mark
            // shapes out of declare_class!'s macro expansion — the
            // macro's IdReturnValue conversion chokes on early
            // `return None;` inside `Option<Retained<_>>` bodies (see
            // [[project_macos_appkit_uikit_diffs]] / the same workaround
            // the backend uses for `item_for_index_path`).
            self.item_for_identifier_impl(identifier)
        }

        // ---- Click action ----------------------------------------------

        /// Toolbar item action. The sender is the NSToolbarItem; we
        /// read its `itemIdentifier` to look up which button fired,
        /// then invoke the stored `on_click` closure.
        #[method(itemClicked:)]
        fn item_clicked(&self, sender: &NSObject) {
            let identifier_ptr: *mut NSString =
                unsafe { msg_send![sender, itemIdentifier] };
            if identifier_ptr.is_null() {
                return;
            }
            let identifier = unsafe { &*identifier_ptr };
            let id_str = identifier.to_string();
            // Borrow the items list immutably, clone the Rc out, then
            // drop the borrow before firing — the callback may
            // re-enter the toolbar state (e.g. by setting a signal
            // that triggers the items Effect to rebuild the list).
            let callback = {
                let items = self.ivars().items.borrow();
                items
                    .iter()
                    .find(|r| r.identifier.to_string() == id_str)
                    .and_then(|r| r.on_click.clone())
            };
            if let Some(cb) = callback {
                cb();
            }
        }
    }
);

impl ToolbarDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ToolbarDelegateIvars {
            items: RefCell::new(Vec::new()),
            visible_identifiers: RefCell::new(Vec::new()),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Body of the `toolbar:itemForItemIdentifier:...` delegate method.
    /// Lives outside `declare_class!` so it can use early returns + `?`
    /// freely. Returns `None` for system identifiers (NSToolbar
    /// substitutes the system item automatically) and for unknown
    /// identifiers (defensive — happens if a stale identifier survives
    /// a reactive item-list swap that races a refresh).
    fn item_for_identifier_impl(
        &self,
        identifier: &NSString,
    ) -> Option<Retained<NSObject>> {
        let id_str = identifier.to_string();
        if is_system_identifier(&id_str) {
            return None;
        }
        let items = self.ivars().items.borrow();
        let record = items.iter().find(|r| r.identifier.to_string() == id_str)?;
        Some(make_toolbar_item(record, self))
    }
}

// =========================================================================
// Item construction
// =========================================================================

fn make_toolbar_item(record: &ItemRecord, delegate: &ToolbarDelegate) -> Retained<NSObject> {
    let item_class = class!(NSToolbarItem);
    let label_ns = NSString::from_str(&record.label);
    let item: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![item_class, alloc];
        let inited: *mut AnyObject =
            msg_send![allocated, initWithItemIdentifier: &*record.identifier];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("NSToolbarItem init returned nil")
    };
    let _: () = unsafe { msg_send![&*item, setLabel: &*label_ns] };
    let _: () = unsafe { msg_send![&*item, setPaletteLabel: &*label_ns] };
    if let Some(tooltip) = &record.tooltip {
        let ns = NSString::from_str(tooltip);
        let _: () = unsafe { msg_send![&*item, setToolTip: &*ns] };
    }
    if let Some(icon_name) = &record.icon {
        if let Some(image) = sf_symbol(icon_name, &record.label) {
            let _: () = unsafe { msg_send![&*item, setImage: &*image] };
        }
    }
    let _: () = unsafe { msg_send![&*item, setTarget: delegate] };
    let _: () = unsafe { msg_send![&*item, setAction: sel!(itemClicked:)] };
    // Auto-enable would require an NSToolbarItemValidation conformance;
    // for v1 keep the item always enabled. `setAutovalidates:false` +
    // `setEnabled:true` is the explicit way to pin that.
    let _: () = unsafe { msg_send![&*item, setAutovalidates: false] };
    let _: () = unsafe { msg_send![&*item, setEnabled: true] };
    item
}

/// Look up an SF Symbol by name and return the resulting NSImage.
/// Falls back to `None` if the symbol doesn't exist on the running
/// macOS version (SF Symbols are 11+; if the API itself isn't
/// available we also return None and the item renders label-only).
fn sf_symbol(name: &str, accessibility_description: &str) -> Option<Retained<NSObject>> {
    let image_class = class!(NSImage);
    let symbol_name = NSString::from_str(name);
    let desc = NSString::from_str(accessibility_description);
    // `+[NSImage imageWithSystemSymbolName:accessibilityDescription:]`
    // returns nil for unknown symbols. On macOS < 11 the selector
    // itself doesn't exist; `respondsToSelector:` guards both cases.
    let sel = sel!(imageWithSystemSymbolName:accessibilityDescription:);
    let responds: bool = unsafe { msg_send![image_class, respondsToSelector: sel] };
    if !responds {
        return None;
    }
    let image_ptr: *mut AnyObject = unsafe {
        msg_send![
            image_class,
            imageWithSystemSymbolName: &*symbol_name,
            accessibilityDescription: &*desc,
        ]
    };
    if image_ptr.is_null() {
        return None;
    }
    // The class method returns an autoreleased instance; retain so it
    // outlives the autorelease pool. `imageWithSystemSymbolName:`
    // follows the AppKit "without alloc/copy/new/init prefix → must
    // retain" convention.
    let retained_ptr: *mut AnyObject = unsafe { msg_send![image_ptr, retain] };
    let retained = unsafe { Retained::from_raw(retained_ptr.cast::<NSObject>()) };
    retained
}

fn is_system_identifier(id: &str) -> bool {
    matches!(
        id,
        "NSToolbarSpaceItem"
            | "NSToolbarFlexibleSpaceItem"
            | "NSToolbarSeparatorItem"
    )
}

// =========================================================================
// Item planning — the pure ToolbarItem → identifier/record translation
// =========================================================================

/// One planned button record: [`ItemRecord`] before NSString
/// materialization. Pure data so the translation ([`plan_items`]) is
/// unit-testable off the main thread.
pub(crate) struct PlannedItem {
    pub(crate) identifier: String,
    pub(crate) label: String,
    pub(crate) icon: Option<String>,
    pub(crate) tooltip: Option<String>,
    pub(crate) on_click: Option<Rc<dyn Fn()>>,
}

/// The output of [`plan_items`]: the ordered visible-identifier list
/// (buttons + system space identifiers, in author order) plus the
/// button records the delegate vends NSToolbarItems from.
pub(crate) struct ItemPlan {
    pub(crate) records: Vec<PlannedItem>,
    pub(crate) identifiers: Vec<String>,
}

/// Translate a fresh `Vec<ToolbarItem>` into an [`ItemPlan`]. Pure —
/// no AppKit — so the translation is unit-testable on its own.
///
/// `wrap_click` is the click-callback discipline, applied to every
/// button `on_click` as it enters the plan: production wraps with
/// `backend_macos::newcore::schedule_flush` (NSToolbar target-action
/// fires outside the runtime's wrapped dispatch sites). Parameterized so
/// the ordering contract is testable without a booted app.
pub(crate) fn plan_items(
    new_items: Vec<ToolbarItem>,
    wrap_click: &dyn Fn(Rc<dyn Fn()>) -> Rc<dyn Fn()>,
) -> ItemPlan {
    let mut records: Vec<PlannedItem> = Vec::new();
    let mut identifiers: Vec<String> = Vec::with_capacity(new_items.len());
    let mut button_counter: usize = 0;

    for item in new_items {
        match item {
            ToolbarItem::Button(b) => {
                let id_string = format!("{}{}", ITEM_IDENTIFIER_PREFIX, button_counter);
                button_counter += 1;
                identifiers.push(id_string.clone());
                records.push(PlannedItem {
                    identifier: id_string,
                    label: b.label,
                    icon: b.icon,
                    tooltip: b.tooltip,
                    on_click: b.on_click.map(|cb| wrap_click(cb)),
                });
            }
            ToolbarItem::Separator => {
                // NSToolbarSeparatorItemIdentifier was deprecated in
                // macOS 10.10. Render as a fixed-width space for a
                // best-effort visual gap. Authors who want a true
                // vertical rule can compose a custom-view item once
                // that surface lands.
                identifiers.push("NSToolbarSpaceItem".to_string());
            }
            ToolbarItem::Space => {
                identifiers.push("NSToolbarSpaceItem".to_string());
            }
            ToolbarItem::FlexibleSpace => {
                identifiers.push("NSToolbarFlexibleSpaceItem".to_string());
            }
        }
    }

    ItemPlan {
        records,
        identifiers,
    }
}

// =========================================================================
// Toolbar build + item application
// =========================================================================

/// The live native toolbar pair. Clone is cheap (two `Retained`
/// bumps); the scene handler's reactive closure captures a clone, which
/// is what anchors the weakly-held delegate (see the module-level
/// lifetime model).
#[derive(Clone)]
pub(crate) struct NativeToolbar {
    toolbar: Retained<NSObject>,
    delegate: Retained<ToolbarDelegate>,
}

/// Resolve the `NSWindow` hosting `root` (the backend's host root
/// view). `None` when the view isn't in a window yet — unusual (the
/// host's `setContentView:` runs before render starts), but defended:
/// the caller then builds a detached toolbar that simply never
/// displays.
pub(crate) fn window_of_view(root: &NSView) -> Option<Retained<NSObject>> {
    let win_ptr: *mut NSObject = unsafe { msg_send![root, window] };
    if win_ptr.is_null() {
        None
    } else {
        // `window` is a non-owning accessor; retain to take
        // ownership for the toolbar's lifetime.
        let retained: *mut AnyObject = unsafe { msg_send![win_ptr, retain] };
        Some(
            unsafe { Retained::from_raw(retained.cast::<NSObject>()) }
                .expect("NSWindow retain returned nil"),
        )
    }
}

/// Build NSToolbar + delegate, configure them, and attach to `window`
/// (when present). Items arrive later via [`apply_items`] — the scene
/// handler's reactive effect drives that.
pub(crate) fn build_native_toolbar(
    mtm: MainThreadMarker,
    window: Option<&NSObject>,
    visible: bool,
) -> NativeToolbar {
    let toolbar_class = class!(NSToolbar);
    let identifier = NSString::from_str(TOOLBAR_IDENTIFIER);
    let toolbar: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![toolbar_class, alloc];
        let inited: *mut AnyObject =
            msg_send![allocated, initWithIdentifier: &*identifier];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("NSToolbar init returned nil")
    };
    let delegate = ToolbarDelegate::new(mtm);

    // Configure: show both icon + label (looks closest to first-party
    // Apple apps with toolbar buttons); persist user customization
    // across launches via the stable identifier.
    // Display modes: 0 = Default, 1 = IconAndLabel, 2 = IconOnly,
    // 3 = LabelOnly. `1` matches Mail/Finder. NSToolbarDisplayMode is
    // typed `NS_ENUM(NSUInteger, …)` — must pass `usize` ('Q' encoding),
    // not `i64`/`isize` ('q'), or objc2's runtime encoding check
    // panics with "expected 'Q', found 'q'".
    let _: () = unsafe { msg_send![&*toolbar, setDisplayMode: 1_usize] };
    let _: () = unsafe { msg_send![&*toolbar, setAllowsUserCustomization: false] };
    let _: () = unsafe { msg_send![&*toolbar, setAutosavesConfiguration: false] };
    let _: () = unsafe { msg_send![&*toolbar, setDelegate: &*delegate] };
    let _: () = unsafe { msg_send![&*toolbar, setVisible: visible] };

    if let Some(win) = window {
        let _: () = unsafe { msg_send![win, setToolbar: &*toolbar] };
    }

    NativeToolbar { toolbar, delegate }
}

/// Build the zero-sized placeholder NSView both legs return as the
/// primitive's in-tree node. The framework's view tree gets a real
/// node so layout/lifecycle wiring is consistent, but the placeholder
/// is transparent and 0×0 so it's invisible. NSView's default frame is
/// zero; the layout pass won't touch it unless the user gives the
/// primitive a style (which isn't a Toolbar idiom).
///
/// Plain NSView (no custom subclass) — already returns
/// `Retained<NSView>`, no `into_super` upcast needed. FlippedView
/// would be appropriate for top-down coordinate spaces in the
/// visible tree, but this placeholder is never visible (0×0,
/// transparent), so the default coordinate convention is fine.
pub(crate) fn make_placeholder_view(mtm: MainThreadMarker) -> Retained<NSView> {
    let placeholder: Retained<NSView> = unsafe { NSView::new(mtm) };
    // Force CALayer-backing on the placeholder. The backend's layout
    // pass walks every registered view and calls per-frame sync
    // helpers (corner-radius clamp, gradient sublayer resize) that
    // request `view.layer` unconditionally and panic on nil. NSView
    // is layer-optional on AppKit, so an unstyled placeholder with
    // no `setWantsLayer:true` has a nil layer and trips those helpers.
    // Setting it here decouples us from pre-existing backend nil-layer
    // assumptions; the layer is invisible (no background) and zero-sized
    // so it has no visual effect.
    let _: () = unsafe { objc2::msg_send![&*placeholder, setWantsLayer: true] };
    placeholder
}

/// Rebuild the NSToolbar's visible items from a fresh [`ItemPlan`].
/// Materializes the plan's records into the delegate's `items` ivar,
/// then applies the visible-identifier list to the toolbar via
/// clear-then-insert.
pub(crate) fn apply_items(native: &NativeToolbar, plan: ItemPlan) {
    let ItemPlan {
        records,
        identifiers,
    } = plan;
    let toolbar = &native.toolbar;
    let delegate = &native.delegate;

    let new_records: Vec<ItemRecord> = records
        .into_iter()
        .map(|p| ItemRecord {
            identifier: NSString::from_str(&p.identifier),
            label: p.label,
            icon: p.icon,
            tooltip: p.tooltip,
            on_click: p.on_click,
        })
        .collect();
    let new_identifiers: Vec<Retained<NSString>> =
        identifiers.iter().map(|s| NSString::from_str(s)).collect();

    // Swap the delegate's source-of-truth before mutating the toolbar
    // so the `toolbar:itemForItemIdentifier:...` callback (which fires
    // synchronously from `insertItemWithItemIdentifier:atIndex:`) sees
    // the new records.
    {
        let mut items_ref = delegate.ivars().items.borrow_mut();
        *items_ref = new_records;
    }
    {
        let mut ids_ref = delegate.ivars().visible_identifiers.borrow_mut();
        *ids_ref = new_identifiers.clone();
    }

    // Remove every existing item, then insert the new identifier list
    // in order. `[NSToolbar items]` returns the current visible list;
    // remove from the end so indices stay valid. Both `removeItemAtIndex:`
    // and `insertItemWithItemIdentifier:atIndex:` take NSInteger
    // ('q' / signed), so the loop indices must be cast from `usize`
    // to `isize` before the msg_send to match the runtime encoding.
    let current_items: Retained<NSArray<NSObject>> =
        unsafe { msg_send_id![&**toolbar, items] };
    let current_count: usize = current_items.len();
    for idx in (0..current_count).rev() {
        let idx_signed: isize = idx as isize;
        let _: () = unsafe { msg_send![&**toolbar, removeItemAtIndex: idx_signed] };
    }
    for (idx, identifier) in new_identifiers.iter().enumerate() {
        let idx_signed: isize = idx as isize;
        let _: () = unsafe {
            msg_send![
                &**toolbar,
                insertItemWithItemIdentifier: &**identifier,
                atIndex: idx_signed,
            ]
        };
    }
}

// =========================================================================
// Imperative ops
// =========================================================================

struct MacosToolbarOps;

impl ToolbarOps for MacosToolbarOps {
    fn set_visible(&self, node: &dyn Any, visible: bool) {
        // The node is the placeholder MacosNode::View. Walk up to
        // `window.toolbar` and toggle it there — the NSToolbar
        // outlives the placeholder, so the window's slot is the
        // canonical source.
        let Some(MacosNode::View(view)) = node.downcast_ref::<MacosNode>() else {
            return;
        };
        let window_ptr: *mut NSObject = unsafe { msg_send![view, window] };
        if window_ptr.is_null() {
            return;
        }
        let toolbar_ptr: *mut NSObject = unsafe { msg_send![window_ptr, toolbar] };
        if toolbar_ptr.is_null() {
            return;
        }
        let _: () = unsafe { msg_send![toolbar_ptr, setVisible: visible] };
    }
}

// =========================================================================
// Tests — the pure item translation. The NSToolbar/delegate/window
// machinery is main-thread AppKit and has no unit-test seam (AppKit
// objects can only be constructed on the main thread — the same
// limitation every backend-macos imp/ module documents); the
// toolbar-demo example is the live gate for that path.
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Identity wrap — isolates the translation from the flush wrap.
    fn identity(cb: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
        cb
    }

    /// Pins the item-kind → identifier mapping (button prefix +
    /// counter, separator/space → system space, flexible → system
    /// flexible space) and that ONLY buttons produce records.
    #[test]
    fn plan_items_maps_kinds_to_identifiers_and_records() {
        let plan = plan_items(
            vec![
                ToolbarItem::button("Save")
                    .icon("square.and.arrow.down")
                    .tooltip("Save the document")
                    .into(),
                ToolbarItem::separator(),
                ToolbarItem::space(),
                ToolbarItem::flexible_space(),
                ToolbarItem::button("Reload").into(),
            ],
            &identity,
        );

        assert_eq!(
            plan.identifiers,
            vec![
                "com.idealyst.toolbar.item.0",
                "NSToolbarSpaceItem",
                "NSToolbarSpaceItem",
                "NSToolbarFlexibleSpaceItem",
                "com.idealyst.toolbar.item.1",
            ],
            "identifier order must follow author order; button counter skips spacers"
        );
        assert_eq!(plan.records.len(), 2, "only buttons produce records");
        assert_eq!(plan.records[0].identifier, "com.idealyst.toolbar.item.0");
        assert_eq!(plan.records[0].label, "Save");
        assert_eq!(plan.records[0].icon.as_deref(), Some("square.and.arrow.down"));
        assert_eq!(plan.records[0].tooltip.as_deref(), Some("Save the document"));
        assert!(plan.records[0].on_click.is_none(), "no handler → inert button");
        assert_eq!(plan.records[1].identifier, "com.idealyst.toolbar.item.1");
        assert!(plan.records[1].icon.is_none());
    }

    /// The `wrap_click` discipline is applied to every button handler
    /// as it enters the plan — this is the seam the handler uses to
    /// append `schedule_flush` after author clicks.
    #[test]
    fn plan_items_routes_button_clicks_through_wrap_click() {
        let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let log_author = log.clone();
        let log_wrap = log.clone();
        let wrap = move |cb: Rc<dyn Fn()>| -> Rc<dyn Fn()> {
            let log = log_wrap.clone();
            Rc::new(move || {
                cb();
                log.borrow_mut().push("after-click-hook");
            })
        };
        let plan = plan_items(
            vec![ToolbarItem::button("Go")
                .on_click(move || log_author.borrow_mut().push("author"))
                .into()],
            &wrap,
        );

        let on_click = plan.records[0].on_click.clone().expect("handler kept");
        on_click();
        assert_eq!(
            *log.borrow(),
            vec!["author", "after-click-hook"],
            "wrap must run AROUND the author callback (author first)"
        );
    }

    /// The system identifiers the delegate must decline to vend
    /// (NSToolbar substitutes the system item automatically).
    #[test]
    fn system_identifiers_are_recognized() {
        assert!(is_system_identifier("NSToolbarSpaceItem"));
        assert!(is_system_identifier("NSToolbarFlexibleSpaceItem"));
        assert!(is_system_identifier("NSToolbarSeparatorItem"));
        assert!(!is_system_identifier("com.idealyst.toolbar.item.0"));
    }
}
