//! Where the scene painter puts a node — the geometry half of the native
//! introspection reader (`introspect.rs`).
//!
//! The painter positions each node by prepending ops onto a GDI+ world
//! transform as it recurses (`scene::paint_node`): the node's frame offset,
//! then its [`XOp`] chain, then — for its children — a scroll offset. This
//! module is that same composition as plain 2D affine maths, so the reader
//! reports the box the painter ACTUALLY draws (transforms, scroll offsets and
//! all), and so the composition can be tested on any host — the crate body is
//! `cfg(windows)`, this module is not (see `tests/painted_geom.rs`).
//!
//! Pure: no Win32, no backend state.

/// One resolved world-transform op, in application order. Built by
/// `scene::transform_ops`; consumed by the painter (as GDI+ calls) and by
/// [`Affine::then_op`] (as maths) — one op list, two readers.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum XOp {
    Translate(f32, f32),
    Scale(f32, f32),
    /// Degrees, clockwise on screen (y grows down) — GDI+'s convention.
    Rotate(f32),
}

// The maths below is used only by the `robot`-gated introspection reader;
// `XOp` above is the painter's, and always live.
/// A 2D affine map `p ↦ (a·x + c·y + e, b·x + d·y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(feature = "robot"), allow(dead_code))]
pub(crate) struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

#[cfg_attr(not(feature = "robot"), allow(dead_code))]
impl Affine {
    pub(crate) const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub(crate) fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// `self ∘ other` — `other` maps the point first. This is exactly what
    /// GDI+'s `MatrixOrderPrepend` does to the world transform, which is how
    /// the painter applies every op.
    fn compose(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    pub(crate) fn then_translate(&self, dx: f32, dy: f32) -> Affine {
        self.compose(&Affine { e: dx, f: dy, ..Affine::IDENTITY })
    }

    /// Prepend one op, as `GdipTranslate/Scale/RotateWorldTransform(…,
    /// MatrixOrderPrepend)` would.
    pub(crate) fn then_op(&self, op: XOp) -> Affine {
        match op {
            XOp::Translate(dx, dy) => self.then_translate(dx, dy),
            XOp::Scale(sx, sy) => self.compose(&Affine { a: sx, d: sy, ..Affine::IDENTITY }),
            XOp::Rotate(deg) => {
                let (s, c) = deg.to_radians().sin_cos();
                self.compose(&Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 })
            }
        }
    }

    /// The transform a node paints its own box under, given its parent's
    /// child transform: frame offset, then the node's op chain.
    pub(crate) fn node(&self, frame_x: f32, frame_y: f32, ops: &[XOp]) -> Affine {
        ops.iter().fold(self.then_translate(frame_x, frame_y), |m, op| m.then_op(*op))
    }

    /// Axis-aligned bounds of the `w × h` box under this map, as
    /// `(x, y, width, height)`. Exact for translate/scale; the enclosing box
    /// for a rotation (what a screen reader of the painted pixels would see).
    pub(crate) fn bounds(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let pts = [self.apply(0.0, 0.0), self.apply(w, 0.0), self.apply(0.0, h), self.apply(w, h)];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg_attr(not(feature = "robot"), allow(dead_code))]
/// The corner radius the painter draws for the box: the first non-zero
/// corner (`[tl, tr, br, bl]`), clamped to half the shorter side exactly as
/// `scene::build_round_rect_path` clamps it. `None` for a square box.
pub(crate) fn painted_radius(radii: [f32; 4], w: f32, h: f32) -> Option<f32> {
    if radii.iter().all(|r| *r < 0.5) {
        return None;
    }
    let rmax = (w.min(h) / 2.0).max(0.0);
    radii.iter().copied().find(|r| *r > 0.0).map(|r| r.clamp(0.0, rmax))
}

/// A leaf's content box inside its frame: `(x, y, width, height)` for a
/// `w × h` frame with padding `inset = [top, right, bottom, left]`. Never
/// negative — padding wider than the frame leaves an empty box at the
/// padding's start, as CSS does.
pub(crate) fn content_box(w: f32, h: f32, inset: [f32; 4]) -> (f32, f32, f32, f32) {
    let [top, right, bottom, left] = inset;
    (left, top, (w - left - right).max(0.0), (h - top - bottom).max(0.0))
}
