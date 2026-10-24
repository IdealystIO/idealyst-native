+++
title = "Design review checklist"
summary = "The checklist a finished idea-ui screen or app must pass — brand/tokens, adaptive behavior, hierarchy, states, forms, overlays, platforms, accessibility — with the fix for each failure."
when = "Before calling a screen done, or when asked to review idea-ui UI."
order = 90
tags = ["review", "checklist", "qa", "accessibility"]
+++
# Design review checklist

Report each failure as `file:line — rule — what to change`, most severe
first. Severity: **blocker** (broken at a width/platform, unreachable,
unreadable), **major** (off-brand, missing state, wrong hierarchy),
**minor** (spacing, copy).

## Brand and tokens
- [ ] No color literal, font name or font size outside `brand.rs`. → move to the theme or a restyle.
- [ ] No pixel spacing in screens; gaps come from the spacing scale. → `StackGap`/`t.spacing`.
- [ ] No per-instance `styles = …` used for branding; no look-only wrapper components. → restyle in `brand.rs`.
- [ ] Works in dark mode (both schemes installed; nothing hard-coded light).

## Adaptive
- [ ] Checked at 390 and 1280 wide (and 820 if a layout changes at `Md`). → read_design_skill("adaptive").
- [ ] Patterns change kind where they should (push/pop ⇄ split view, drawer/tabs ⇄ sidebar, sheet ⇄ dialog, cards ⇄ table).
- [ ] All breakpoint logic uses `current_breakpoint()` / `breakpoint` blocks — no pixel checks, no `viewport_size()` layout.
- [ ] No horizontal page scroll; wide content scrolls in its own container.
- [ ] Crossing a breakpoint keeps state (open detail, input, scroll).

## Structure and hierarchy
- [ ] The screen's job and primary action are obvious; one filled button per view.
- [ ] Exactly one H1; sections H3; secondary text muted.
- [ ] Inside `Page` with the right width; one scroll container.
- [ ] Spacing encodes grouping (tighter inside, looser between).
- [ ] idea-ui components used where they exist (no hand-rolled buttons, inputs, lists, modals).

## Navigation
- [ ] The active section is visible; derived from the navigator.
- [ ] Every detail route handles an unknown id with a not-found page.
- [ ] Back is always available below `Lg` and on desktop; one visible title per screen.
- [ ] Bars have safe areas (`TOP` header, `BOTTOM` tab bar).

## States
- [ ] Loading (skeleton), error (inline + retry), first-run empty, no-results, data — all designed.
- [ ] Actions show pending (`loading`), success (navigate/toast) and failure (inline, input kept).
- [ ] One `ToastHost`, at the root.

## Forms
- [ ] Every field labeled; placeholders are examples.
- [ ] Validation on blur/submit with actionable messages.
- [ ] One save model per section, stated.
- [ ] Submit names the outcome; destructive actions fenced and confirmed (or undoable).

## Platforms and accessibility
- [ ] Nothing reachable only by hover; tooltips hold no essential info.
- [ ] Touch targets ≥ 44pt on touch.
- [ ] Inputs near the bottom are keyboard-avoided on phones.
- [ ] Color is never the only signal (status has text, errors have words).
- [ ] Text contrast ≥ 4.5:1 in both schemes.
- [ ] Text color set on text, not on containers.
- [ ] Copy: verbs on buttons, sentence case, errors say what to do.
