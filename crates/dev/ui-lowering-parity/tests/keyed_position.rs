//! Keyed `for` rows land between their neighbours when only a key changes
//! — the downstream report, end to end through `ui!`.
//!
//! The report: rows keyed on a digest of the whole item (so toggling a
//! field changes that row's key), and a section-head list followed by an
//! items list in one parent. After a key change the remounted row
//! rendered one position too high (above its previous neighbour), or the
//! head landed after the items list; a full rebuild drew the right order.
//!
//! The cause was in `runtime-scene`: a spliced region resolved its start
//! index once, at mount. Any EARLIER region in the same parent that later
//! changed its node count (a sibling list loading or emptying) left every later region splicing at a stale index. The unit-level
//! regressions live in `runtime-scene`'s tests (`regression_keyed_*`);
//! these pin that `ui!`'s lowering of sibling `for … key =` blocks puts
//! them in ONE parent as sibling spliced regions, which is the shape that
//! hit it.
//!
//! The harness splices (`supports_splice = true`) like every shipping
//! backend; the anchored fallback rebuilds whole lists and was never
//! affected.

use runtime_macros::ui;
use runtime_vocabulary::glue::{signal, Element, Signal};

#[derive(Clone, PartialEq)]
struct Field {
    id: u32,
    required: bool,
}

impl Field {
    fn digest(&self) -> String {
        format!("{}:{}", self.id, self.required)
    }
    fn label(&self) -> String {
        format!("f{}{}", self.id, if self.required { "*" } else { "" })
    }
}

fn fields(ids: &[u32]) -> Vec<Field> {
    ids.iter().map(|&id| Field { id, required: false }).collect()
}

fn toggle(list: Signal<Vec<Field>>, id: u32) {
    let mut v = list.peek();
    for f in &mut v {
        if f.id == id {
            f.required = !f.required;
        }
    }
    list.set(v);
}

struct State {
    notes: Signal<Vec<Field>>,
    heads: Signal<Vec<Field>>,
    items: Signal<Vec<Field>>,
}

/// Three keyed lists flattened into ONE parent: a notes list, then the
/// reporter's section-head list, then the items list. (A reactive `if`
/// is no stand-in for the earlier region: `ui!` gives it exactly one
/// node in either state — a wrapper or an empty view — so it never
/// shifts what follows. A keyed list changing length does.)
fn form(s: &State) -> Element {
    let notes = s.notes;
    let heads = s.heads;
    let items = s.items;
    ui! {
        view {
            for note in notes, key = note.digest() {
                text { format!("note{}", note.id) }
            }
            for head in heads, key = head.digest() {
                text { head.label() }
            }
            for item in items, key = item.digest() {
                text { item.label() }
            }
        }
    }
}

/// The text rows under the root view, in on-screen order. The view is
/// the ONLY live root: a removed keyed row is released, not left behind
/// as a parentless node.
fn rows(h: &host_mock::Harness) -> Vec<String> {
    let roots = h.live_roots();
    assert_eq!(roots.len(), 1, "one live root, the view: {roots:?}");
    h.children_of(roots[0])
        .into_iter()
        .map(|n| {
            let kind = h.kind_of(n).unwrap_or_default();
            kind.strip_prefix("text ")
                .map(|q| q.trim_matches('"').to_string())
                .unwrap_or(kind)
        })
        .collect()
}

fn mount(
    notes: &[u32],
    heads: &[u32],
    items: &[u32],
) -> (host_mock::Harness, State, runtime_scene::Realized<host_mock::Node>) {
    let h = host_mock::Harness::new();
    h.shared.splice.set(true);
    let (state, tree) = h.world.enter(|| {
        let state = State {
            notes: signal(fields(notes)),
            heads: signal(fields(heads)),
            items: signal(fields(items)),
        };
        let tree = form(&state);
        (state, tree)
    });
    let r = h.mount(tree);
    h.flush();
    (h, state, r)
}

fn act(h: &host_mock::Harness, f: impl FnOnce()) {
    h.world.enter(f);
    h.flush();
}

#[test]
fn key_change_with_unchanged_regions_before_keeps_position() {
    let (h, s, _r) = mount(&[9], &[0], &[1, 2, 3]);
    assert_eq!(rows(&h), ["note9", "f0", "f1", "f2", "f3"]);
    act(&h, || toggle(s.items, 2));
    assert_eq!(rows(&h), ["note9", "f0", "f1", "f2*", "f3"]);
    act(&h, || toggle(s.heads, 0));
    assert_eq!(rows(&h), ["note9", "f0*", "f1", "f2*", "f3"]);
    act(&h, || toggle(s.items, 1));
    act(&h, || toggle(s.items, 3));
    assert_eq!(rows(&h), ["note9", "f0*", "f1*", "f2*", "f3*"]);
}

#[test]
fn regression_item_key_change_after_head_list_loaded() {
    // Shape 1: heads arrive after mount (async data), so the items list
    // mounted with an empty head list before it. The changed row "jumped
    // up one position until a refresh".
    let (h, s, _r) = mount(&[], &[], &[1, 2, 3]);
    act(&h, || s.heads.set(fields(&[0])));
    assert_eq!(rows(&h), ["f0", "f1", "f2", "f3"]);
    act(&h, || toggle(s.items, 2));
    assert_eq!(rows(&h), ["f0", "f1", "f2*", "f3"]);
}

#[test]
fn regression_head_key_change_after_earlier_list_shrank() {
    // Shape 2: the list before the heads shrank, so the head's remounted
    // node "landed after the items list".
    let (h, s, _r) = mount(&[7, 8], &[0], &[1]);
    act(&h, || s.notes.set(Vec::new()));
    assert_eq!(rows(&h), ["f0", "f1"]);
    act(&h, || toggle(s.heads, 0));
    assert_eq!(rows(&h), ["f0*", "f1"]);
}
