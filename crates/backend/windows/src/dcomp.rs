//! DirectComposition tree for `Element::Graphics` surfaces.
//!
//! A graphics surface is an `IDCompositionVisual` in a composition
//! target over the host window — NOT a child HWND. The distinction is
//! the whole point: moving a child HWND (`SetWindowPos`) recomposes on
//! the window manager's schedule, so during fast scrolling the
//! swapchain visibly detached from the painted scene around it (and
//! `SetWindowRgn` rounded-corner clips forced a child repaint per
//! scroll tick). A visual moves by `SetOffsetX/Y` + `Commit`, which
//! the DWM applies atomically with its next composition frame, and
//! clips with an antialiased `IDCompositionRectangleClip` — no repaint,
//! no drift-by-construction. This is the same architecture browsers
//! use to place GPU-composited content inside scrolling pages.
//!
//! wgpu mounts directly on the visual
//! (`SurfaceTargetUnsafe::CompositionVisual`): it AddRefs the visual,
//! creates the swapchain via `CreateSwapChainForComposition`, and calls
//! `SetContent` on it — but never `Commit` (it doesn't have our
//! device), which is why [`ComposedTarget::commit`] exists for hosts
//! to call after configuring (see `runtime-core`'s trait docs).
//!
//! The device is created with a NULL rendering device
//! (`DCompositionCreateDevice2(None)`): visuals + targets don't need
//! one; only DComp *surfaces* would, and our content is always a DXGI
//! swapchain bound by wgpu. (Same call shape wgpu-hal itself uses for
//! its internal DComp path.)

use windows::core::IUnknown;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionClip, IDCompositionDevice, IDCompositionTarget,
    IDCompositionVisual,
};

/// The per-window composition tree: one device + one target on the
/// host HWND + a root visual every graphics visual parents under.
/// Created lazily by the first `create_graphics`.
pub(crate) struct CompositionTree {
    pub(crate) device: IDCompositionDevice,
    /// Kept alive for the tree's lifetime; unused after `SetRoot`.
    _target: IDCompositionTarget,
    root: IDCompositionVisual,
}

impl CompositionTree {
    /// Build the device/target/root chain, or `None` if any COM call
    /// fails (logged — there is no fallback path; DComp ships in every
    /// supported Windows version, so failure means something is deeply
    /// wrong with the session).
    ///
    /// `topmost = true` on the target: the visual tree composes above
    /// the host's child windows, matching the old z-order where the
    /// canvas HWND was created after (above) the native controls. The
    /// GDI-painted scene lives on the window's redirection surface,
    /// which composes below the target's visuals either way.
    pub(crate) fn new(host_hwnd: HWND) -> Option<Self> {
        unsafe {
            let device: IDCompositionDevice =
                match DCompositionCreateDevice2(None::<&IUnknown>) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[backend-windows] DCompositionCreateDevice2 failed: {e}");
                        return None;
                    }
                };
            let target = match device.CreateTargetForHwnd(host_hwnd, true) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[backend-windows] CreateTargetForHwnd failed: {e}");
                    return None;
                }
            };
            let root = match device.CreateVisual() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[backend-windows] CreateVisual (root) failed: {e}");
                    return None;
                }
            };
            if let Err(e) = target.SetRoot(&root) {
                eprintln!("[backend-windows] IDCompositionTarget::SetRoot failed: {e}");
                return None;
            }
            let _ = device.Commit();
            Some(Self { device, _target: target, root })
        }
    }

    /// Create the two-level visual chain for one graphics node,
    /// parented under the root:
    ///
    /// ```text
    /// root ─ container (offset 0,0; SQUARE clip: viewport/overflow)
    ///          └─ content (offset = node abs; ROUNDED clip: bezel)
    /// ```
    ///
    /// Two levels because a DComp visual carries ONE clip, but a
    /// surface can be cut by two independent shapes at once: the
    /// nearest rounded ancestor (whose corners ride WITH the content)
    /// and the scroll viewport (whose straight edge stays fixed to
    /// the window). Collapsing them into a single rounded rect put
    /// rounded corners on the scroll cut line — the "bezel corners
    /// follow the viewport edge" bug. The container stays at (0,0) so
    /// both clips are written in root coordinates (a visual's clip
    /// lives in its PARENT's coordinate space).
    pub(crate) fn add_visual(&self) -> Option<VisualPair> {
        unsafe {
            let container = self.device.CreateVisual().ok()?;
            let content = self.device.CreateVisual().ok()?;
            container.AddVisual(&content, true, None::<&IDCompositionVisual>).ok()?;
            self.root.AddVisual(&container, true, None::<&IDCompositionVisual>).ok()?;
            Some(VisualPair { container, content })
        }
    }

    /// Detach a node's visual chain from the tree (node teardown). The
    /// caller commits; wgpu's own reference keeps the content visual
    /// alive until the author drops their surface.
    pub(crate) fn remove_visual(&self, v: &VisualPair) {
        unsafe {
            let _ = self.root.RemoveVisual(&v.container);
        }
    }

    pub(crate) fn commit(&self) {
        unsafe {
            if let Err(e) = self.device.Commit() {
                eprintln!("[backend-windows] IDCompositionDevice::Commit failed: {e}");
            }
        }
    }

    /// Write a [`Placement`] onto a node's visual chain. The caller
    /// batches `commit()` after applying every changed placement.
    pub(crate) fn apply_placement(&self, v: &VisualPair, p: &Placement) {
        unsafe {
            // Square clip (viewport / overflow) on the container —
            // fixed in window space, cuts straight edges.
            match p.square {
                None => {
                    let _ = v.container.SetClip(None::<&IDCompositionClip>);
                }
                Some((l, t, r, b)) => {
                    let rect = D2D_RECT_F { left: l, top: t, right: r, bottom: b };
                    if let Err(e) = v.container.SetClip2(&rect) {
                        eprintln!("[backend-windows] SetClip2(square) failed: {e}");
                    }
                }
            }
            // Offset + rounded clip (bezel) on the content — both in
            // root coords (container sits at 0,0), so the corners ride
            // with the content as it scrolls.
            let rx = v.content.SetOffsetX2(p.x as f32);
            let ry = v.content.SetOffsetY2(p.y as f32);
            eprintln!(
                "[dcomp-debug] apply x={} y={} ox={:?} oy={:?} square={:?} rounded={:?}",
                p.x, p.y, rx, ry, p.square, p.rounded
            );
            match p.rounded {
                None => {
                    let _ = v.content.SetClip(None::<&IDCompositionClip>);
                }
                Some((l, t, r, b, radius)) => {
                    // Antialiased rounded clip — replaces the aliased
                    // `CreateRoundRectRgn` the HWND path used.
                    if let Ok(clip) = self.device.CreateRectangleClip() {
                        let _ = clip.SetLeft2(l);
                        let _ = clip.SetTop2(t);
                        let _ = clip.SetRight2(r);
                        let _ = clip.SetBottom2(b);
                        let _ = clip.SetTopLeftRadiusX2(radius);
                        let _ = clip.SetTopLeftRadiusY2(radius);
                        let _ = clip.SetTopRightRadiusX2(radius);
                        let _ = clip.SetTopRightRadiusY2(radius);
                        let _ = clip.SetBottomLeftRadiusX2(radius);
                        let _ = clip.SetBottomLeftRadiusY2(radius);
                        let _ = clip.SetBottomRightRadiusX2(radius);
                        let _ = clip.SetBottomRightRadiusY2(radius);
                        // Failures logged, not ignored: a silently
                        // retained stale clip renders as a phantom
                        // rounded cut pinned mid-scene — nearly
                        // undiagnosable from the visual alone.
                        if let Err(e) = v.content.SetClip(&clip) {
                            eprintln!("[backend-windows] SetClip(rounded) failed: {e}");
                        }
                    } else {
                        eprintln!("[backend-windows] CreateRectangleClip failed");
                    }
                }
            }
        }
    }
}

/// The per-node visual chain — see [`CompositionTree::add_visual`].
/// `content` is what wgpu binds the swapchain to.
pub(crate) struct VisualPair {
    pub(crate) container: IDCompositionVisual,
    pub(crate) content: IDCompositionVisual,
}

// =========================================================================
// Placement — pure geometry, unit-tested
// =========================================================================

/// The clip state accumulated down the positioning walk, kept as TWO
/// channels because they behave differently under scrolling:
///
/// - `square`: the intersection of every square-cornered clipper
///   (scroll viewports, `overflow: hidden` boxes). Fixed in window
///   space — a surface scrolling past it is cut with a straight edge.
/// - `rounded`: the NEAREST rounded clipper's own rect + radius. Rides
///   with the content — its corners stay on the bezel, never on the
///   viewport edge. An outer rounded clipper displaced by an inner one
///   demotes to the square channel (its bounds still apply; only its
///   corner rounding is approximated away — same nearest-radius-wins
///   approximation the region path always had).
#[derive(Clone, Copy, Default)]
pub(crate) struct ClipChain {
    pub square: Option<(f32, f32, f32, f32)>,
    pub rounded: Option<((f32, f32, f32, f32), f32)>,
}

fn isect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3))
}

impl ClipChain {
    /// Fold one clipping ancestor (rect as `(l, t, r, b)` in root
    /// coords, its max corner radius) into the chain.
    pub(crate) fn push(self, rect: (f32, f32, f32, f32), radius: f32) -> Self {
        let mut square = self.square;
        let mut rounded = self.rounded;
        if radius < 0.5 {
            square = Some(match square {
                Some(s) => isect(s, rect),
                None => rect,
            });
        } else {
            if let Some((prev, _)) = rounded {
                square = Some(match square {
                    Some(s) => isect(s, prev),
                    None => prev,
                });
            }
            rounded = Some((rect, radius));
        }
        Self { square, rounded }
    }

    /// The single-clip form the HWND region path consumes:
    /// all rects intersected, the rounded radius if one is present.
    pub(crate) fn legacy(&self) -> Option<((f32, f32, f32, f32), f32)> {
        match (self.square, self.rounded) {
            (None, None) => None,
            (Some((l, t, r, b)), None) => Some(((l, t, r - l, b - t), 0.0)),
            (None, Some(((l, t, r, b), rad))) => Some(((l, t, r - l, b - t), rad)),
            (Some(s), Some((rr, rad))) => {
                let (l, t, r, b) = isect(s, rr);
                Some(((l, t, r - l, b - t), rad))
            }
        }
    }
}

/// Where a graphics surface's visual chain sits and how it's clipped.
/// Diffed against the last applied placement so an unchanged layout
/// pass writes nothing (and triggers no commit).
///
/// Both clip rects are `(left, top, right, bottom)` in ROOT-VISUAL
/// coordinates (= host client coordinates): a DComp visual's clip is
/// specified in its PARENT's coordinate space, and both levels of the
/// chain resolve to root space (the container sits at 0,0).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Placement {
    pub x: i32,
    pub y: i32,
    /// Container clip: viewport/overflow intersection. `None` = none.
    pub square: Option<(f32, f32, f32, f32)>,
    /// Content clip: nearest rounded ancestor's rect + corner radius.
    pub rounded: Option<(f32, f32, f32, f32, f32)>,
}

/// Compute a surface's placement from its absolute frame, the walk's
/// accumulated [`ClipChain`], and the subtree-hidden flag.
///
/// - Hidden (portal-hidden ancestor): empty square clip — the chain
///   stays in the tree with content bound, it just renders nothing.
///   (Cheaper and simpler than detach/reattach, and the host
///   independently skips rendering via `ComposedTarget::is_visible`.)
/// - A square intersection that fully contains the frame clips
///   nothing → `None`. Empty intersection → empty clip.
/// - The rounded clip is passed through UNINTERSECTED — cutting it to
///   the viewport is exactly what moved the bezel's corners onto the
///   scroll edge.
pub(crate) fn visual_placement(
    abs: (f32, f32, f32, f32),
    chain: ClipChain,
    hidden: bool,
) -> Placement {
    let (ax, ay, w, h) = abs;
    let x = ax.round() as i32;
    let y = ay.round() as i32;
    if hidden {
        return Placement { x, y, square: Some((0.0, 0.0, 0.0, 0.0)), rounded: None };
    }
    let square = match chain.square {
        None => None,
        Some((cl, ct, cr, cb)) => {
            let l = cl.max(ax);
            let t = ct.max(ay);
            let r = cr.min(ax + w);
            let b = cb.min(ay + h);
            if l >= r || t >= b {
                // Fully scrolled out of the clipping ancestor.
                Some((0.0, 0.0, 0.0, 0.0))
            } else if l <= ax && t <= ay && r >= ax + w && b >= ay + h {
                // Covers the whole frame — clips nothing.
                None
            } else {
                Some((l, t, r, b))
            }
        }
    };
    let rounded = chain.rounded.map(|((l, t, r, b), rad)| (l, t, r, b, rad));
    Placement { x, y, square, rounded }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(clips: &[((f32, f32, f32, f32), f32)]) -> ClipChain {
        clips
            .iter()
            .fold(ClipChain::default(), |c, (rect, rad)| c.push(*rect, *rad))
    }

    /// Port of the `hwnd_clip_region` decision table to the visual
    /// world: full-cover square clips vanish, partial overlaps clip to
    /// the intersection, empty intersections clip everything.
    #[test]
    fn full_cover_square_clip_is_none() {
        let p = visual_placement(
            (100.0, 200.0, 300.0, 300.0),
            chain(&[((0.0, 0.0, 800.0, 800.0), 0.0)]),
            false,
        );
        assert_eq!(p, Placement { x: 100, y: 200, square: None, rounded: None });
    }

    #[test]
    fn partial_scroll_clip_intersects_in_root_coords() {
        // Canvas half scrolled above a viewport spanning y 120..520.
        let p = visual_placement(
            (60.0, 20.0, 300.0, 300.0),
            chain(&[((60.0, 120.0, 360.0, 520.0), 0.0)]),
            false,
        );
        assert_eq!(p.square, Some((60.0, 120.0, 360.0, 320.0)));
        assert_eq!(p.rounded, None);
    }

    #[test]
    fn fully_scrolled_out_clips_everything() {
        let p = visual_placement(
            (60.0, -400.0, 300.0, 300.0),
            chain(&[((60.0, 120.0, 360.0, 520.0), 0.0)]),
            false,
        );
        assert_eq!(p.square, Some((0.0, 0.0, 0.0, 0.0)));
    }

    /// The simulator-bezel case: a rounded ancestor keeps its clip
    /// even when it fully contains the frame — the corners still cut.
    #[test]
    fn regression_rounded_full_cover_clip_is_kept() {
        let p = visual_placement(
            (100.0, 100.0, 300.0, 640.0),
            chain(&[((100.0, 100.0, 400.0, 740.0), 42.0)]),
            false,
        );
        assert_eq!(p.rounded, Some((100.0, 100.0, 400.0, 740.0, 42.0)));
        assert_eq!(p.square, None);
    }

    /// THE user-reported bug this split exists for: a rounded bezel
    /// inside a scroll viewport, phone half scrolled above the
    /// viewport top. The old single-clip form applied the bezel radius
    /// to the viewport intersection, putting rounded corners on the
    /// scroll cut line. Correct: the SQUARE clip is the viewport
    /// (straight edge, fixed to the window) and the ROUNDED clip is
    /// the bezel's own rect (uncut — its corners ride with the
    /// content, off-screen here).
    #[test]
    fn regression_scroll_cut_is_straight_while_bezel_corners_stay_rounded() {
        // Viewport y 100..700; bezel scrolled up so it spans y -80..569.
        let ch = chain(&[
            ((260.0, 100.0, 1024.0, 700.0), 0.0), // page scroll viewport
            ((640.0, -80.0, 960.0, 569.0), 42.0), // rounded bezel wrapper
        ]);
        let p = visual_placement((650.0, -70.0, 300.0, 629.0), ch, false);
        assert_eq!(
            p.square,
            Some((650.0, 100.0, 950.0, 559.0)),
            "square clip = viewport ∩ frame — the straight cut"
        );
        assert_eq!(
            p.rounded,
            Some((640.0, -80.0, 960.0, 569.0, 42.0)),
            "rounded clip = bezel's OWN rect, not intersected with the viewport"
        );
    }

    /// An outer rounded clipper displaced by an inner one demotes to
    /// the square channel: its bounds still cut, only its corner
    /// rounding is approximated away.
    #[test]
    fn nested_rounded_keeps_innermost_and_demotes_outer_to_square() {
        let ch = chain(&[
            ((0.0, 0.0, 500.0, 500.0), 20.0),  // outer rounded card
            ((50.0, 50.0, 450.0, 450.0), 8.0), // inner rounded wrapper
        ]);
        assert_eq!(ch.square, Some((0.0, 0.0, 500.0, 500.0)));
        assert_eq!(ch.rounded, Some(((50.0, 50.0, 450.0, 450.0), 8.0)));
    }

    /// The HWND region path still consumes the single-clip form —
    /// all rects intersected, rounded radius carried.
    #[test]
    fn legacy_form_matches_old_single_clip_shape() {
        let ch = chain(&[
            ((0.0, 100.0, 400.0, 700.0), 0.0),
            ((50.0, 50.0, 350.0, 650.0), 42.0),
        ]);
        assert_eq!(ch.legacy(), Some(((50.0, 100.0, 300.0, 550.0), 42.0)));
        assert_eq!(ClipChain::default().legacy(), None);
    }

    /// Portal-hidden subtree: the chain keeps its offset (cheap
    /// re-show) but renders nothing.
    #[test]
    fn regression_portal_hidden_visual_renders_nothing() {
        let p = visual_placement((100.0, 100.0, 300.0, 640.0), ClipChain::default(), true);
        assert_eq!(
            p,
            Placement { x: 100, y: 100, square: Some((0.0, 0.0, 0.0, 0.0)), rounded: None }
        );
    }
}
