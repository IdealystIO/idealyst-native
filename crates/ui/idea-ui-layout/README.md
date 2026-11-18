# idea-ui-layout

Responsive screen layouts for [`idea-ui`](../idea-ui). idea-ui decides
what a control looks like; this kit decides how a screen is put together
at each width. A screen picks a layout and hands it content, and the
layout draws the phone, tablet and desktop form. A screen built from it
never reads the breakpoint.

How to build a screen with it is the `layouts` guide
([`crates/mcp/catalog/guides/layouts.md`](../../mcp/catalog/guides/layouts.md), also served by
the MCP as `describe(kind: "guide", name: "layouts")`). The rules it is built to are in [`RULES.md`](./RULES.md), and
[`examples/layout-demo`](./examples/layout-demo) has one screen per layout.

| | Components |
|---|---|
| The app | `AppFrame` (`FrameSurface::Flat`, `Raised`), `SidebarToggle` |
| The toolbar | `Toolbar` (`start`, `end`, `below`), `ToolbarTabs`, `ToolbarSearch`, `ToolbarButton` |
| The frame | `Page` (`PageVariant::Content`, `Table`, `Blank`), `PageHeader`, `PageFrame`, `PinnedPane` |
| Layouts | `SplitView` (`SplitMode::Nav`, `Detail`, `Inspector`), `Flow` |
| Blocks | `Section`, `SectionHeading`, `FieldGrid`, `FieldGridRow`, `LabelValue`, `StatGrid`, `ActionBar`, `StateMessage`, `ListStates` |
| Overlays | `ActionMenu`, `ActionMenuSurface`, `AdaptivePopover`, `DrillSheet`, `Dialog`, `DialogFooter`, `Sheet` and its rows |

```rust
use idea_ui::{Stack, StackGap};
use idea_ui_layout::{Action, FieldGrid, LabelValue, Page, PageHeader, Section};

ui! {
    Page(
        header = ui! {
            PageHeader(
                title = rx!(worker.get().name),
                actions = vec![Action::new("Edit", on_edit).icon(PENCIL).primary()],
                menu = vec![Action::new("Archive", on_archive).icon(ARCHIVE)],
            )
        },
        body = ui! {
            Stack(gap = StackGap::Lg) {
                Section(title = "Employment".to_string()) {
                    Stack(gap = StackGap::Sm) {
                        LabelValue(label = "Hired".to_string(), value = rx!(worker.get().hired_on))
                        LabelValue(label = "Position".to_string(), value = rx!(worker.get().position))
                    }
                }
                Section(title = "Contact".to_string()) {
                    FieldGrid(columns = 2_usize) { /* fields */ }
                }
            }
        },
    ) {
        EditWorkerDialog(open = editing)
    }
}
```

On a phone that header draws the title, a pencil button and one ⋮ that
opens an action sheet; the two `LabelValue` rows stack their labels over
their values; the field grid is one column; and `EditWorkerDialog`,
built on `Dialog`, rises as a bottom sheet. None of it is written in the
screen.
