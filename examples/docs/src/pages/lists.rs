//! Lists page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "lists",
    title = "Lists and virtualization",
    category = Reference,
    description = "Virtualized lists that mount only what's on screen and recycle the rest.",
    related = ["primitives", "reactivity", "backends", "hot-reload"],
    concepts = [Virtualizer, FlatList, ItemKey, ItemSize],

    section(heading = "Overview") {
        p("For lists with more than a few dozen items, you don't want to build every \
           row up front and hand the backend a huge tree. Idealyst's ",
          code("flat_list<T>"), " is a virtualized list — it only mounts the rows \
           currently in (or near) the viewport, recycles them as you scroll, and \
           discards the rest. Memory stays bounded by the window size; scroll \
           performance stays smooth regardless of how big the data is."),
        p("The wrapper underneath every Idealyst virtualized list is the ",
          code("Virtualizer"), " primitive. ", code("flat_list"),
          " is the typed convenience layer on top — the API you'll actually use \
           from app code."),
    },

    section(heading = "The shape") {
        code(rust, r##"
            use runtime_core::{flat_list, fixed_size, signal, ui, Signal};

            #[derive(Clone)]
            pub struct Message {
                pub id: u64,
                pub author: String,
                pub body: String,
            }

            #[component]
            pub fn inbox() -> Element {
                let messages: Signal<Vec<Message>> = signal!(load_messages());

                ui! {
                    flat_list(
                        data       = messages,
                        key        = |_idx, msg| msg.id,
                        item_size  = fixed_size(72.0),
                        render_item = |_idx, msg| message_row(msg),
                    )
                }
            }

            fn message_row(msg: &Message) -> Element {
                ui! {
                    View {
                        Text { msg.author.clone() }
                        Text { msg.body.clone() }
                    }
                }
            }
        "##),

        p("Four required pieces:"),
        list(
            [code("data"), " — a ", code("Signal<Vec<T>>"), " holding the source of truth."],
            [code("key"), " — a function that returns a stable ", code("u64"), " identity per item."],
            [code("item_size"), " — either ", code("fixed_size(N)"), " for uniform-height rows, or a per-item closure for variable heights (see below)."],
            [code("render_item"), " — builds the subtree for one item. Called only when the item enters the mount window."],
        ),

        p("That's the whole API surface for the common case. The advanced knobs (",
          code("overscan"), ", ", code("horizontal"),
          ") are builder methods on the returned handle."),
    },

    section(heading = "How it stays bounded") {
        p("The framework never realizes more than the rows the backend needs. \
           When you scroll, the backend computes which rows are now inside the \
           viewport (plus an overscan buffer for smooth-feeling scrolling), tells \
           the framework which to mount and which to unmount, and the framework \
           runs your ", code("render_item"), " for the new ones."),
        p("Each rendered item lives in its own per-item reactive scope. When the \
           item scrolls out of the window:"),
        list(
            ["The scope drops."],
            ["Every signal allocated in that item's ", code("render_item"), " is freed."],
            ["Every effect, every ref, every backend node tied to that item is torn down."],
        ),
        p("When the item scrolls back in, a fresh scope is built and ",
          code("render_item"), " runs again. State that lived inside the item's \
           scope doesn't persist across scroll-out events — if you need state \
           that survives, lift it into the parent's scope or into the item's ",
          code("Message"), " struct."),
    },

    section(heading = "Keys — what they're for") {
        p("The ", code("key"), " closure returns a stable ", code("u64"),
          " identity per item. Whenever ", code("data"),
          " changes (insertions, removals, reorders), the framework matches old \
           keys against new keys to figure out which items moved, which stayed, \
           and which need to be torn down or built up."),
        p("Items whose key is still present after a data change keep their \
           mounted subtree intact. They may move in the layout, but their \
           internal state survives."),

        code(rust, r##"
            key = |_idx, msg| msg.id           // stable: matches database id
            key = |idx, _msg| idx as u64        // unstable: every reorder shuffles state
            key = |_idx, msg| hash(&msg.body)   // works, but recomputed every check
        "##),

        p("Two items returning the same key is a bug. The framework treats them \
           as the same logical row and will silently drop one of them. Use a \
           hashable unique field (a database id, a GUID)."),
    },

    section(heading = "Sizing strategies") {
        p("The backend needs to know how big each item is to lay out the scroll \
           content correctly. Two strategies:"),
    },

    section(heading = "Known") {
        p("The author tells the framework the exact height. The backend never \
           measures."),

        code(rust, r##"
            // Every row is 72px tall.
            item_size = fixed_size(72.0)

            // Or per-item, from data:
            item_size = FlatListItemSize::Known(Rc::new(|_idx, msg: &Message| {
                if msg.has_image { 200.0 } else { 72.0 }
            }))
        "##),

        p("Use this whenever the size is a function of data you have. It's the \
           cheapest path — layout is deterministic, the backend doesn't install \
           any measurement hooks."),
    },

    section(heading = "Measured") {
        p("The author provides an estimate; the backend measures the actual \
           rendered size after mount and stores it. Subsequent layout uses the \
           measured value."),

        code(rust, r##"
            item_size = FlatListItemSize::Measured(Rc::new(|_idx, _msg| 100.0))
        "##),

        p("Use this when the rendered size depends on layout/content the \
           framework can't see ahead of time — wrapped text whose wrap width \
           depends on the container, items with images of unknown aspect ratio, \
           anything where \"tall enough\" needs the backend's layout pass to \
           determine."),

        p("If the item's rendered size changes later (the item's content updates), \
           the backend's native layout observer (", code("ResizeObserver"),
          " on web, ", code("layoutSubviews"), " on iOS, ",
          code("OnLayoutChangeListener"),
          " on Android) re-fires and refreshes the stored size."),

        p("Cost: each measured item carries a layout observer. Use ",
          code("Known"), " when you can."),
    },

    section(heading = "Overscan and direction") {
        p("The two builder methods worth knowing:"),
        list(
            [code(".overscan(factor)"), " — multiplier on the viewport height for the mount window. ",
             code("1.0"), " (default) means mount one viewport-height of rows above and below the visible area. Higher values trade memory for smoother scroll feel on fast flicks; lower values save memory."],
            [code(".horizontal(true)"), " — flip the scroll axis. Items are laid out left-to-right and the viewport scrolls horizontally. Default is vertical."],
        ),

        code(rust, r##"
            ui! {
                flat_list(
                    data = items,
                    key = |_, item| item.id,
                    item_size = fixed_size(120.0),
                    render_item = |_, item| card_view(item),
                )
                .overscan(2.0)
                .horizontal(true)
            }
        "##),
    },

    section(heading = "What each backend does") {
        p("Same Rust contract, different native widgets:"),
        list(
            ["Web — A JS-side scroll handler (in ", code("backend-web/runtime/ts/virtualizer.ts"),
             ") owns the ", code("IntersectionObserver"),
             " and the visible-range diff. It calls back into Rust only when items enter or leave the window. Per-item scopes drop on exit, freeing signals/effects."],
            ["iOS — ", code("UICollectionView"), " with a flow layout that consults ",
             code("item_size"), ". Real cell recycling: ", code("prepareForReuse"),
             " releases an item's subtree; ", code("cellForItemAt"),
             " builds the next one when scrolling brings it back."],
            ["Android — ", code("RecyclerView"), " plus a ", code("ListAdapter"), " with ",
             code("DiffUtil"), ". ", code("onBindViewHolder"), " runs ",
             code("render_item"), "; ", code("onViewRecycled"), " releases the scope."],
            ["Roku — Different model. As a generator backend, Roku can't ship closures, so the row template is built once at snapshot time. The device-side BrightScript runtime materializes per-row instances and remaps signal references per row. Trade-off: more constrained, no measured sizing."],
        ),
        p("You write one ", code("flat_list(...)"),
          " call. Each backend dispatches it through its native virtualization \
           machinery without you knowing."),
    },

    section(heading = "Reactivity") {
        p(code("data"), " is a signal, so the list is reactive end-to-end:"),

        code(rust, r##"
            let messages = signal!(load_messages());

            // Add an item:
            messages.update(|v| v.push(new_message));

            // Remove by id:
            messages.update(|v| v.retain(|m| m.id != target_id));

            // Sort:
            messages.update(|v| v.sort_by_key(|m| m.timestamp));
        "##),

        p("The framework reads the current snapshot whenever the virtualizer \
           queries item count, keys, or sizes. Each backend's diff algorithm \
           figures out the minimal set of mount, unmount, and reorder operations \
           to apply."),

        p("A few specifics:"),
        list(
            ["Insertions mount the new items if they fall inside the current window; otherwise they're just bookkeeping."],
            ["Removals unmount the items if they were inside the window. Their scopes drop."],
            ["Reorders preserve mounted subtrees — items whose key stayed move to their new position, but their internal state survives."],
            ["Bulk replacements (assigning a whole new ", code("Vec"), ") — same diff algorithm. Keys that match preserve state; new keys build fresh; old keys tear down."],
        ),
    },

    section(heading = "Simple list of strings") {
        code(rust, r##"
            let names = signal!(vec!["Ada".to_string(), "Linus".to_string()]);

            ui! {
                flat_list(
                    data = names,
                    key = |idx, s: &String| {
                        // Hashing a String works but recomputes each check.
                        // For stable data, pass the source's id instead.
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        s.hash(&mut h);
                        h.finish()
                    },
                    item_size = fixed_size(44.0),
                    render_item = |_, s| ui! { Text { s.clone() } },
                )
            }
        "##),
    },

    section(heading = "List with reactive item content") {
        p("If your items have reactive state, lift it into a struct that includes \
           the signal, and have ", code("render_item"), " set up the bindings:"),

        code(rust, r##"
            #[derive(Clone)]
            pub struct TodoItem {
                pub id: u64,
                pub title: String,
                pub done: Signal<bool>,    // Signal is Copy — fine to hold in T
            }

            ui! {
                flat_list(
                    data = todos,
                    key = |_, t| t.id,
                    item_size = fixed_size(56.0),
                    render_item = |_, t| {
                        let done = t.done;
                        ui! {
                            Pressable(on_click = move || done.update(|v| *v = !*v)) {
                                Text { t.title.clone() }
                                Text { if done.get() { "✔" } else { "" } }
                            }
                        }
                    },
                )
            }
        "##),

        p("The signal lives in the parent's scope (where the ",
          code("Vec<TodoItem>"),
          " was built), so its lifetime is tied to the parent — not to the \
           item's mount/unmount cycle. Items can scroll in and out without \
           losing checked state."),
    },

    section(heading = "Horizontal list of cards") {
        code(rust, r##"
            ui! {
                flat_list(
                    data = featured,
                    key = |_, c| c.id,
                    item_size = fixed_size(280.0),    // width here, since horizontal
                    render_item = |_, c| card_view(c),
                )
                .horizontal(true)
            }
        "##),
    },

    section(heading = "Pitfalls") {
        list(
            ["Duplicate keys. Two items returning the same key get conflated. The visible symptom is rows appearing to \"lose\" state on a reorder. Pick a key from a unique field."],
            ["Index-as-key. ", code("key = |idx, _| idx as u64"),
             " is tempting but defeats the point — any insertion or reorder shifts every index, so the framework tears down every mounted item and rebuilds. Use a stable id from the data instead."],
            ["Stale closure captures in ", code("render_item"), ". ",
             code("render_item"),
             " runs per mount — every time an item enters the window. If you capture a ", code("Vec"),
             " snapshot outside the closure and refer to it inside, you'll see the snapshot's value, not the current signal value. Read signals inside the closure."],
            ["Mixing ", code("Known"), " and ", code("Measured"),
             ". Not directly supported — pick one strategy per list. If most items are predictable but some need measuring, use ",
             code("Measured"), " for the whole list (cost is per-item, not per-list-mode)."],
            ["Items with their own scrollable content. ", code("flat_list"),
             " expects to control the scroll axis. Putting a ", code("ScrollView"),
             " inside an item works, but cross-axis scrolling (horizontal list of items each with a vertical ",
             code("ScrollView"),
             " inside) is the only sane combination — same-axis nesting is a UX anti-pattern most platforms will fight you on."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            [link("Primitives", to = "primitives"), " — the ",
             code("Virtualizer"), " primitive entry and the rest of the primitive list."],
            [link("Reactivity", to = "reactivity"), " — per-item scopes and the cleanup model."],
            [link("Backends", to = "backends"), " — what each backend does to implement virtualization."],
            [link("Hot reload", to = "hot-reload"), " — what happens to mounted items when the source code changes (spoiler: identity-keyed nodes survive)."],
        ),
    },
}
