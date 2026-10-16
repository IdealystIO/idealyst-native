//! Platform-native render introspection for the Win32 backend — the Windows
//! half of cross-platform render parity.
//!
//! `caps::IntrospectionOps` was an empty impl on `WindowsBackend`, so
//! `introspect_native` returned `None` for every element and a parity capture
//! against this backend came back empty: every element reported as "only on
//! the other platform". Windows could not take part in parity testing at all.
//!
//! # The cardinal rule, and how far this backend can honor it
//!
//! `runtime_shared::introspect` asks for values read from the LIVE platform
//! object, never echoed from the framework's style structs. This backend
//! splits in two, and the reader is explicit about which half a value comes
//! from:
//!
//! - **Native controls** (button / edit / checkbox / trackbar / progress, and
//!   SDK external HWNDs) are real Win32 windows, and every value is a Win32
//!   query: `GetWindowRect` mapped into the host's client space,
//!   `GetClassNameW`, `GetWindowTextW`, `IsWindowVisible`, and the control's
//!   font as GDI realized it (`WM_GETFONT` → `GetTextFaceW` /
//!   `GetTextMetricsW`).
//! - **Painted nodes** (view / text / icon / image / code) have no native
//!   object: the host window is ONE GDI+ surface the scene painter draws into
//!   (`scene.rs` explains why). There is no independent engine to ask, so the
//!   reader reports what the PAINTER draws — the same position the GTK reader
//!   is in for its custom view's fill. Geometry is the box under the
//!   painter's own transform composition ([`crate::painted_geom`], driven by
//!   the same `transform_ops` list the painter feeds GDI+), so a transform, a
//!   scroll offset or a stale frame shows up; it is not the Taffy frame
//!   echoed. Fill / radius / border are the painter's resolved visual state
//!   (post-token, post-state, radius clamped the way the path builder clamps
//!   it). The text FONT is read from the GDI+ font object the painter draws
//!   with (family / size) and GDI's realized metrics (weight) — a real
//!   platform read, which is what catches a missing face silently replaced
//!   by a fallback. A bug where the visual state is right and the pixels are
//!   wrong is out of reach of any of this; the `screenshot` verb is the tool
//!   for that.
//!
//! # Element boundaries
//!
//! Every backend node is a framework element (one node per primitive), so
//! each element's native tree is a single node and `note_introspection_root`
//! has nothing to mark.

use runtime_shared::introspect::{keys, NativeNode, NativeRect, NativeValue};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, GetTextFaceW, GetTextMetricsW, MapWindowPoints, SelectObject,
    HFONT, HGDIOBJ, TEXTMETRICW,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipDeleteFontFamily, GdipGetFamily, GdipGetFamilyName, GdipGetFontSize, GpFont, GpFontFamily,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    SendMessageW, WM_GETFONT,
};

use crate::painted_geom::{painted_radius, Affine};
use crate::{font, NodeKind, WindowsBackend};

impl WindowsBackend {
    /// Read the native render tree for one element. Backs
    /// `IntrospectionOps::introspect_native`.
    ///
    /// `None` only when the id names no live node. A node that exists is
    /// always reported, even at 0x0: dropping it would read as "this element
    /// exists on the other platform only", a worse lie than an honest zero.
    pub(crate) fn introspect_native_impl(&self, id: u64) -> Option<NativeNode> {
        let meta = self.nodes.get(&id)?;
        let mut node = match &meta.kind {
            NodeKind::Control { hwnd } | NodeKind::External { hwnd } => self.read_control(*hwnd),
            _ => {
                let frame = self.painted_rect(id).unwrap_or(NativeRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                });
                let mut n = NativeNode::leaf(painted_class(&meta.kind), frame);
                n.set(keys::HIDDEN, Some(NativeValue::Flag(meta.hidden)));
                // Radii/border are planned in the node's OWN space, against
                // its frame size — not the transformed bounds.
                let (_, _, w, h) = meta.frame;
                self.read_painted(&meta.kind, w, h, &mut n);
                n
            }
        };
        node.set(keys::OPACITY, Some(NativeValue::Number(meta.effective_opacity())));
        // Which primitive this backs is framework knowledge, not a platform
        // read — attached as the role hint only.
        node.role = Some(node_role(&meta.kind).to_string());
        Some(node)
    }

    /// The window-space box the painter draws node `id` in, in logical px.
    /// `None` when the node is not reachable from the mounted root.
    fn painted_rect(&self, id: u64) -> Option<NativeRect> {
        // Private-layer content sits in its popup at the same client
        // coordinates, so its roots are searched the same way.
        let (x, y, width, height) = crate::private_layer::all_roots(self)
            .find_map(|root| self.find_painted(root, Affine::IDENTITY, id))?;
        Some(NativeRect { x, y, width, height })
    }

    /// Depth-first walk mirroring `scene::paint_node`'s transform stack:
    /// `parent` is the transform the node's parent hands its children.
    fn find_painted(&self, node: u64, parent: Affine, target: u64) -> Option<(f32, f32, f32, f32)> {
        let meta = self.nodes.get(&node)?;
        let (fx, fy, w, h) = meta.frame;
        let ops = crate::scene::transform_ops(w, h, &meta.author_transform, &meta.anim);
        let m = parent.node(fx, fy, &ops);
        if node == target {
            return Some(m.bounds(w, h));
        }
        let child_m = match &meta.kind {
            NodeKind::View(v) => match &v.scroll {
                Some(s) => m.then_translate(-s.offset_x, -s.offset_y),
                None => m,
            },
            _ => m,
        };
        self.children
            .get(&node)?
            .iter()
            .find_map(|c| self.find_painted(*c, child_m, target))
    }

    /// Fill / radius / border / text props from the painter's live visual.
    fn read_painted(&self, kind: &NodeKind, w: f32, h: f32, n: &mut NativeNode) {
        match kind {
            NodeKind::View(v) => {
                n.set(keys::BACKGROUND_COLOR, v.effective_background().map(NativeValue::Color));
                if let Some(r) = painted_radius(v.radii, w, h).filter(|r| *r > 0.0) {
                    n.set(keys::CORNER_RADIUS, Some(NativeValue::Length(r)));
                }
                // The TOP side, as the GTK reader and the web reader
                // (`border-top-width`) both report — a single canonical key
                // cannot carry four sides, and two readers must answer the
                // same question.
                let top = &v.borders[0];
                if top.width > 0.0 {
                    n.set(keys::BORDER_WIDTH, Some(NativeValue::Length(top.width)));
                    if let Some(c) = top.color {
                        n.set(keys::BORDER_COLOR, Some(NativeValue::Color(c.to_srgb_f32())));
                    }
                    // The pattern the painter actually draws: `border_pattern::plan`
                    // returns marks only for a dashed/dotted style it can lay
                    // out; otherwise `paint_border` strokes solid.
                    let patterned =
                        crate::border_pattern::plan(&v.borders, v.radii, v.border_style, w, h).is_some();
                    let style = if patterned { border_keyword(v.border_style) } else { "solid" };
                    n.set(keys::BORDER_STYLE, Some(NativeValue::Text(style.to_string())));
                }
            }
            NodeKind::Text(t) => {
                n.set(keys::TEXT, Some(NativeValue::Text(t.content.clone())));
                let color = t.anim_color.unwrap_or_else(|| t.color.to_srgb_f32());
                n.set(keys::TEXT_COLOR, Some(NativeValue::Color(color)));
                let key = t.font_key.clone().unwrap_or_else(|| self.default_font_key.clone());
                self.read_font(&key, n);
            }
            NodeKind::Code(c) => {
                let text: Vec<String> = c
                    .lines
                    .iter()
                    .map(|line| line.iter().map(|r| String::from_utf16_lossy(&r.text16)).collect())
                    .collect();
                n.set(keys::TEXT, Some(NativeValue::Text(text.join("\n"))));
                if let Some(first) = c.lines.iter().flatten().next() {
                    n.set(keys::TEXT_COLOR, Some(NativeValue::Color(first.color.to_srgb_f32())));
                }
                self.read_font(&c.font_key, n);
            }
            // An icon's colour under the SAME key the web reader fills from
            // CSS `color` — what an icon's `currentColor` resolves to.
            NodeKind::Icon(p) => {
                n.set(keys::TEXT_COLOR, Some(NativeValue::Color(p.color.to_srgb_f32())));
            }
            NodeKind::Image(_) | NodeKind::Control { .. } | NodeKind::External { .. } => {}
        }
    }

    /// Family + size from the GDI+ font the painter draws this key with,
    /// weight from GDI's realized metrics for the same font.
    fn read_font(&self, key: &font::FontKey, n: &mut NativeNode) {
        // The Direct2D renderer measures and draws with DirectWrite; the
        // GDI font cache is never filled for it.
        if let Some(dw) = &self.dwrite {
            if let Some(r) = dw.realized(key) {
                n.set(keys::FONT_FAMILY, Some(NativeValue::Text(r.family)));
                n.set(keys::FONT_SIZE, Some(NativeValue::Length(r.size)));
                n.set(keys::FONT_WEIGHT, Some(NativeValue::Number(r.weight as f32)));
            }
            return;
        }
        let Some(entry) = self.font_cache.get(key) else { return };
        if !entry.gpfont.is_null() {
            if let Some(family) = unsafe { gdiplus_family_name(entry.gpfont) } {
                n.set(keys::FONT_FAMILY, Some(NativeValue::Text(family)));
            }
            let mut size = 0.0f32;
            if unsafe { GdipGetFontSize(entry.gpfont, &mut size) }.0 == 0 && size > 0.0 {
                // The cache's fonts are LOGICAL-size (`UnitPixel` under the
                // painter's DPI scale), so this is already logical px.
                n.set(keys::FONT_SIZE, Some(NativeValue::Length(size)));
            }
        }
        if let Some((_face, metrics)) = unsafe { realized_font(entry.hfont) } {
            n.set(keys::FONT_WEIGHT, Some(NativeValue::Number(metrics.tmWeight as f32)));
            if entry.gpfont.is_null() {
                // Never painted yet (no GDI+ font): fall back to GDI's size.
                let em = (metrics.tmHeight - metrics.tmInternalLeading) as f32;
                n.set(keys::FONT_SIZE, Some(NativeValue::Length(em)));
            }
        }
    }

    /// A native control, read entirely through Win32.
    fn read_control(&self, hwnd: HWND) -> NativeNode {
        let scale = self.dpi.factor();
        let frame = unsafe { client_rect_of(hwnd, self.host_hwnd) }
            .map(|r| NativeRect {
                x: r.left as f32 / scale,
                y: r.top as f32 / scale,
                width: (r.right - r.left) as f32 / scale,
                height: (r.bottom - r.top) as f32 / scale,
            })
            .unwrap_or(NativeRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 });
        let class = unsafe { class_name(hwnd) }.unwrap_or_else(|| "HWND".to_string());
        let mut n = NativeNode::leaf(class, frame);
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        n.set(keys::HIDDEN, Some(NativeValue::Flag(!visible)));
        if let Some(text) = unsafe { window_text(hwnd) } {
            n.set(keys::TEXT, Some(NativeValue::Text(text)));
        }
        // The control's font as GDI realized it, in device px → logical.
        let hfont = HFONT(unsafe { SendMessageW(hwnd, WM_GETFONT, Some(WPARAM(0)), Some(LPARAM(0))) }.0 as _);
        if !hfont.is_invalid() {
            if let Some((face, m)) = unsafe { realized_font(hfont) } {
                n.set(keys::FONT_FAMILY, Some(NativeValue::Text(face)));
                let em = (m.tmHeight - m.tmInternalLeading) as f32 / scale;
                n.set(keys::FONT_SIZE, Some(NativeValue::Length(em)));
                n.set(keys::FONT_WEIGHT, Some(NativeValue::Number(m.tmWeight as f32)));
            }
        }
        n
    }
}

fn painted_class(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::View(_) => "IdealystView",
        NodeKind::Text(_) => "IdealystText",
        NodeKind::Code(_) => "IdealystCode",
        NodeKind::Icon(_) => "IdealystIcon",
        NodeKind::Image(_) => "IdealystImage",
        NodeKind::Control { .. } | NodeKind::External { .. } => "HWND",
    }
}

fn node_role(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::View(v) if v.scroll.is_some() => "scroll_view",
        NodeKind::View(v) if v.on_click.is_some() => "pressable",
        NodeKind::View(_) => "view",
        NodeKind::Text(_) | NodeKind::Code(_) => "text",
        NodeKind::Icon(_) => "icon",
        NodeKind::Image(_) => "image",
        NodeKind::Control { .. } => "control",
        NodeKind::External { .. } => "external",
    }
}

fn border_keyword(style: runtime_shared::BorderStyle) -> &'static str {
    match style {
        runtime_shared::BorderStyle::Dashed => "dashed",
        runtime_shared::BorderStyle::Dotted => "dotted",
        runtime_shared::BorderStyle::Solid => "solid",
    }
}

/// `hwnd`'s window rect in `host`'s client coordinates (device px).
unsafe fn client_rect_of(hwnd: HWND, host: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    GetWindowRect(hwnd, &mut r).ok()?;
    let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
    MapWindowPoints(None, Some(host), &mut pts);
    Some(RECT { left: pts[0].x, top: pts[0].y, right: pts[1].x, bottom: pts[1].y })
}

unsafe fn class_name(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut buf);
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

unsafe fn window_text(hwnd: HWND) -> Option<String> {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// The face GDI's mapper actually chose for `hfont`, and its realized
/// metrics — selected into a scratch DC, so a requested face the system
/// lacks reports the substitute, not the request.
unsafe fn realized_font(hfont: HFONT) -> Option<(String, TEXTMETRICW)> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return None;
    }
    let old = SelectObject(dc, HGDIOBJ(hfont.0));
    let mut face = [0u16; 64];
    let n = GetTextFaceW(dc, Some(&mut face));
    let mut tm = TEXTMETRICW::default();
    let ok = GetTextMetricsW(dc, &mut tm).as_bool();
    SelectObject(dc, old);
    let _ = DeleteDC(dc);
    // `GetTextFaceW` counts the terminating NUL.
    let face = String::from_utf16_lossy(&face[..(n.max(1) as usize - 1).min(face.len())]);
    ok.then_some((face, tm))
}

/// The family name of a GDI+ font — the family GDI+ is drawing with.
unsafe fn gdiplus_family_name(font: *mut GpFont) -> Option<String> {
    let mut family: *mut GpFontFamily = std::ptr::null_mut();
    if GdipGetFamily(font, &mut family).0 != 0 || family.is_null() {
        return None;
    }
    // LF_FACESIZE (32) is GDI+'s documented upper bound for a family name.
    let mut name = [0u16; 32];
    let status = GdipGetFamilyName(family, &mut name, 0);
    let _ = GdipDeleteFontFamily(family);
    if status.0 != 0 {
        return None;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RendererKind;
    use runtime_shared::{AccessibilityProps, Length, StyleRules};
    use std::rc::Rc;

    /// On the Direct2D renderer the GDI font cache is never filled, so the
    /// reader reported no font at all for painted text — a `--parity` run
    /// saw every Windows label as fontless.
    #[test]
    fn regression_d2d_text_reports_its_directwrite_font() {
        let _serial = crate::render_test_lock();
        let mut b = WindowsBackend::with_renderer(HWND(std::ptr::null_mut()), RendererKind::D2d);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let label = b.create_text("Label", &a11y);
        let mut st = StyleRules::default();
        st.font_size = Some(Length::Px(18.0).into());
        b.apply_style(&label, &Rc::new(st));
        let id = label.id;
        b.insert(&mut root, label);
        b.finish(root);
        let n = b.introspect_native_impl(id).expect("node");
        match n.props.get(keys::FONT_FAMILY) {
            Some(NativeValue::Text(f)) => assert!(!f.is_empty()),
            other => panic!("font family: {other:?}"),
        }
        assert!(matches!(n.props.get(keys::FONT_SIZE), Some(NativeValue::Length(s)) if *s == 18.0));
        assert!(matches!(n.props.get(keys::FONT_WEIGHT), Some(NativeValue::Number(w)) if *w == 400.0));
    }
}
