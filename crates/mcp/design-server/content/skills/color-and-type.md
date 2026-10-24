+++
title = "Color, emphasis and type"
summary = "What each tone means, which variant to use for which level of emphasis, when palette hues are allowed, and how to use the type scale — so the brand's colors land in the right places."
when = "Choosing tone/variant for buttons, badges, alerts; coloring status or categories; picking text styles."
order = 38
tags = ["color", "tone", "variant", "emphasis", "typography", "badge", "status"]
+++
# Color, emphasis and type

The brand sets the colors; these rules decide where they go.

## Tones are meanings

| Tone | Means | Use for |
| --- | --- | --- |
| `tone::Primary` | "this is the brand's main path" | the primary action, selected/active states, links |
| `tone::Neutral` | no particular meaning | secondary actions, Cancel, toolbar buttons, neutral badges |
| `tone::Success` | it worked / it's healthy | saved, active, paid, passing |
| `tone::Warning` | needs attention soon | at risk, expiring, partial |
| `tone::Danger` | destructive or failed | delete, errors, overdue, blocked |
| `tone::Info` | neutral information worth noticing | tips, "new" notices |
| `tone::Secondary` | the brand's second accent | sparingly — only if the brand defines one |

- **Color is never the only signal**: a status badge has a label, an error
  has text, a required field says so.
- **Status maps to tone through one function** (`fn status_tone(s) -> ToneRef`),
  so "At risk" is the same yellow everywhere.
- **Palette hues** (`tone::Teal`, `tone::Violet`, …,
  `tone::palette_from_seed(id)`) carry no meaning — use them only to tell
  categories apart (tags, avatars, calendar colors, chart series). Never
  for status, never for actions.

## Variants are emphasis

| Variant | Emphasis | Button use |
| --- | --- | --- |
| `variant::Filled` (default) | highest | the ONE primary action on screen / in a dialog |
| `variant::Soft` | medium | a secondary action that still matters; badges, alerts |
| `variant::Outlined` | medium-low | secondary actions beside a primary; a danger action at rest |
| `variant::Ghost` | lowest | Cancel, toolbar and icon buttons, "show more" |

- **One filled button per view** (a dialog counts as a view). Two filled
  buttons means two primary actions — pick one.
- **Destructive at rest is outlined danger; the confirm is filled danger.**
- **Disabled is not a design.** Prefer enabled buttons that explain what's
  missing on press (validation) over a dead button with no reason.

## Type

idea-ui's scale (`typography_kind::…`) — the brand may change sizes and
the face; the roles stay:

| Kind | Role |
| --- | --- |
| `Display` | marketing heroes, auth splash. Not in app screens. |
| `H1` | the page title — once per screen |
| `H2` | a big number in a stat tile; a page with a second grouping level |
| `H3` | section titles |
| `H4` | card titles, dialog titles on phones |
| `BodyLg` | lead paragraph, prominent row label |
| `Body` | everything else |
| `BodySm` | metadata, secondary lines, help text |
| `Caption` / `Overline` | labels above values, timestamps |

- **Muted, not smaller, for secondary.** `muted = true` first; drop a size
  only if it's also less important.
- **Two weights per screen** is plenty: regular and the heading weight.
  Use `weight` for emphasis inside a line, not for whole paragraphs.
- **Line length**: body text ≤ ~75 characters — `Page(width = Narrow)`
  gets this for free.
- **Never set a font size** in an app sheet; pick a kind.
