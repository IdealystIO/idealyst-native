+++
title = "Forms and input"
summary = "Form layout, labels and help, validation timing, save models (explicit vs immediate), choosing the right control (Select vs RadioGroup vs SegmentedControl, Switch vs Checkbox), submit/loading/error feedback."
when = "Building any form, settings section, filter panel or input-heavy screen."
order = 40
tags = ["forms", "validation", "field", "select", "switch", "checkbox", "settings", "input"]
+++
# Forms and input

## Layout

- **One column.** Two columns only from `Md` up, and only for short paired
  fields (first/last name, city/postcode).
- **Group** related fields into titled `Section`s of ≤ 6 fields, each in a
  `Card` with `Stack(gap = StackGap::Md)`.
- **Width**: `Page(width = PageWidth::Narrow)`. Fields fill the column;
  short values (postcode, quantity) may set `width` on the `Field`.
- **Order** fields the way the user thinks of them, not the database.

## Labels, help, placeholders

- **Every field has a visible label** (`Field(label = Some(…))`). A
  placeholder is an example ("name@company.com"), never the label. The one
  exception is a search field at the top of the list it filters: no
  visible label, but `accessibility_label = Some("Search customers".into())`.
- **Help** (`help = Some(…)`) says what the value is for or its rules
  ("Shown to your teammates", "At least 12 characters"), shown before the
  user errs.
- **Optional, not required, is marked** — most fields should be required;
  mark the few that aren't with `label_hint = Some("Optional".into())`.

## Validation

- **On blur, and on submit — never per keystroke.** Errors appear after
  the user leaves a field or tries to submit, and clear the moment the
  value becomes valid. Submitting marks every field touched so all errors
  show at once.
- **Error text** (`error = rx!(…)`) says what's wrong and how to fix it:
  "Enter an email address like name@company.com", not "Invalid".
- **Server errors** go to the field they're about when possible; otherwise
  an inline `Alert(tone = Danger)` at the top of the form section.
- The `settings_form` recipe's `ValidatedField` is the pattern.

## Save models

Pick one per section and say which:

| Model | Use for | Feedback |
| --- | --- | --- |
| **Explicit save** (draft + Save button) | text fields, anything with validation, multi-field changes | `Button(loading = saving)`; success toast; error inline |
| **Immediate** | toggles, single choices, preferences | the control itself; section description "Changes apply immediately" |

Never mix models inside one section. A modal form is explicit save:
primary "Save", ghost "Cancel"; dismissing with unsaved changes asks first.

## Choosing a control

| Choice | Control |
| --- | --- |
| on/off that applies now | `Switch` |
| on/off that's part of a form submission; agree to terms; multi-select from a short list | `Checkbox` |
| 2–4 mutually exclusive options, short labels, view modes | `SegmentedControl` |
| 2–6 exclusive options that need descriptions | `RadioGroup` |
| 5+ options, or options from data | `Select` |
| many options where typing is faster | `Autocomplete` |
| free text, one line / multi-line | `Field` / `Textarea` |
| a number in a range where exactness doesn't matter | `Slider` (with the value shown) |
| dates and times | `DateInput` / `DatePicker` / `TimeInput` |

- Use the right `keyboard` on `Field` (`KeyboardKind::Email`, numeric, …)
  and `secure = true` for passwords — phones show the right keyboard.

## Submit

- **The submit button names the outcome**: "Create project", "Save
  profile". Primary, filled, at the end of the form (trailing), Cancel
  ghost beside it.
- **While submitting**: `loading = true` on the button; the form stays
  editable unless the submission makes edits meaningless.
- **On success**: navigate to the result, or toast for in-place saves.
  **On failure**: keep every value the user typed.
- **Enter submits** single-field forms (search, sign-in) via
  `on_key_down`; multi-field forms don't submit on Enter from a Textarea.

## Phones

- Inputs near the bottom need `keyboard_avoiding_view` at the root
  (read_design_skill("platforms")).
- Long forms on phones keep the submit reachable: at the end of the form
  *and* in the page header actions.
