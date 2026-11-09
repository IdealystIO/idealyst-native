//! Native measure probes, asked once per layout pass.
//!
//! A text leaf (and an external view) sizes itself through
//! `View.measure`, called from its Taffy measure_fn. Flex resolution asks a
//! leaf for its size several times in one pass: min-content and
//! max-content contributions, the flex basis, then the final size at the
//! resolved width. Many of those questions reach Android as the SAME
//! `MeasureSpec` pair, because Taffy's known-width and definite-available
//! cases both become `AT_MOST width`. Taffy's own cache is keyed on its
//! inputs, not on the spec, so it misses them. A freshly mounted
//! `TextView` cannot answer from Android's measure cache either: `setText`
//! requested a layout, and the request stands until the view is laid out.
//! Each probe was therefore a full text layout, about 8 per text leaf in a
//! mounted list.
//!
//! [`PassMemo`] answers a repeated question from the first answer. It is
//! only valid while a pass runs: nothing that changes a view's content runs
//! inside Taffy's compute, but between passes the text, the font or the
//! style can all change. So the memo is opened for the pass
//! (`AndroidBackend::compute_layout_roots`), cleared when it closes, and
//! outside a pass every probe goes to the view.
//!
//! Un-gated so the tests run on the host; the JNI half is
//! `imp::probe_measure` and `primitives::text`.

use std::collections::HashMap;
use std::hash::Hash;

/// `View.MeasureSpec.UNSPECIFIED`: no bound on the axis.
pub(crate) const UNSPECIFIED: i32 = 0;
/// `View.MeasureSpec.EXACTLY` (`1 << 30`).
pub(crate) const EXACTLY: i32 = 0x4000_0000;
/// `View.MeasureSpec.AT_MOST` (`2 << 30`, the i32 sign bit).
pub(crate) const AT_MOST: i32 = -0x8000_0000;
/// The size bits of a `MeasureSpec`.
const SIZE_MASK: i32 = 0x3fff_ffff;

/// `AT_MOST` the width in px, or `UNSPECIFIED` for an unbounded (non-finite)
/// width: the spec a text leaf is measured with.
pub(crate) fn at_most_or_unspecified(dp: f32, density: f32) -> i32 {
    if dp.is_finite() {
        AT_MOST | ((dp * density).round() as i32 & SIZE_MASK)
    } else {
        UNSPECIFIED
    }
}

/// `EXACTLY px`.
pub(crate) fn exactly(px: i32) -> i32 {
    EXACTLY | (px & SIZE_MASK)
}

/// A memo that is open for one layout pass at a time.
///
/// Passes can nest (the keyboard avoider computes the layout from inside
/// work that may itself be a pass), so `begin` / `end` count: the memo
/// clears when the outermost pass ends.
pub(crate) struct PassMemo<K, V> {
    depth: u32,
    answers: HashMap<K, V>,
}

impl<K: Hash + Eq, V: Copy> PassMemo<K, V> {
    pub(crate) fn new() -> Self {
        Self { depth: 0, answers: HashMap::new() }
    }

    pub(crate) fn begin(&mut self) {
        self.depth += 1;
    }

    pub(crate) fn end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.answers.clear();
        }
    }

    /// The answer this pass already has for `key`. Always `None` outside a
    /// pass.
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        if self.depth == 0 {
            return None;
        }
        self.answers.get(key).copied()
    }

    /// Record `value` as this pass's answer for `key`. Dropped outside a
    /// pass.
    pub(crate) fn put(&mut self, key: K, value: V) {
        if self.depth > 0 {
            self.answers.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_layout::{AvailableSpace, LayoutTree, Size};
    use runtime_shared::{AlignItems, FlexDirection, Length, StyleRules};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const DENSITY: f32 = 2.0;
    const CHAR_PX: i32 = 16;
    const LINE_PX: i32 = 40;

    /// What `TextView.measure` would answer for `chars` characters under
    /// `width_spec`, in px: one line if it fits, else wrapped lines at the
    /// bound.
    fn text_view_measure(chars: i32, width_spec: i32) -> (i32, i32) {
        let natural = chars * CHAR_PX;
        if width_spec == UNSPECIFIED {
            return (natural, LINE_PX);
        }
        let bound = (width_spec & SIZE_MASK).max(CHAR_PX);
        let w = natural.min(bound);
        let lines = (natural + bound - 1) / bound;
        (w, lines.max(1) * LINE_PX)
    }

    /// The text leaf's measure_fn, shaped like `primitives::text`'s: the
    /// width it measures at, the spec through `memo`, and `probes` counting
    /// the questions that reached the view.
    fn text_measure(
        id: usize,
        chars: i32,
        memo: Rc<RefCell<PassMemo<(usize, i32, i32), (i32, i32)>>>,
        probes: Rc<Cell<usize>>,
    ) -> runtime_layout::MeasureFn {
        Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
            let avail_w = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                AvailableSpace::MaxContent => f32::INFINITY,
                // The widest word: these labels are one word.
                AvailableSpace::MinContent => (chars * CHAR_PX) as f32 / DENSITY,
            });
            let spec = at_most_or_unspecified(avail_w, DENSITY);
            let key = (id, spec, UNSPECIFIED);
            let cached = memo.borrow().get(&key);
            let (w, h) = cached.unwrap_or_else(|| {
                probes.set(probes.get() + 1);
                let answer = text_view_measure(chars, spec);
                memo.borrow_mut().put(key, answer);
                answer
            });
            Size {
                width: known.width.unwrap_or((w as f32 / DENSITY).ceil()),
                height: known.height.unwrap_or((h as f32 / DENSITY).ceil()),
            }
        })
    }

    fn rules(f: impl FnOnce(&mut StyleRules)) -> StyleRules {
        let mut r = StyleRules::default();
        f(&mut r);
        r
    }

    /// A roster block: 24 rows of avatar initials, a growing name and a
    /// status, each row a bordered flex row. Returns the tree, the root and
    /// the number of text leaves.
    fn roster(
        memo: &Rc<RefCell<PassMemo<(usize, i32, i32), (i32, i32)>>>,
        probes: &Rc<Cell<usize>>,
    ) -> (LayoutTree, runtime_layout::LayoutNode, usize) {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        t.set_style(root, &rules(|r| r.gap = Some(8.0.into())));
        let mut texts = 0;
        let mut text = |t: &mut LayoutTree, parent, chars| {
            let n = t.new_node();
            t.add_child(parent, n);
            t.set_measure_fn(n, text_measure(texts, chars, memo.clone(), probes.clone()));
            texts += 1;
            n
        };
        for i in 0..24 {
            let row = t.new_node();
            t.add_child(root, row);
            t.set_style(
                row,
                &rules(|r| {
                    r.flex_direction = Some(FlexDirection::Row);
                    r.align_items = Some(AlignItems::Center);
                    r.gap = Some(8.0.into());
                    r.padding_left = Some(Length::Px(8.0).into());
                    r.padding_right = Some(Length::Px(8.0).into());
                }),
            );
            let avatar = t.new_node();
            t.add_child(row, avatar);
            t.set_style(
                avatar,
                &rules(|r| {
                    r.width = Some(Length::Px(40.0).into());
                    r.height = Some(Length::Px(40.0).into());
                    r.align_items = Some(AlignItems::Center);
                }),
            );
            text(&mut t, avatar, 2);
            let name = text(&mut t, row, 18 + i % 5);
            t.set_style(name, &rules(|r| r.flex_grow = Some(1.0.into())));
            text(&mut t, row, 8);
        }
        (t, root, texts)
    }

    #[test]
    fn regression_android_mount_measures_each_text_once_per_spec() {
        // The bug: mounting a 24-row roster ran `TextView.measure` about 8
        // times per text leaf (625 probes for 77 labels, ~90% of an 80 ms
        // layout pass; this fixture makes 576 for 72 without the memo).
        // Within one pass a repeated spec must not reach the view again,
        // which leaves 2-3 distinct specs per leaf.
        let memo = Rc::new(RefCell::new(PassMemo::new()));
        let probes = Rc::new(Cell::new(0));
        let (mut t, root, texts) = roster(&memo, &probes);
        memo.borrow_mut().begin();
        t.compute(root, 400.0, 2000.0);
        memo.borrow_mut().end();
        assert!(
            probes.get() <= texts * 3,
            "{} probes for {texts} text leaves: a pass re-measured a spec it already had",
            probes.get(),
        );
        // And the memo changed no answer: the same tree computed with
        // every probe reaching the view lays out identically.
        let unmemoized = Rc::new(RefCell::new(PassMemo::new()));
        let raw_probes = Rc::new(Cell::new(0));
        let (mut raw, raw_root, _) = roster(&unmemoized, &raw_probes);
        raw.compute(raw_root, 400.0, 2000.0);
        assert!(raw_probes.get() > probes.get(), "the fixture must exercise repeated specs");
        let frames = |t: &LayoutTree, r| {
            let mut out = Vec::new();
            let mut stack = vec![r];
            while let Some(n) = stack.pop() {
                out.push(t.frame_of(n));
                stack.extend(t.children_of(n));
            }
            out
        };
        assert_eq!(frames(&t, root), frames(&raw, raw_root));
    }

    #[test]
    fn a_closed_memo_answers_nothing_so_a_changed_text_is_measured_again() {
        let mut memo: PassMemo<u8, i32> = PassMemo::new();
        memo.begin();
        memo.put(1, 10);
        assert_eq!(memo.get(&1), Some(10));
        memo.end();
        // Between passes the label may have changed: no stale answer.
        assert_eq!(memo.get(&1), None);
        memo.put(1, 20);
        memo.begin();
        assert_eq!(memo.get(&1), None, "a put outside a pass must not leak into the next");
    }

    #[test]
    fn a_nested_pass_keeps_the_outer_pass_answers_until_the_outer_one_ends() {
        let mut memo: PassMemo<u8, i32> = PassMemo::new();
        memo.begin();
        memo.put(1, 10);
        memo.begin();
        memo.end();
        assert_eq!(memo.get(&1), Some(10));
        memo.end();
        assert_eq!(memo.get(&1), None);
    }

    #[test]
    fn specs_match_android_measure_spec_bits() {
        assert_eq!(at_most_or_unspecified(f32::INFINITY, 2.0), UNSPECIFIED);
        assert_eq!(at_most_or_unspecified(100.0, 2.0), AT_MOST | 200);
        assert_eq!(exactly(300), 0x4000_0000 | 300);
    }
}
