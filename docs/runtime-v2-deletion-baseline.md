# Wave 2c deletion baseline

Wave 2c deletes the old core: the `runtime-core` crate (the `Element`
enum, the `Backend` mega-trait, the render walker, `Bound`/builders,
`external`), `runtime-vocabulary`'s `legacy-bridge` feature
(`bridge::LegacyBridge` + `caps::NavigatorOps::create_navigator`), and
every `old-core` cargo leg.

This document is the **pre-deletion record**: what the old core's output
was frozen to, what test coverage exists today, which methods currently
resolve to `Backend` trait defaults, and which tests legitimately die.
It exists so a later stage can prove nothing was lost.

Companion reading: [`migrating-to-runtime-v2.md`](migrating-to-runtime-v2.md)
(especially "What is guaranteed" — the closed sanctioned-divergence
list — and the backend boot table).

---

## 1. Frozen parity corpora

A large share of the migration's regression value lived in **old-vs-new
parity harnesses**: tests that render the same scene on both cores in one
process and assert equality. Those assertions cannot survive the
deletion — they need the old core to produce the reference half.

Every one of them has been converted: the old core's output is now
committed as a golden artifact, and the surviving assertion compares the
**new core** against that artifact. The mechanism is
[`crates/dev/parity-goldens`](../crates/dev/parity-goldens/src/lib.rs)
(`Goldens::freeze_*` writes only under `IDEALYST_FREEZE_GOLDENS=1`;
`Goldens::check_*` always compares).

Wherever it was cheap, the in-process old-vs-new assertion was **kept
alive alongside** the golden gate, so this wave reduces coverage nowhere.
Those call sites are marked with the greppable comment
`DIES-WITH-OLD-CORE` — that marker is the deletion wave's edit list:

```bash
grep -rn "DIES-WITH-OLD-CORE" crates/ websites/
```

| Suite | Artifact | Location | What it pins |
| --- | --- | --- | --- |
| `backend-cpu/tests/newcore_parity.rs` | lossless RGBA8 PNG framebuffers (8) | `crates/backend/cpu/tests/goldens/` | **pixel**-exact `MemSurface` output |
| `backend-terminal/tests/newcore_parity.rs` | cell-exact grid dumps (2) | `crates/backend/terminal/tests/goldens/` | glyph + fg + bg per cell |
| `backend-roku/tests/newcore_parity.rs` | serialized command streams (8) | `crates/backend/roku/tests/goldens/` | byte-exact wire commands (the stream IS the observable — no thin client in-tree) |
| `backend-ssr/tests/newcore_byte_identity.rs` | `html` + `head.css` per item (12 items) | `crates/backend/ssr/tests/goldens/` | byte-exact SSR output = the hydration acceptance gate |
| `backend-email/tests/newcore_golden.rs` | `html` + `txt` (subject + plaintext) per item (5) | `crates/backend/email/tests/goldens/` | byte-exact email output, incl. the REAL idea-ui-mail welcome template |
| `mock-backend/tests/wire_behavior_newcore.rs` | canonical catch-up snapshot JSON | `crates/dev/mock-backend/tests/goldens/` | wire-protocol identity across cores |
| `websites/website/tests/ssg_parity.rs` | 33 routes × (`html` + `head.css`) + served doc = 67 files, 1.7 MB | `websites/website/tests/goldens/ssg/` | full-site SSG byte parity — the broadest SSR/hydration coverage in the tree |
| `crates/dev/scene-parity` | structural + full-op op-log goldens (already committed before this wave) | `goldens/`, `goldens_full/`, `goldens_newcore/`, `goldens_full_newcore/` | exact backend op sequences for 13 structural × 2 modes + 27 full-op scenarios |

Each directory carries its own `README.md` with the corpus table, the
exact regeneration command, and the post-deletion warning.

### The post-deletion regeneration warning (stated once, repeated in every README)

While the old core exists, `IDEALYST_FREEZE_GOLDENS=1` **re-derives the
artifacts from the old core** — the freeze call sites are fed old-core
output.

**After wave 2c this is impossible.** The freeze call sites go away with
the old-core legs, and the only thing a regeneration can then do is
re-baseline the corpus against **the new core's current output**. That is
a deliberate re-baseline, not a regeneration: it permanently discards the
old core's testimony and there is no way to recover it. Treat it like
editing a golden — review the diff as the substance of the change, and
never do it to make a red test green.

### Normalizations (carried over verbatim; none widened)

Only three suites normalize anything, and no normalization was loosened
to make the new core pass:

1. **scene-parity** — the pre-existing closed sanctioned-divergence
   machinery (`new_core::normalize` for the virgin-anchor `clear_children`
   skip, plus the explicit `goldens_newcore/` /
   `goldens_full_newcore/` override files for divergence classes #3, #5,
   #6). Unchanged by this wave.
2. **backend-roku** — (a) the same virgin-anchor `ClearChildren` skip
   (`normalize_sanctioned_old`, applied to the OLD side only, exactly as
   before — Roku is the first stream-visible instance of that class);
   (b) `cache_key` **interning**, an artifact-serialization fix, not a
   divergence: `CreateIcon`/`UpdateIconData` carry a key derived from the
   icon `paths` static's ADDRESS, stable within a process but not across
   them under ASLR. Each distinct key becomes `#0`, `#1`, … in
   first-appearance order, preserving icon identity and aliasing. The
   in-process compare still uses the RAW value and therefore still pins
   cross-core identity of the key.
3. **website ssg_parity** — the pre-existing `assert_bytes`: strict byte
   equality FIRST, with the documented `display: contents`
   reactive-anchor collapse accepted only when it is the *entire*
   difference (the presence Dyn-hole anchor; each core's SSR matches its
   own hydration contract). Reused verbatim for the frozen comparison.
   In practice it fires on exactly one page (`primitives.html`).

`backend-cpu`, `backend-terminal`, `backend-ssr`, `backend-email` and
`mock-backend` normalize **nothing**.

### Divergences discovered while freezing

- **None in output.** Every corpus froze and re-verified green on the
  first pass: the new core reproduces the old core's frozen bytes/pixels/
  cells/streams exactly (modulo the pre-existing sanctioned set above).
- **One artifact-stability problem, not a core divergence**: Roku's
  pointer-derived `cache_key` (handled by interning, above). Found by the
  freeze run failing on a *second* process — precisely the class of bug
  freezing is supposed to surface.

---

## 2. Default-resolved `Backend` methods, per backend

The next stage converts each backend's caps impls off `Backend`. Today
every `newcore.rs` implements `runtime_scene::Host` + all 30
`runtime_vocabulary::caps::*Ops` traits by **UFCS-delegating** to
`<X as Backend>::method`. Any method a backend never overrode therefore
resolves to a **`Backend` trait default** at runtime. After the trait
dies it must resolve to a **caps-trait default** instead — and a
silently-differing default would change behavior invisibly.

Totals (verified by reading `crates/runtime/core/src/backend.rs` and
`crates/runtime/vocabulary/src/caps/*.rs`, cross-checked against
`crates/runtime/vocabulary/COVERAGE.md`):

| | count |
| --- | --- |
| `Backend` trait methods | 159 |
| …required (no default body) | 8 — `create_view`, `create_text`, `create_button`, `insert`, `update_text`, `clear_children`, `apply_style`, `finish` |
| …with default bodies | 151 |
| mapped to a caps `*Ops` trait | 152 |
| absorbed by `runtime_scene::Host` (no caps counterpart) | 7 |
| caps-trait methods | 154 (152 mapped + 2 caps-only) |

### 2.1 Caps defaults vs Backend defaults — VERIFIED EQUIVALENT

All comparable default bodies were extracted from both traits and diffed
after path normalization. **136 of 141 are byte-identical; the 10
textual differences are all cosmetic** — `runtime_core::` vs
`runtime_shared::` spellings of the same re-exported type, `crate::`/
`caps::`/`noop::` prefixes, trailing commas, `{ }` vs `{}`, line
wrapping. Every one was read in raw source and classified individually:

`create_element`, `create_styled_text`, `update_styled_text`,
`note_virtualizer_binding`, `set_text_input_focus_handler`,
`create_activity_indicator`, `make_pressable_handle`,
`make_virtualizer_handle`, `missing_primitive_placeholder`,
`create_navigator`.

The cross-trait-delegating defaults were checked explicitly and match:
`apply_styled_states` → `apply_style`; `apply_styled_variants` →
`apply_styled_states`; `update_styled_text` →
`update_text(plain_text_of(runs))`; `execute_batch_with_attach` →
`execute_batch` + `insert_many`; `apply_scroll_view_safe_area_inset` →
`apply_safe_area_padding`; `create_element` / `create_pressable` /
`create_link` / `create_presence_placeholder` → `create_view`;
`update_button_label` → `update_text`; `missing_primitive_placeholder` →
`create_external`.

One pre-documented, behaviourally-inert difference:
`missing_primitive_placeholder`'s local `struct MissingPrimitive` has a
distinct `TypeId` from runtime-core's private one (COVERAGE.md,
"Judgment calls" §5) — both are registry-unknown, so dispatch is
identical.

**Empirically confirmed** by the scene-parity conversion: `FullRecorder`
(which default-resolves 84 of 152 caps methods) was moved off
`LegacyBridge` onto direct `Host` + 30 caps impls in this wave, and all
102 scene-parity tests stayed green against **unchanged** goldens. That
is a live proof that the default swap is behavior-preserving across the
whole op alphabet those goldens cover.

> **So the caps defaults are not the risk. §2.2 and §2.3 are.**

### 2.2 🔴 THE REAL RISK: four `Host` methods have Backend defaults but are `Host`-REQUIRED

Of the 7 Backend methods absorbed by `runtime_scene::Host`, five have
Backend defaults — and four of those are **required** on `Host`
(`crates/runtime/scene/src/host.rs`). Today every `newcore.rs` papers
over this by UFCS-delegating to the Backend default. After deletion each
affected backend needs an **explicit body reproducing exactly**:

| Backend method | Host name | Backend default body that must be reproduced | Host status |
| --- | --- | --- | --- |
| `insert_at` | `insert_at` | `self.insert(parent, child)` (append — index ignored) | **required** |
| `remove_child` | `remove_child` | no-op | **required** |
| `supports_child_splice` | `supports_splice` | `false` | **required** |
| `create_reactive_anchor` | `create_anchor` | `self.create_view(&AccessibilityProps::default())` | **required** |
| `insert_many` | `insert_many` | N× `insert` loop | default — **byte-identical** on `Host`, safe |

Backends relying on ≥1 of these Backend defaults today:

| Relies on | Backends |
| --- | --- |
| all 5 | backend-terminal, backend-cpu, backend-linux, backend-windows, and the three non-target-OS `stub.rs` impls (macos/ios/android) |
| 4 (`insert_many`, `supports_child_splice`, `remove_child`, `insert_at`) | backend-roku, render-wgpu |
| 3 (`insert_many`, `supports_child_splice`, `remove_child`) | backend-ssr, backend-email |
| 3 (`supports_child_splice`, `remove_child`, `insert_at`) | dev-server `WireRecordingBackend` |
| 2 (`create_reactive_anchor`, `insert_many`) | backend-macos, backend-ios-mobile, backend-android-mobile |
| 0 | backend-web, mock-backend, scene-parity `FullRecorder` / `ParityBackend` |

One backend already does this correctly and is the model:
`crates/backend/ssr/src/newcore.rs` gives `supports_splice` a
**hard-coded `false` real body** (not a delegation) with a comment naming
the hydration-anchor invariant, pinned by the `newcore_host_is_anchored`
regression. The value matches the Backend default, so there is no
behavior change — but the *mechanism* is explicit.

**Checklist for the de-trait pass**: for each backend above, write the
explicit `Host` body, and pin `supports_splice`'s value with a test
(the terminal/cpu/roku suites already have one:
`newcore_host_splice_matches_backend*`, which itself dies with the old
core — replace the `Backend::supports_child_splice` half with a literal
`false` assertion and keep the test).

**Status — the five live backends (done).** `render-wgpu` inherited four
of the five (`insert_many`, `insert_at`, `remove_child`,
`supports_child_splice`) and now spells them out: `insert_at` appends
and drops the index, `remove_child` is the no-op, `supports_splice` is a
literal `false`, and `insert_many` was DELETED so the byte-identical
`Host` default serves it. `create_anchor` is NOT a landmine there — wgpu
overrides it with its own `NodeKind::ReactiveAnchor`. All five bodies
are pinned by
`gpu-backend/engine/tests/newcore.rs::newcore_host_seam_reproduces_the_deleted_backend_defaults`
(a real headless `Host`, asserting append-order, the no-op, the literal,
the anchor kind and `insert_many` ordering). backend-macos /
backend-ios-mobile / backend-android-mobile inherited two: `create_anchor`
now carries the explicit `create_view(&AccessibilityProps::default())`
body with the web-only-`display:contents` rationale in a comment, and
`insert_many` is deleted onto the `Host` default. Their caps impls are
`target_os`-gated, so the reachable gate is the launched smoke app
(`crates/dev/newcore-*-smoke`), not a host unit test. backend-web
inherited none.

> **DONE for the seven artifact-gated backends** (ssr, terminal, email,
> cpu, roku, linux, windows). Each now carries explicit bodies with a
> comment naming the ported default, and a literal
> `newcore_host_splice_is_anchored` assertion. **Verified the hard way**:
> flipping `supports_splice` to `true` on backend-cpu and
> backend-terminal leaves every frozen framebuffer / grid dump
> byte-identical (the anchor view is visually and — for these scenes —
> layout-inert), and only the literal assertion goes red. The frozen
> corpora alone would NOT have caught that flip. `insert_many` is left
> defaulted everywhere: `Host`'s default is the same N-x-`insert` loop.
> Per-backend Host status: ssr/email/roku already overrode
> `create_reactive_anchor`, `insert` and (ssr/email) `insert_at`, so those
> bodies moved verbatim; terminal/cpu/linux/windows needed explicit
> `insert_at` + `create_anchor`; all seven needed explicit `remove_child`
> and `supports_splice`.

### 2.3 🟠 `NavigatorOps::create_navigator` disappears (it does not fall back)

`crates/runtime/vocabulary/src/caps/nav_overlay.rs` gates
`create_navigator` behind `#[cfg(feature = "legacy-bridge")]` — the only
cfg-gated method anywhere in caps — because its `NavigatorHost` closes
over the old-core `Element`. `legacy-bridge = ["dep:runtime-core"]`.

When `runtime-core` goes, the feature goes, and the method **ceases to
exist** rather than falling back to a default. All 12 `newcore.rs`
delegations must be **deleted**, not re-defaulted. The new core never
calls it (navigators mount through `handlers/navigator.rs` over the
Lifecycle/View caps) — confirmed live this wave: scene-parity dropped
`legacy-bridge` entirely and all three `nav_*` scenarios still pass on
the new-core leg.

> **DONE for the seven artifact-gated backends.** A knock-on worth naming:
> `create_navigator` was the ONLY writer of the backend-side navigator
> registries (`NavigatorRegistry` + the per-instance `NavigatorHandler`
> map on terminal and ssr), so the four surviving `NavigatorOps` methods
> that read those maps became unreachable and now resolve to their caps
> defaults (no-op release / no-op slot style / no-op handle /
> attach-initial drops the screen). The registries themselves, terminal's
> `TerminalNavigatorRegistrar` inventory hook, and ssr's
> `register_navigator` are deleted. Same story for `ExternalRegistry` /
> `register_external` on ssr, linux and windows: third-party primitives
> register scene-`Registry` handlers now, so `create_external` only serves
> `missing_primitive_placeholder` and keeps just its placeholder body
> (the caps default would `unimplemented!()`-panic, which is why it stays
> overridden). **Breakage to hand off**: `crates/sdk/client/toolbar`'s
> `linux.rs`/`windows.rs` and `crates/sdk/client/menu` call
> `backend_{linux,windows}::register_external` — old-core-only External
> legs that need the scene-`Registry` port or deletion.

### 2.4 Caps-only methods (no Backend counterpart — nothing to verify)

| Method | Trait | Caps default | Real impl |
| --- | --- | --- | --- |
| `notify_signal_text_js` | `TextOps` | no-op | web only |
| `notify_signal_value_js` | `StyleOps` | no-op | web only |

### 2.5 Per-backend default-resolved counts

The counts below were **re-derived mechanically** during the de-trait pass
(parse each `impl Backend` block, intersect with the caps method map) and
matched this table exactly for all seven artifact-gated backends: ssr 102,
terminal 120, email 116, cpu 128, roku 115, linux 128, windows 129.


Explicit = methods in the backend's `impl Backend` block. Default-resolved
= caps-mapped methods it does NOT override (i.e. the verification list).

| Backend | explicit / 159 | default-resolved / 152 | Host-seam defaults unoverridden |
| --- | --- | --- | --- |
| backend-web `WebBackend` | 125 | 34 | 0 |
| backend-macos `MacosBackend` | 86 | 71 | 2 |
| backend-ios-mobile `IosBackend` | 85 | 72 | 2 |
| backend-android-mobile `AndroidBackend` | 78 | 79 | 2 |
| dev-server `WireRecordingBackend` | 74 | 82 | 3 |
| render-wgpu `WgpuBackend` | 63 | 92 | 4 |
| backend-ssr `SsrBackend` | 54 | 102 | 3 |
| scene-parity `FullRecorder` | 75 | 84 | 0 |
| mock-backend `MockBackend` | 44 | 115 | 0 |
| backend-email `EmailBackend` | 40 | 116 | 3 |
| backend-roku `RokuBackend` | 40 | 115 | 4 |
| backend-terminal `TerminalBackend` | 34 | 120 | 5 |
| backend-cpu `CpuBackend` | 26 | 128 | 5 |
| backend-linux `LinuxBackend` | 26 | 128 | 5 |
| backend-windows `WindowsBackend` | 25 | 129 | 5 |
| scene-parity `ParityBackend` | 13 | 146 | 0 |
| macos/ios/android `stub.rs` | 9 each | 145 each | 5 each |

The three `stub.rs` impls are never paired with a caps impl (each
`newcore` module is `target_os`-gated), so they are pure code deletions,
not verification rows.

**`ParityBackend` (146/152 default-resolved) is the single
highest-leverage fixture for this pass** — if a caps default ever
diverges from a Backend default, scene-parity's structural goldens move
first.

### 2.6 Useful cross-cuts

**Default-resolved in ALL 12 real backends** — verify the caps default
once, globally (7):
`make_activity_indicator_handle`, `make_image_handle`,
`make_presence_handle`, `make_slider_handle`, `make_toggle_handle`,
`make_virtualizer_handle`, `missing_primitive_placeholder`.

**Overridden by every real backend** — nothing to verify (9):
`create_activity_indicator`, `create_icon`, `create_image`,
`create_portal`, `create_pressable`, `create_scroll_view`,
`create_text_input`, `create_toggle`, `platform`.

**Near-universal defaults** — a wrong caps default here breaks 11 of 12
backends, so verify these first: all nine `WireBindingOps` methods
(default-resolved everywhere except roku), `execute_batch` /
`execute_batch_with_attach` / `supports_batched_repeat`
(everywhere except web), the whole `make_*_handle` family, and
`apply_safe_area_padding` / `apply_scroll_view_safe_area_inset`.

### 2.7 Per-backend default-resolved method lists

The complete per-backend lists (grouped by caps family) are long; they
are reproduced here so the de-trait pass can tick them off by hand.

**backend-web** (34) — A11y: `dump_accessibility_tree`. ActivityIndicator:
`make_activity_indicator_handle`. AppEnv: `set_page_metadata`. Button:
`update_button_label`. Document: `register_raw_css`. External:
`missing_primitive_placeholder`. Icon: `make_icon_handle`. Image:
`make_image_handle`. Input: `claim_touch`. Introspection:
`absolute_frame`, `capture_screenshot`, `device_frame`, `frame`,
`supports_screenshot`. Lifecycle: `renders_lazy_chunks`, `run_layout`,
`schedule_layout_pass`. Portal: `set_portal_hidden`. Presence:
`create_presence_placeholder`, `make_presence_handle`. SafeArea:
`apply_safe_area_padding`, `apply_scroll_view_safe_area_inset`. Slider:
`make_slider_handle`. Toggle: `make_toggle_handle`. Virtualizer:
`make_virtualizer_handle`. WireBinding: all 9.

**backend-macos** (71) — A11y 1; ActivityIndicator 2
(`make_activity_indicator_handle`, `update_activity_indicator_size`);
AppEnv 2 (`set_page_metadata`, `set_scrollbar_theme`); Batch 3; Document
5; External 1; Graphics 2 (`make_graphics_handle`, `release_graphics`);
Icon 4 (`animate_icon_stroke`, `make_icon_handle`, `update_icon_data`,
`update_icon_stroke`); Image 2; Input 1 (`claim_touch`); Introspection 3
(`absolute_frame`, `device_frame`, `note_introspection_root`); Lifecycle 3
(`is_hydrating`, `renders_lazy_chunks`, `run_layout`); Link 2; Portal 1;
Presence 1; SafeArea 2; Slider 1; **Style 18** (`apply_default_text_font`,
`apply_styled_states`, `apply_styled_variants`,
`handles_states_natively`, `install_tokens`, `mark_container`,
`mint_class_for_app`, `mint_style_class`, `on_node_unstyled`,
`register_reactive_class_binding`, `register_stylesheet`,
`release_reactive_class_binding`, `set_disabled`,
`supports_js_class_bindings`, `supports_preminted_styles`,
`token_updates_propagate_via_cascade`, `unregister_stylesheet`,
`update_tokens`); Text 6 (`create_text_with_id`,
`register_reactive_text_binding`, `release_reactive_text_binding`,
`release_text_id`, `supports_js_text_bindings`, `update_text_by_id`);
Toggle 1; Virtualizer 1; WireBinding 9.

**backend-ios-mobile** (72) — as macOS, plus AppEnv `set_app_background`,
Input 3 (`install_file_drop_handler`, `install_hover_handler`,
`install_wheel_handler`), Introspection 4 (`device_frame`,
`introspect_native`, `note_introspection_root`,
`supports_native_introspection`), Portal 2, Presence 2, TextInput 2
(`set_text_input_focus_handler`, `update_text_input_placeholder`); Style
17 (as macOS minus `set_disabled`); Icon 1 (`update_icon_data`).

**backend-android-mobile** (79) — A11y 1; ActivityIndicator 2; AppEnv 2;
Batch 3; Button 1; Document 5; External 1; Icon 2; **Image 5**
(`install_image_error_handler`, `install_image_load_handler`,
`make_image_handle`, `update_image_alt`, `update_image_src`); Input 4;
Introspection 4; Lifecycle 2; Link 2; Portal 2; Presence 2; Pressable 1;
Scroll 2 (`node_scroll`, `set_node_scroll`); Slider 1; Style 16;
TextInput 3; Text 6; Toggle 1; Virtualizer 2; WireBinding 9.

**backend-ssr** (102) — A11y 3; ActivityIndicator 2; Animation 2; AppEnv 4
(`color_scheme`, `fullscreen_setter`, `set_app_key_handler`,
`url_opener`); Asset 2; Batch 3; Button 1; Document 1; External 2; Graphics
2; Icon 5; Image 4; **Input all 6**; **Introspection all 8**; Lifecycle 3;
Link 2; Navigator 2 (`apply_navigator_slot_style`,
`make_navigator_handle`); Portal 3; Presence 3; Pressable 1; SafeArea 2;
Scroll 3; Slider 1; Style 11; TextInput 4; Text 8; Toggle 1; View 1
(`make_view_handle`); Virtualizer 3; WireBinding 9.

**render-wgpu** (92) — ActivityIndicator 2; AppEnv 5; Asset 2; Batch 3;
Button 2; Document 5; External 1; Graphics 1; Icon 2; Image 4; Input 5;
Introspection 6; Lifecycle 4; Link 2; Portal 3; Presence 2; Pressable 1;
Scroll 3; Slider 1; Style 16; TextInput 4; Text 6; Toggle 1; Virtualizer
2; WireBinding 9.

**backend-email** (116) — A11y 3; ActivityIndicator 2; Animation 2; AppEnv
5; Asset 4; Batch 3; Button 1; Document 3; External 2; Graphics 2; Icon 5;
Image 4; Input 6; Introspection 8; Lifecycle 3; Link 2; **Navigator all 5
incl. `create_navigator`** (see §2.3); Portal 3; Presence 3; Pressable 1;
SafeArea 2; Scroll 3; Slider 2; Style 14; TextInput 5; Text 9; Toggle 1;
View 1; Virtualizer 3; WireBinding 9.

**backend-roku** (115) — A11y 3; ActivityIndicator 2; Animation 2; AppEnv
7; Asset 4; Batch 3; Button 1; Document 5; **External all 3 incl.
`create_external`**; Graphics 2; Icon 4; Image 4; Input 6; Introspection
8; Lifecycle 4; Link 3; **Navigator all 5 incl. `create_navigator`**;
Portal 3; Presence 3; Pressable 1; SafeArea 2; Scroll 3; Slider 1; Style
14; TextInput 7; Text 9; Toggle 1; View 1; Virtualizer 4; WireBinding **0
— roku is the only backend that overrides all nine**.

**backend-terminal** (120) — A11y 3; ActivityIndicator 2; AppEnv 5; Asset
4; Batch 3; Button 1; Document 5; External 2; **Graphics all 3 incl.
`create_graphics`**; Icon 5; Image 5; Input 6; Introspection 8; Lifecycle
4; Link 2; Portal 3; Presence 3; Pressable 1; SafeArea 2; Scroll 3;
**Slider all 3 incl. `create_slider`**; Style 19; TextInput 6 (incl.
`create_text_area`); Text 8; Toggle 1; Virtualizer 4; WireBinding 9.
Terminal does NOT default-resolve any `NavigatorOps` method.

**backend-cpu** (128) — A11y 3; ActivityIndicator 2; AppEnv 6; Asset 4;
Batch 3; Button 2; Document 5; External 2; Graphics 2; Icon 5; Image 5;
Input 6; Introspection 8; Lifecycle 4; **Link all 3 incl. `create_link`**;
Navigator 4; Portal 3; Presence 3; Pressable 1; SafeArea 2; Scroll 3;
Slider 2; Style 19; TextInput 7; Text 9; Toggle 2; View 1; Virtualizer 3;
WireBinding 9.

**backend-linux** (128) / **backend-windows** (129) — same family shape as
CPU, plus Animation 2 (`set_animated_color`, `set_animated_f32`);
Button 1 only (both override `update_button_label`); Windows additionally
default-resolves `update_text_input_secure` (TextInput 7 vs linux's 6).

**dev-server `WireRecordingBackend`** (82) — A11y 1; ActivityIndicator 1;
Animation 2; AppEnv 4 (incl. `platform`); Batch 3; Button 1; Document 4;
External 1; Graphics 1; Icon 1; Image 3; Input 6; Introspection 7;
Lifecycle 4; Link 1; Portal 2; Presence 2; Pressable 1; Scroll 3; Slider 1;
Style 11; TextInput 3; Text 8; Toggle 1; Virtualizer 1; WireBinding 9.

**mock-backend `MockBackend`** (115) — A11y 3; ActivityIndicator 2; AppEnv
8 (all); Asset 4; Batch 3; Button 1; Document 5; External 2; Graphics 2;
Icon 5; Image 4; Input 6; Introspection 8; Lifecycle 4; Link 2; Navigator 1;
Portal 3; Presence 3; Pressable 1; Scroll 1; Slider 1; Style 18; TextInput
4; Text 9; Toggle 1; View 1; Virtualizer 4; WireBinding 9.

**scene-parity `FullRecorder`** (84) — A11y 3; ActivityIndicator 1;
Animation 2; AppEnv 8; Asset 3; Batch 3; Button 1; Document 4; External 3;
Graphics 1; Icon 1; Image 1; Input 1; Introspection 8; Lifecycle 4; Link 1;
Navigator 1; Portal 1; Presence 1; Pressable 1; Scroll 3; Slider 1; Style
12; TextInput 2; Text 5; Toggle 1; View 1; Virtualizer 1; WireBinding 9.

**scene-parity `ParityBackend`** (146) — essentially the whole surface:
A11y 3; ActivityIndicator 3; Animation 2; AppEnv 8; Asset 4; Batch 3;
Button 2; Document 5; External 3; Graphics 3; Icon 6; Image 6; Input 6;
Introspection 8; Lifecycle 4; Link 3; Navigator 5; Portal 4; Presence 3;
Pressable 2; SafeArea 2; Scroll 4; Slider 3; Style 19; TextInput 9; Text 9;
Toggle 3; View 1; Virtualizer 4; WireBinding 9.

### 2.8 Coverage-breadth extensions made this wave

Two corpora were thin on caps families their backend actually implements
(or default-resolves visibly), and were extended so the frozen artifacts
would catch a default change:

- **backend-cpu** — new `caps_breadth_leaves.png`: image, icon, activity
  indicator, controlled text input, text area, toggle, slider, plus a
  `link` (default-resolved: `create_link` → `create_view`). Previously
  the corpus reached only view/text/button/pressable/scroll_view/dyn/keyed.
- **backend-terminal** — new `caps_breadth.grid`: image, icon, `link`
  (a real `NodeKind::Pressable` here, NOT the default), activity
  indicator, controlled text input, `scroll_view` clipping an oversized
  child, plus `slider` and `text_area`, both default-resolved — and the
  frozen dump now literally contains the default's
  `[external "… not supported in terminal]` placeholder rows.

No extension was needed for:

- **backend-roku** — the existing `full_scene` already drives every
  primitive family the backend implements (view/text styled + flex, a
  `state hovered` sheet, button, toggle, slider with range + step, text
  input with placeholder, pressable, image, icon, activity indicator,
  scroll view), plus portal, dyn and keyed scenarios.
- **backend-ssr** — `static_kitchen_sink` covers all 13 core primitives;
  the remaining default-resolved surface is imperative-handle and no-op
  plumbing with no serialized output.
- **backend-email** — same, and the headline item is a real component
  template.
- **website ssg_parity** — 33 real routes.
- **scene-parity** — 27 full-op scenarios; and `ParityBackend` /
  `FullRecorder` are themselves the widest default-resolution fixtures in
  the tree (see §2.5).

### 2.9 What could NOT be frozen, and why

**backend-web, backend-macos, backend-ios-mobile,
backend-android-mobile and render-wgpu have no frozen cross-core corpus**,
and deliberately so:

- **web / macOS / iOS / Android** never had an old-vs-new artifact gate to
  freeze. Their adoption evidence is live verification (Playwright in a
  real browser; `newcore-*-smoke` apps launched on a real window,
  simulator, and emulator) plus host-side unit suites. There is no
  serializable output to capture — the "output" is a live DOM /
  `NSView` / `UIView` / `ViewGroup` tree. A structural op-log corpus
  could be built for them, but it would be a *new* fixture asserting the
  new core against itself, not a frozen record of the old core, so it
  would add no old-core testimony. `scene-parity`'s op-log goldens
  already play that role backend-independently.
- **render-wgpu** *can* capture pixels headless (real Metal, via
  `Screenshotter`), but GPU rasterization is not bit-reproducible across
  drivers, GPUs, and OS versions, so a frozen framebuffer would be a
  flaky golden that gets silently re-baselined — the exact failure mode
  this whole exercise is meant to prevent. Its 22 host-e2e new-core tests
  (`crates/gpu-backend/engine/tests/newcore.rs`) drive the real `Host`
  and assert structure, not pixels; that is the right shape for this
  backend.

Consequence for the de-trait pass, stated plainly: the default-resolved
lists for those five backends in §2.7 are verified **by hand only**, with
no artifact backstop. macOS/iOS/Android default-resolve 71–79 methods
each; render-wgpu 92; web only 34. §2.1's pairwise default audit and the
scene-parity live proof are what make that hand-verification tractable —
but it is hand-verification.

---

## 3. Pre-deletion test-count baseline

Measured on this tree, this session (`CARGO_INCREMENTAL=0`, shared
workspace target dir). "Both cores" means the crate has two invocations;
each is listed.

### Core matrix

| Suite | Invocation | Passing |
| --- | --- | --- |
| runtime-world | `cargo test -p runtime-world` | **73** |
| runtime-scene | `cargo test -p runtime-scene` | **27** |
| runtime-vocabulary | `cargo test -p runtime-vocabulary` | **183** |
| runtime-vocabulary (robot) | `… --features robot` | **183** |
| scene-parity | `cargo test -p scene-parity` | **102** (goldens 20, goldens_full 30, goldens_full_new 31, goldens_new_core 21) |
| **matrix total** | | **385** |
| runtime-core (old core) | `cargo test -p runtime-core` | **411** |
| runtime-shared (survivor) | `cargo test -p runtime-shared` | **457** |
| runtime-macros | default (old-core emission) | **137** |
| runtime-macros | `--features new-core` | **162** |

### Backends with a frozen corpus

| Crate | old-core leg | new-core leg |
| --- | --- | --- |
| backend-cpu | 25 | **35** |
| backend-terminal | 2 | **9** |
| backend-roku | 5 | **12** |
| backend-ssr | 27 | **39** |
| backend-email | 10 | **15** |
| mock-backend | 46 (single invocation, both cores in one binary) | — |
| dev-server | 42 | — |
| wire (protocol codec) | 36 | — |
| website (`--features ssr`, lib + ssg_parity) | 13 (`--no-default-features --features old-core,ssr`) | **11** |

### Backends without a frozen corpus (§2.9)

| Crate | Invocation | Passing |
| --- | --- | --- |
| render-wgpu | `cargo test -p render-wgpu --features new-core` | 33 default + **22** new-core |
| newcore-app (dual-core proof crate) | `cargo test -p newcore-app` | **23** (22 e2e + 1 recipes); old-core leg is a check |
| host-mock | `cargo test -p host-mock` | compile-time `AllCaps` proof only |
| idea-ui-nav | `--features new-core-harness` / `--features old-core` | **12** each |
| SDK new-core legs | codeblock 2, swap 4, stack 3, table 8 | — |

The new-core legs are supersets: they include the crate's default tests
plus the parity/golden suite.

### Conformance (documented facts, not re-run this session)

The robot-driven cross-platform conformance app
(`crates/dev/robot-e2e/examples/conformance`, 5 suites):

- **old core: 8/9.** The sole failure is the pre-existing, unrelated
  `component methods` static-label test (`MethodCounter`) — it also fails
  on a clean `HEAD` worktree and is deliberately untouched.
- **new core: 9/9** (primitives 5, modal 1, stack-nav 1, idea-ui 1,
  methods 1).

The new core passes every test the old core passes, plus the one the old
core fails. When the old-core leg is deleted, **9/9 is the surviving
baseline**; nothing regresses because the old core's 8/9 was strictly
weaker.

### What disappears from the counts at deletion

- runtime-core's **411** tests (see §4).
- Every `old-core` invocation in the table above — the *invocations*
  vanish; the behaviors they covered are covered by the new-core legs
  plus the frozen corpora.
- scene-parity's `goldens` (20) and `goldens_full` (30) binaries — the
  old-walker halves. `goldens_new_core` (21) and `goldens_full_new` (31)
  survive against the SAME committed goldens, so **52 of 102 scene-parity
  tests survive and keep pinning the old core's recorded behavior**.

---

## 4. Tests that legitimately die with the old core

Classification of every test target that goes away, so nothing valuable
is dropped by accident. Categories:

- **DIES-legit** — tests old-core *mechanics* that will not exist.
- **DIES-covered** — the behavior survives; the named new-core test
  covers it.
- **DIES-uncovered** — 🔴 coverage would be lost. Must be addressed
  before the deletion lands.

Scope: **`runtime-core` = 437 tests** (356 in `tests/`, 81 inline in
`src/`), plus 55 in `dev/mock-backend/tests`, 50 in `scene-parity`, 9 in
`vocabulary/tests/bridge.rs`, ~28 in `sdk/client/*` old-core legs, and a
tail of old-core-driven targets across `backend/*`, `dev/*`, `ui/*`,
`websites/*`, `mcp/catalog`.

A fourth category turned up while inventorying and is worth naming
separately, because it is *not* a coverage loss and *not* a legitimate
death:

- **SV-R (survives, must be RELOCATED)** — the test's subject lives in
  `runtime-shared` (the legacy reactive arena, `identity`, `introspect`,
  the primitive prop structs, `scheduling`), but its host crate dies. If
  the file is deleted with `runtime-core`, coverage of a **surviving**
  module goes with it. These are the cheapest losses to avoid and the
  easiest to miss.

### 4.1 🔴 DIES-uncovered — coverage lost unless addressed in-wave

Ordered by severity. **This is the list that must be worked, not just
read.**

| # | What | Where it dies | Why it matters |
| --- | --- | --- | --- |
| 1 | **Browser-history / URL-sync behavior — goes to ZERO** | `mock-backend/tests/navigator_url_sync.rs` (9) | `backend/web/src/newcore_url_sync.rs` (~500 lines) has **0 tests**, and `backend/web/tests/` holds only `minify_shims.rs`. `vocabulary/tests/walker_ports.rs` ported the *seam* halves and explicitly defers browser behavior to that untested module. Lost: one `pushState` per Select/Push · popstate reconcile writes no history · **a root pop must not `history.back()` out of the app** · browser Forward · scroll snapshot/restore · cold-start `replaceState` claim · **a root claim must not clobber a nested URL slice** (the "cold `/alerts` rewritten to `/`" bug) · no duplicate entry on re-selecting the active URL. The `SimHistory` fake browser exists only in this file. |
| 2 | **Hydration — goes to ZERO** | `runtime-core/tests/walker/hydration.rs` (6) + `navigators/stack/tests/hydration_web.rs` (1 wasm) | The `is_hydrating()` anchor-adoption contract (skip `clear_children`; use an anchor even when splice is supported) and the only test in the repo that **hydrates a navigator over SSR HTML**. The frozen SSR/SSG corpora prove server *output* is byte-identical, which is the precondition for adoption — they do not exercise the client adopt path. ⚠️ **PARTLY ADDRESSED (SDK flattening wave)**: `navigators/stack/tests/hydration_web.rs` was PORTED to `backend_web::newcore::hydrate_in` + `backend_ssr::newcore::render_path_with` and still asserts the once-only screen build + a single marker copy in the DOM. It is a `wasm_bindgen_test`, so `cargo check --target wasm32-unknown-unknown` COMPILES it but no in-repo gate RUNS it (`wasm-pack test --headless` is the only runner). The 6 `walker/hydration.rs` cases remain uncovered. |
| 3 | **MCP catalog emission (41 tests, 1335 lines)** | `mcp/catalog/tests/registers_component.rs` | The whole catalog contract: doc capture, `composes` edges from real `ui!`/`jsx!` bodies, tool/param specs, methods, animations, scopes. Stubs return `runtime_core::Element` so the real macro expansions typecheck. The two cores emit **different** inventory paths (`runtime_core::__mcp` vs `glue::__mcp`) and only the old one is tested end-to-end. This is the surface the `idealyst` MCP server serves.  ✅ **CLOSED (catalog/vocabulary wave)**: the 1335-line body moved to `crates/dev/newcore-catalog/tests/shared/catalog_emission.rs` and is `include!`d by BOTH legs — `crates/mcp/catalog/tests/registers_component.rs` (old anchor) and `crates/dev/newcore-catalog/tests/registers_component.rs` (facade alias ⇒ retargeted `glue::__mcp` for `#[component]`, facade-root `__mcp` for the derive/tool/`recipe!`/`doc_scope!` emissions, which do NOT pass through `runtime_macros::finish`). Same-source ⇒ same inventory, pinned explicitly by `catalog_inventory_is_identical_across_cores` (a sorted fingerprint of every macro-emitted slice, expected value a literal in the shared source, so both legs assert the same string; `file`/`line` excluded because `file!()` reports the include path). Needed three same-source fixes: `counter` moved to INLINE props (`#[method]`'s legacy explicit-props form is a named new-core compile error) with `set(get()+n)` instead of `update`; a `stub_view()` helper (`IntoElement::into_element(view(..))` resolves on both roots) since `view()` returns a builder on the new core; and a new `runtime_vocabulary::animated!` mirror re-exported from the facade (the old macro constructs the SHARED `AnimatedValue`, whose inherent `bind*` is inert off the old core). mcp-catalog cannot host the new leg itself: `runtime-facade` depends transitively on `mcp-catalog` (cargo cycle) and dev-deps cannot be optional. |
| 4 | **Per-primitive reactive plumbing (18 tests, 7 primitives)** | `runtime-core/tests/{activity_indicator_size_reactive, icon_reactive_data, image_alt_reactive, link_url_reactive, text_input_secure_reactive, preserves_focus, text_input_blur}.rs` | Each pins the same triple: live source → in-place `update_*` with no rebuild · **static source installs NO effect** · **effect freed on Owner drop**. The new core has that triple for exactly ONE primitive (`vocab.rs::{const,dyn}_button_label_*`); `caps_conformance.rs` only proves the caps are callable. The two halves that silently regress are the ones unguarded. Includes `mark_preserves_focus` (the Autocomplete mouse-selection fix) and cancelable `BlurOutcome`.  ✅ **CLOSED**: `crates/runtime/vocabulary/tests/reactive_prop_plumbing.rs` (21 tests) covers the full triple for icon `data`, image `alt`, link `url`, activity-indicator `size`, text-input `secure` and `placeholder`, plus `preserves_focus` marks (view + pressable, and unmarked nodes) and the cancelable `on_blur` `BlurOutcome` (Keep/Allow/absent). Half 2 ("static installs no effect") asserts no `update_*` op at mount and nothing on a later flush; half 3 ("effect freed") asserts backend silence after teardown AND a `Weak` probe on an `Rc` moved into the source closure upgrading to `None` — strictly stronger than the old arena's `effects_in_use` balance, which the world kernel has no analogue for. host-mock gained the `on_blur`/`on_key_down`/file-drop handler captures and richer `update_icon_data` (paths) / `create_text_area` (wrap + row bounds) records. |
| 5 | **Wire / hot-reload chain** | `dev/wire/tests/roundtrip.rs` (11 of 17), `dev/wire/tests/transport.rs` (2), `mock-backend/tests/{wire_external_payload, wire_navigator_outlet, wire_safe_area, wire_screenshot}.rs` (7), `dev/server/tests/{runtime_server_shell_e2e (5), welcome_pipeline_end_to_end (2), aas_headless (9)}`, `dev/client/tests/reconnect_reconcile.rs` (1) | `TraceBackend`, `MockBackend`, `WireRecordingBackend` and `WireBackend` are all `impl Backend`. Unguarded if not retargeted: the AX-action reverse channel (`Rc<dyn Fn()>` → HandlerId → trampoline → `dispatch_event`), External-payload serde round-trip, navigators needing **no** kind-specific wire commands, safe-area over the wire + late-joiner persistence, the Robot/MCP `screenshot` verb chain, the **animation emit path** (`AnimatedValue::bind` → `SetAnimatedF32`), reconnect field-fold re-apply, real-socket e2e, and the production **`RuntimeServerShell`** that iOS/Android/macOS run in hot-reload mode. |
| 6 | **`walker/style.rs` (8 inline)** | `runtime-core/src/walker/style.rs` | Breakpoint + container overlay merge: sort-ascending-not-declaration-order, mobile-first layering by viewport/container width, zero-width ⇒ base, and the **convergence property** the native container-query feedback loop depends on (merging twice at one width is byte-identical, so the change-guarded signal never re-fires). `runtime-shared` tests the *primitives*, not the merge.  ✅ **CLOSED**: ported verbatim into `crates/runtime/vocabulary/src/style_attach.rs::overlay_merge_tests` (8), against the new core's own private `resolve_*_overlays` / `merge_active_*` — including the same-`Rc` fast path, min-width inclusivity, the zero-container-width ⇒ base case (which is what makes the documented "no container signal on the new core" deferral safe rather than merely stated) and the convergence property. |
| 7 | **`scheduling_scoped.rs` (11 of 16 not mirrored)** | `runtime-core/tests/scheduling_scoped.rs` | `vocabulary/tests/scoped_scheduling.rs` (5) mirrors cancel-on-rerun / nested-anchor / inert-outside-scope / `timeline!`. Not mirrored, and several are documented crash-or-leak regressions: `raf_loop_scoped` (and its `after_ms` twin) must not fire after scope drop **even if the browser already dispatched** · skip-a-frame while the arena is busy · `after_ms_detached` pending-then-fires · the detached sweep never cancelling live tasks · idempotent explicit-cancel-plus-scope-drop · nested-effect anchor inheritance.  ⚠️ **MOSTLY CLOSED**: `vocabulary/tests/scoped_scheduling.rs` gained the two documented CRASH regressions (`regression_{raf_loop,after_ms}_scoped_stays_inert_after_owner_drop_even_if_already_dispatched` — the body is STOLEN from the test scheduler so cancellation cannot reach it, and reads a scope-owned signal so a leak aborts), plus `after_ms_scoped_fires_while_the_anchor_owner_is_alive` and `on_cleanup_coexists_with_scoped_helpers_in_the_same_effect`. NOT ported, with reasons: the busy-skip test (`raf_loop_scoped_skips_a_frame_while_reactive_arena_is_busy`) has **no new-core analogue by design** — staging is re-entrancy-safe, so there is no busy state to skip (`scoped_scheduling.rs` module docs); `explicit_cancel_inside_scope_is_idempotent_with_scope_drop` has no surface — the new `after_ms_scoped`/`raf_loop_scoped` return `()`, cancellation is the anchor's alone; and the two `after_ms_detached_*` tests are **SV-R** (subject is `runtime_shared::scheduling`, which survives) → relocate to `crates/runtime/shared/tests/`. |
| 8 | **`newcore-app/tests/e2e.rs` (23) + `idea-ui-nav/tests/*` (7)** | via `dep:scene-parity` / `LegacyBridge` | ⚠️ **Partly addressed in an earlier wave**: both were moved onto the bare `FullRecorder`, which implements `Host` + all 30 caps directly, so they no longer touch `LegacyBridge`.  ✅ **CLOSED**: the scene-parity split has landed — the old halves (`src/{scenarios,scenarios_full}.rs`, `tests/{goldens,goldens_full}.rs`) are deleted and the core-free root survives, so **52 of the original 102 tests survive against UNCHANGED goldens** and nothing went silent. The 50 that die are the old-walker halves, exactly as forecast in §3. 🔴 **Two `#![cfg]`-goes-silent instances WERE found and fixed elsewhere this wave** — the failure mode this row warned about is real: `dev-client/tests/newcore_caps_replay.rs` and `dev-server/tests/newcore_robot_catalog.rs` both opened with `#![cfg(feature = "new-core")]`, and when that core-selector feature was deleted each silently became ZERO tests instead of a compile error. Both are unconditional now (1 + 1 tests restored). **Anyone deleting a feature must grep `^#!\[cfg` in every test dir that forwarded it.** |
| 9 | **`stack_depth.rs` (1)** + `wire_card_overflow_repro.rs::deep_tree_overflow_multi_site` (`#[ignore]`) | runtime-core / mock-backend | The only records of the tree-depth recursion budget (`build_inner` 77 KB → 2.3 KB wasm frame; the `/demo` `memory access out of bounds` bug). `runtime_scene::realize` recurses too and had **no depth-budget test**.  ✅ **CLOSED, and the new core is strictly better.** Measured on this tree: the old walker needed ~20 KiB of stack per nesting level (≈50 levels on wasm-ld's 1 MiB default); `realize` needs **~1.1 KiB** — 1 MiB carries ~900 levels and aborts at 1000. No robustness regression; an ~18× improvement. Three successors in `runtime-scene/src/tests.rs`: `deep_nested_items_realize_within_wasm_stack_budget` (the direct port — SAME depth 30 and SAME 1 MiB constrained thread as the old test, so the old guarantee is provably carried over rather than restated), `realize_per_level_stack_cost_stays_within_its_measured_budget` (400 levels on 1 MiB — a ~2.4× margin, so it trips if one level ever costs more than ~2.6 KiB), and `realize_past_the_depth_cap_panics_by_name_instead_of_overflowing`. That last one closes something the old core never had: `realize::depth` adds an explicit `MAX_DEPTH = 512` RAII guard, so an unbounded-recursion component body raises a NAMED panic instead of an opaque wasm `memory access out of bounds` trap. `depth_counter_unwinds_to_zero_after_success_and_after_a_panic` pins the RAII half. |
| 10 | **`walker/theme_cohort.rs` (1)** + **`robot_scroll_action.rs` (1)** | runtime-core | Two documented **hard-abort** regressions: `reset_theme_cohort_state` must be panic-safe when a cohort TLS cannot be `borrow_mut`-ed (the terminal-app "thread local panicked on drop" abort at process exit), and `Robot::set_scroll` must route via the backend's `ScrollViewHandle`, not `set_node_scroll` under a held `borrow_mut` (the live macOS "RefCell already borrowed" abort).  ✅ **CLOSED**: `crates/runtime/vocabulary/tests/hard_abort_regressions.rs` (4, `--features robot`). The theme half is re-homed on `theme::LAST_CTX` — the one TLS in the new engine whose lifetime spans worlds — plus a structural test that cohort teardown from an `Owned`/`Realized` drop never consults the ambient world (the new core's equivalent guarantee: `cohort_unregister` is a METHOD on a mount-time-captured ctx, so there is no TLS for a destructor to miss). The scroll half is **stronger than the original**: a purpose-built host hands out a real `ScrollViewHandle` whose `scroll_to` RE-ENTERS the same backend `RefCell`, standing in for the synchronous native scroll notification whose reactive restyle re-borrows. It asserts both that the offsets reach the backend (the old assertion) and that the call stack is borrow-free at the write (the invariant the old test could only state in prose). **Proven failing**: routing `set_scroll` under a held `backend.borrow_mut()` turns `regression_set_scroll_write_can_reborrow_the_backend_mid_call` red. |
| 11 | ~~`headless_screenshot.rs` (3)~~ + ~~`idea-ui-docs-gpu/tests/render.rs` (1)~~ | gpu-backend / websites | **CLOSED.** `Screenshotter` grew `mount_scene(register, build)` — `newcore::start` on the headless host's own backend — and `mount_and_capture_png` now takes `FnOnce() -> runtime_scene::Element`. Both suites are retargeted onto it (`idea-ui-docs-gpu` calls `newcore::start(shot.backend(), …)` directly so it can pass the app's `register_scene_extensions`). Verified: `cargo test -p render-wgpu --features headless --test headless_screenshot` → 3 passed on real Metal. The dev-server `headless-screenshot` Robot verb rides the same substrate; its wire-replay half (`screenshot_commands`) is blocked on `dev-client`'s caps re-bound, not on `Screenshotter`. |
| 12 | **Navigator SDK legs** | `navigators/stack/tests/{stack_local.rs (1 of 6), ssr.rs (3), recipes.rs}`, `navigators/swap/tests/ssr.rs (3)` | Nothing covers `regression_rebuild_cold_deep_link_never_mounts_parent_until_pop` (parent builds == 0 while deep-linked, builds only on first pop). SSR-through-the-SDK (author chrome, deep path, route collector, `render_all` crawl) exists only here. And `recipes` modules are `cfg(not(new-core))`-gated, so the deletion removes **both navigator MCP recipes**, not just their test.  ⚠️ **RECIPES CLOSED**: both navigator recipes are now core-independent static data in `runtime_shared::recipes`, with their sources at `crates/runtime/shared/recipes/{swap_three_screens_tab_bar,stack_two_screens}.rs` (the wave-2b core-primitive-recipe pattern) — they could not stay in the SDKs and be same-source, since each SDK carried an unconditional `runtime_core` dep, so `::runtime_core::Element` in a recipe body was the OLD Element even in a new-core graph. Registration + served-source assertions (the extension-trait import lines, `recipes_for(<NavigatorType>)`) ported from the deleted `stack/tests/recipes.rs` into `runtime_shared::recipes::tests::navigation_recipes_register_with_their_import_lines`; the compile-and-realize gate is `newcore-app/tests/recipes_compile.rs`, which builds both against the SDKs' real surface. ✅ **REST CLOSED (SDK flattening wave)**: `stack/tests/stack_local.rs` was PORTED onto `crates/dev/host-mock` — 5 tests covering both retention modes and BOTH cold-deep-link regressions, `regression_rebuild_cold_deep_link_never_mounts_parent_until_pop` included (its parent-builds==0 counter is now an effect-cleanup counter: `runtime_world::on_cleanup` panics outside a running effect, so the screen body creates a dep-less effect whose cleanup fires when the screen scope drops) — plus `pop_at_root_is_a_noop`. SSR-through-the-SDK is `stack/tests/ssr.rs` + `swap/tests/ssr.rs`, 3+3 on `backend_ssr::newcore::{render_path_with, render_all}`. |
| 13 | **`inline_props.rs` (7)** + **`component_dispatch.rs` (2)** | runtime-core/tests/walker | The end-to-end props-arrival contract (values arrive, declared defaults, `Signal` threaded un-wrapped, signal→data prop arrives `Dynamic`, `#[prop(static)]`, `children:` param) and struct-literal `BuildElement` dispatch. The new core has *emission* tests in `runtime-macros` but no behavioral prop-arrival suite. Note the sanctioned divergence: omitting a required signal prop panics on the old core, mints a fresh signal on the new one.  ✅ **CLOSED**: `crates/dev/newcore-app/tests/props_arrival.rs` (9) — values arrive, declared defaults, signal→data prop arrives `Dynamic` (`is_static() == false`), a `Signal`-typed prop is threaded UN-wrapped (the child's write lands on the parent's slot), `#[prop(static)]`, optional callbacks (None default + passthrough), the `children:` param, and explicit-struct `BuildElement` dispatch with a required signal prop surviving `..defaults()`. The old `omitting_required_signal_prop_panics_loudly` is replaced by `omitted_required_signal_prop_mints_a_fresh_signal` — the sanctioned divergence, now pinned instead of merely documented. |
| 14 | **Event-plumbing installs** | `walker/{key_events.rs (6), file_drop.rs (2), scroll_view_on_scroll.rs (2)}` | `on_key_down` + `KeyOutcome::PreventDefault` propagation, `secure` threading, TextArea wrap/code-mode defaults; `install_file_drop_handler` install + fire; `on_scroll` register + fire with offsets. `caps_conformance.rs` proves callability only.  ✅ **CLOSED**: `crates/runtime/vocabulary/tests/event_plumbing.rs` (9) — `on_key_down` registration + delivery of the AUTHOR closure, `KeyOutcome::PreventDefault` propagation, no-handler when absent, text-area wrap default + code-mode opt-out (now observable: host-mock records the create-time config), `install_file_drop_handler` absent/installed + fired through all phases with the accept-the-drag return value, and `on_scroll` register/fire-with-offsets/absence + a one-copy-only teardown probe. |
| 15 | **Macro-lowering field regressions** | `tests/{fstring_text.rs (9), if_reactive_lowering.rs (5), match_reactive_call_regression.rs (5), a11y_macro.rs (7), catalog_macros_noop.rs (1)}` | Real field reports: **B4** (`match key(state)` froze), **B5** (default-arm non-`Copy` capture), the 0.4.0 inverted `if` gate with borrowed captures, and the `JsBindingSpec` **parallel-array arity** that the web text fast path depends on. Plus a11y attr lowering end-to-end and the "no-`catalog` no-op resolution" class that the new lowering can re-break. |
| 16 | **`backend-email` headline golden** | `backend/email/tests/newcore_golden.rs` | ⚠️ **Mitigated this wave**: the old core's render of the REAL idea-ui-mail template is now frozen at `tests/goldens/idea_ui_mail_welcome.{html,txt}`, so the *output* is preserved forever. Still lost: any test that **compiles** idea-ui-mail's real components — the crate has no `new-core` feature at all. The frozen file plus the hand-maintained replica are the whole gate. |
| 17 | Smaller, still real | `walker/hydration`-adjacent + `style_dynamic_gating.rs` (4), `external.rs` (2 of 13: External `on_touch`/`on_hover` slot routing — the clickable-table-row fix — and `build_detached` adopt), `primitives/link.rs` (`regression_reactively_remounted_link_keeps_navigator`), `primitives/overlay.rs` (`click_through` ⇒ `pointer-events:none`, the empty-ToastHost regression), `primitives/image.rs` (asset sentinel URL, `on_load` natural dimensions), `styled_text.rs` (theme-cohort re-realize of runs), `toolbar/src/items.rs` builders (zero unit tests), `video::url(..).resolve()`, `webview`/`form` `ui!`-macro path tests, `dev/server/tests/newcore_robot_catalog.rs` (subject sits on a `Backend`-delegating recorder) | Each is one named regression or one uncovered surface. |


#### Rows added while working the list (not in the original inventory)

| # | What | Where | Why it matters |
| --- | --- | --- | --- |
| 18 | 🔴 **`use_id()` / `use_id_keyed()` are position-independent on the surviving core** | `runtime_shared::identity` (survivor) + `runtime_scene::realize` | **A real behavioral regression, not a test gap.** The documented contract is "deterministic per position in the tree". The OLD walker delivered it by calling `with_current_identity` before every emission (`runtime-core/src/walker.rs::build`, plus per-row in `walker/view.rs` and per-screen in `walker/navigator.rs`). `realize` sets no ambient identity, so every call answers from `Identity::UNIDENTIFIED` and all call sites in a tree return the SAME string. It is stable and non-panicking, which is why it would ship unnoticed. **Same root cause** as the already-documented dev-server gap ("Identity-keyed node dedup across re-mounts", `dev_server::newcore` module docs): the recorder reuses wire `NodeId`s across hot-reload rebuilds by ambient identity, so today every re-mount mints fresh ids and clients do a full rebuild instead of an incremental patch. **One fix closes both**: seed the ambient identity per mount site in the vocabulary drivers / `realize`. Pinned meanwhile by `glue_reactive_surface.rs::use_id_is_currently_position_independent_because_the_renderer_seeds_no_identity`, which goes red the moment the renderer starts seeding. Documented for authors in the migration guide's root-surface table ("Restored but DEGRADED"). |
| 19 | 🔴 **The author-facing `runtime_core::` root lost ~30 items** | `runtime_vocabulary::glue` | The old root was `pub use runtime_shared::*;`; the facade root enumerates. Anything nobody listed silently stopped resolving — `announce`, `color`, `open_url`, `set_fullscreen`, `set_app_key_handler`, `host::color_scheme`, `use_id`/`use_id_keyed`/`Identity`, `on`, `memo_with`, `reducer`, `async_reducer`, `flat_list`/`fixed_size`, the file-drop and wheel event PAYLOADS, the gesture-recognizer contract, `active_touch_claim`, `schedule_microtask`, `logging`. Several were actively breaking builds. ✅ **CLOSED**: all restored, with the four that ride old-arena reactivity (`on`, `memo_with`, `reducer`, `async_reducer`) REIMPLEMENTED on the world kernel rather than forwarded — a forwarded call would have compiled and then silently done nothing. Pinned by `runtime-vocabulary/tests/{glue_host_surface,glue_reactive_surface}.rs` (18 tests, path-identity pins). Full accounting, including what is deliberately gone (`batch`, `cycle`, `style-dynamic`, `Signal::dispose`, `arena_stats`) and the newly-public `world_is_entered()`, is the migration guide's "The `runtime_core::` root: what moved, what is gone, what replaced it" table. |
| 20 | 🟠 **The reactive profiler emits nothing** | `runtime_shared::debug` ↔ `runtime-world` | The `reactive_profile` robot verb and the Inspector's profiler tab are permanently empty on runtime v2. All the instrumentation (`record_txn_enter/exit`, `record_effect_run`, `record_commit`, `record_signal_created`, `record_effect_created`) is hooked into the OLD arena's `fan_out_now` / batch flush / effect runner in `runtime-shared/src/reactive.rs`; **`runtime-world` has zero counterparts** — no `debug-stats` feature, no `#[track_caller]`, no `Effect::raw_id`. The verb still answers (the vocabulary bridge has no `reactive_profile` arm, so it falls through to the old bridge) and drains an event log nothing writes. Not closed in this wave — the remediation plan (hook points, identity capture, and the missing bridge arm) is in the wave report. |
| 21 | 🟢 **`style-dynamic` was obsolete, and is now deleted** | `runtime-shared`, `runtime-core` | Resolved as obsolescence rather than restoration. It gated **nothing**: `runtime-shared` had zero `cfg(feature = "style-dynamic")` blocks, `runtime-vocabulary/src/style_attach.rs` matches all six `StyleProp` arms unconditionally, `backend-web` had already dropped its forward (so the documented app-side edit was an unknown-feature error), and feature unification force-enabled it in any graph containing the vocabulary. It was the last remnant of the `prim-*` bundle-size gating model stage 2 removed by decision. Removed from `runtime-shared` (and from its `style-dump` implication); `runtime-core` keeps a local, non-forwarding gate purely for the dying walker's own cfgs. `docs/styling.md` rewritten. **Three stale author-facing instructions remain in peer-owned files** and are named in the wave report. |

### 4.2 🟡 SV-R — survives, but must be RELOCATED (subject is in `runtime-shared`)

Delete these with `runtime-core` and you lose coverage of a module that
is *not* being deleted.

| Path | # | Subject that survives |
| --- | --- | --- |
| `runtime-core/tests/reactive/**` (11 files) + `reactive.rs` | 75 | ✅ **RELOCATED** to `crates/runtime/shared/tests/reactive.rs` + `tests/reactive/**` (12 files incl. the `counted` harness). **75 → 75, exact** (`--features async-driver`; 68 without, `resource` is gated). Path rename only, with TWO exceptions flagged in-place by a `RELOCATION NOTE`: `dispose::scope_owned_signal_freed_by_scope_after_early_dispose_is_safe` and `split::regression_stale_write_half_is_a_safe_noop_after_scope_drop` opened their owning scope by RENDERING through the walker's `TestRuntime`; the scope was scenery, and both now drive `reactive::Scope` / `with_scope` directly — the SAME batched-free path with the walker out of the picture. |
| `runtime-core/tests/identity.rs` | 12 | ✅ **RELOCATED whole** to `crates/runtime/shared/tests/identity.rs` (12 → 12) rather than triaged: four cases have no inline twin (`nested_with_current_identity`, `use_id_outside_scope_is_stable`, `hash_key_is_deterministic`, `unidentified_is_distinct`), and the inline 16 reach private items while these reach only the public surface — the half the facade re-exports. 🔴 **Finding**: `use_id()`'s per-position contract is currently DEAD on the new core (see §4.1 #18). |
| `runtime-core/tests/native_introspect.rs` (4 of 6) | 4 | ✅ **RELOCATED** to `crates/runtime/shared/tests/introspect.rs` (4 → 4). `shared/src/introspect.rs` still has zero inline tests, so this file IS the module's coverage. The other 2 were NOT relocated and are DIES-legit, said explicitly in the new file's header: both drove the OLD walker's registry wiring (`TestRuntime` + the walker-attached introspection closure + old-core `ui!` emission); their successor is the vocabulary robot registry's `--features robot` suite. |
| `runtime-core/src/primitives/virtualizer.rs` | 7 | ⚠️ **5 of 7 RELOCATED** inline into `crates/runtime/shared/src/primitives/virtualizer.rs` (which had zero tests): the 4 pure lane-math cases byte-for-byte, plus `default_layout_is_a_vertical_single_lane_list` rewritten against `VirtualLayout::default()` (same data, the builder is what dies). The other 3 drove the dead `Bound<VirtualizerHandle>` builder (`.axis`/`.lanes`/`.spacing`/`.gap` writing into `Element::Virtualizer`); their successor on this core is `runtime-vocabulary/tests/virtualizer_graphics.rs`, which drives the `VirtualizerPrim` builder and asserts the same resulting `VirtualLayout`. **This is a rewrite, not a relocation, for those 3 — said plainly.** |
| `runtime-core/src/primitives/presence.rs` | 10 | ✅ **RELOCATED** inline into `crates/runtime/shared/src/primitives/presence.rs` (10 → 10, verbatim; only `crate::Easing` → `crate::style::Easing`). Pure data — the types always lived in shared; only the `Element`/`Bound` constructor was in runtime-core, and these tests never touched it. |
| `runtime-core/src/primitives/flat_list.rs` | 3 | ✅ **RELOCATED** inline into `crates/runtime/shared/src/primitives/flat_list.rs` (3 → 3, verbatim). |
| `runtime-core/tests/style.rs` | 25 | ⚠️ **12 of 25 RELOCATED** to `crates/runtime/shared/tests/style_tokens.rs` (`shared/src/style.rs` had zero inline tests): the whole token-registry half — `Tokenized` constructors, `install_tokens`/`update_tokens` round-trip, per-token reactive subscription, per-token SCOPING, the pending-map ordering invariant. The other 13 are walker-driven (`TestRuntime` + backend `Event`) or `stylesheet!`-driven and cannot live in runtime-shared. **Successors, named**: sheet registration / sweep / signal-class fallback / state overlays → `runtime-vocabulary/tests/vocab.rs`; breakpoint + container overlay merge → `style_attach.rs::overlay_merge_tests` (§4.1 #6, closed in an earlier wave). 🔴 **Still genuinely uncovered** (unchanged from the original note): the native container-query FEEDBACK LOOP (`container_overlay_reapplies_on_container_width_crossing_native`), the breakpoint bucket-change re-apply pair, typeface asset emission (`typeface_emits_register_asset_with_real_bytes_then_register_typeface`), and the 3 `stylesheet!`-builder Static-vs-Reactive source tests. |
| `runtime-core/tests/common/counted.rs` | 0 | ✅ **RELOCATED** with the suite, to `crates/runtime/shared/tests/reactive/counted.rs`. |
| `mock-backend/tests/wire_virtual_layout_roundtrip.rs` | 2 | Core-agnostic; relocate to `dev/wire/tests/` or `dev/server/tests/`. |
| `dev/server/tests/{aas_async_reactor,scheduler_raf_reentrancy}.rs` | 4 | ✅ **DONE** — `runtime_core::` → `runtime_shared::`, in place (dev-server survives, so no move was needed). Green on both legs. |
| `scene-parity/src/lib.rs` (the non-`Backend` half) | 0 | ✅ **DONE.** The split has landed: `src/{scenarios,scenarios_full}.rs` and `tests/{goldens,goldens_full}.rs` are deleted, `src/lib.rs` keeps the core-free `PNode`/`Recorder`/`Step`/`Mode`/`serialize_steps`/`golden_path`, and the crate is `goldens_new_core` (21) + `goldens_full_new` (31) = **52 against the SAME committed goldens**. See §4.1 #8. |

### 4.3 ✅ DIES-legitimately — old-core mechanics, successor named

Summarised; the successor is named in each row so the deletion wave can
verify rather than trust.

| Area | # | Successor |
| --- | --- | --- |
| `tests/common/{mock_backend,runtime}.rs` — `impl Backend` + `TestRuntime` | 0 | `crates/dev/host-mock` (`HostMock`, all 30 caps, no `Backend`) |
| `walker/{primitives,lifecycle,refs,control_flow}.rs` | 25 | `vocabulary/tests/vocab.rs` handler tests + `world/src/tests.rs` scope tests |
| `walker/batched_repeat.rs` | 8 | `vocab.rs::batched_repeat_is_one_execute_batch` + `payload_sizes.rs` (granularity thins: per-row op order, attach-parent identity, re-batch on rebuild) |
| `walker/ui_iteration_and_branches.rs`, `ui_for_flattening.rs` | 44 | `world/src/tests.rs` keyed reconcile + `newcore-app/tests/e2e.rs` + scene-parity Spliced goldens |
| `walker/{method_handles,pressable_disabled,lazy,rebuild}.rs` | 21 | `newcore-app/tests/e2e.rs` robot-method test, `vocab.rs::pressable_disabled_*`, `vocabulary/tests/lazy.rs` |
| `walker/snapshot_warning.rs` | 6 | **Architecturally retired** — component bodies run untracked on the new core, so the hoisted-snapshot trap cannot occur. Worth a release note: the *diagnostic* has no new-core detector. |
| `tests/{text_reactive_lowering,dynamic_reactive,reactive_props,switch_splice,native_reactive_parity,component_link,portal_screen_visibility}.rs` | 19 | `vocab.rs::text_content_*`, world dyn/untracked tests, `Host::supports_splice` + Spliced goldens, `portal_presence.rs` |
| `tests/preminted_style.rs` | 6 | `vocab.rs::preminted_stamps_classes_and_layers_overrides` |
| `tests/external.rs` | 13 | `runtime_scene::Registry` + `caps_conformance.rs` + `form/tests/newcore.rs` (2 flagged in §4.1 #17) |
| `tests/prim_gating.rs` | 7 | **No successor by design** — `runtime-vocabulary` has no `prim-*` features, so the bundle-size gating model does not exist on the new core. This is a **product-surface change**, not just a test loss; `idea-ui/tests/prim_gating.rs` (4 compile pins) is the remnant. |
| `src/{backend,builder,external,split_walker_tests,walker,walker/lazy,walker/robot}.rs` inline | 32 | caps defaults + `caps_conformance.rs`, `vocabulary/tests/navigator.rs`, `vocabulary/tests/lazy.rs`, `vocab.rs` robot module. Note `pressable_handler_is_born_batched` was **deliberately dropped** (`cycle()` wrapping is gone). |
| `src/primitives/{lazy,icon}.rs` inline | 6 | `vocabulary/tests/lazy.rs`, `vocab.rs::icon_size_mints_shared_square_sheet` |
| `vocabulary/tests/bridge.rs` | 9 | `vocabulary/tests/caps_conformance.rs` (10) — same 30 caps + 7 `Host` ops in the same order, on `HostMock`, no bridge. Both files already document this hand-off. |
| `mock-backend/tests/{navigator_control_keepalive,nested_dual_layout,nested_teardown_repro,swap_navigator_local,theme_chrome_cohort,wire_virtualizer}.rs` | 21 | `vocabulary/tests/walker_ports.rs` (19 ports, verified 1:1) + `swap/tests/newcore.rs` + `vocabulary/tests/navigator.rs`. One deliberate behavior inversion: `regression_lazy_disposing_cleanup_navigation_*` — the new dispatch-ordered queue makes the redirect win, not the outer selection. **Release-note item.** |
| `scene-parity/{src/scenarios.rs, src/scenarios_full.rs, tests/goldens.rs (20), tests/goldens_full.rs (30)}` + `src/full.rs`'s `impl Backend`/`FullCx` | 50 | The new-core halves (`goldens_new_core.rs` 21 + `goldens_full_new.rs` 31) already run against the SAME committed goldens with no `runtime_core` in the path. Scenario registries are identical on both sides (enforced by test). What genuinely goes: golden coverage of the swap/stack SDKs' **old-core `register_generic`** path, `Ref<StackHandle>` `.bind()`, and the `LegacyBridge` faithfulness proof (moot post-deletion). |
| `sdk/client/*/tests/oldcore.rs` + gated inline mods | ~28 | Each crate's `tests/newcore.rs` twin (svg 4, webview 5, video 4, maps 3, form 5, markdown 13, toolbar 5→10, codeblock 2, table 8, canvas-core, screen-recorder). Exceptions flagged in §4.1 #12/#17. |
| `dev/server/tests/sidecar_mount_regression.rs` | 2 | The test *is* the `mount`-vs-`render` distinction; the new core has one boot entry. Analogue: `gpu-backend/engine/tests/newcore.rs::start_in_world_stop_disposes_*`. |

### 4.4 Old-core invocations that disappear

**Explicit `old-core` feature, new-core default** (leg =
`--no-default-features --features old-core`): `dev/newcore-app`,
`ui/{idea-ui,idea-ui-nav,idea-theme}`, `sdk/client/*` (17 crates),
`examples/{welcome,nav-showcase}`, `robot-e2e/examples/conformance`,
`websites/{idea-ui-docs,website}`.

**`new-core` feature but no `old-core`** — the "old leg" IS the bare
default build, so these features become vacuous and their contents
should be made unconditional: all 11 `backend/*`, `gpu-backend/engine`,
8 `gpu-backend/host/*`, 3 `gpu-backend/variant/*`, `dev/{client,server,robot-e2e}`,
`runtime/macros`. **Done for the 8 hosts + 3 variants**: the feature is
deleted, the `newcore` module is flattened into the crate's normal
layout, and every historical path (`host_*::newcore::run`,
`variant_*::newcore::run_at`, `*::mount_newcore`) survives as a
compat re-export next to the canonical name. Concretely: **`cargo test -p backend-terminal` with no
features today compiles ZERO of `newcore_parity.rs`'s tests** — the
frozen-corpus suites only run under `--features new-core`.

**⚠️ Crates where old-core is the DEFAULT (these break at resolution,
not invocation):**

- `benchmark/idealyst-native/wasm` — `default = ["old-core"]`; every bare
  `cargo build` / `wasm-pack build` compiles the old core today.
- `websites/idea-ui-docs-gpu` — hard-pins `idea-ui-docs`'s `old-core`.
- 🔴 **`websites/tutorial` — old-core-only BY DESIGN, no `new-core`
  feature at all.** Its own Cargo.toml says the LESSON CONTENT teaches
  old-core semantics (`batch(..)`, "the write is visible immediately")
  that are *wrong* on runtime v2. Deleting `runtime-core` breaks the
  tutorial site's build outright — **the content must be rewritten in the
  same wave as the code.**

`[[test]]`/`[[example]]` targets with `required-features` that disappear:
`websites/website`'s `serve` + `ssr` examples (`["ssr","old-core"]`).

### 4.5 🔴 Resolution-level blockers (fail before any assertion runs)

Two findings that will break the build rather than a test, and that gate
everything else:

1. **`crates/dev/host-mock` — the designated `Backend`-free replacement —
   itself depends on `legacy-bridge` and `runtime-core`**
   (`runtime-vocabulary = { features = ["legacy-bridge"] }` for its
   `NavigatorOps::create_navigator`, plus `runtime-core` for
   `async-driver`). Fix this FIRST; §4.1 #8 and most of §4.3 depend on it.
2. **The `legacy-bridge` cliff — 14 crates forward it from their own
   `new-core` feature** and all stop resolving at the same instant: the
   11 `backend/*`, `gpu-backend/engine`, `dev/client`, `dev/server`. So
   the ~49 surviving backend parity tests (including every frozen-corpus
   suite in §1) fail at the **Cargo resolution** step, not the assertion
   step. Every backend's `newcore.rs` must drop its `create_navigator`
   delegation in the same commit (§2.3 — safe, since the new core has no
   call site). `scene-parity` and `newcore-app` already dropped it this
   wave and are the worked examples.


---

## 5. Deletion-wave checklist (derived from the above)

1. `grep -rn "DIES-WITH-OLD-CORE" crates/ websites/` and remove exactly
   those call sites. Every frozen-golden `check_*` call must survive.
2. For each backend in §2.2, write explicit `Host` bodies for
   `insert_at` / `remove_child` / `supports_splice` / `create_anchor`
   reproducing the Backend defaults verbatim. Pin `supports_splice`'s
   value with a literal assertion (replace the dying
   `Backend::supports_child_splice` half of the existing
   `newcore_host_splice_*` tests).
3. Delete all 12 `NavigatorOps::create_navigator` impls (§2.3) — the
   method ceases to exist; do not re-default it.
4. Tick off each backend's default-resolved list in §2.7 by hand,
   confirming the caps default is the intended behavior. §2.1 says the
   bodies match today; step 4 is about *intent*, not equality.
5. Re-run every frozen-corpus suite. They must be green with **no
   artifact edits**. `git diff --stat` over the `tests/goldens/` trees
   must be empty.
6. Re-run the count table in §3 and confirm the surviving numbers.
7. Mechanical `runtime_core::` → `runtime_shared::` rename in the
   surviving test suites that still spell shared types via the old
   re-export path (scene-parity fixtures, the host-mock-rewritten
   vocabulary suites, the backend parity tests' `StyleRules` imports).
   These are the same types by identity — a path rename, not a
   behavior change.

---

## 6. Stage 2 (consumer removal) — what landed, and what stage 3 inherits

The deletion runs in stages. **Stage 2 removed every old-core *consumer*
path** so that when the `Backend` impls go, nothing is still asking for
one. It did not delete `runtime-core` itself — that is stage 3.

### What no longer exists anywhere

- **Core-selector cargo features.** No crate has `new-core` or
  `old-core` as a core selector. Dual-core library crates lost both and
  their content is unconditional; the `new-core` features that only
  forwarded core deps are gone with their contents promoted.
- **The `oldcore.rs` / `newcore.rs` mutually-exclusive split.** Every SDK
  is one implementation in its crate's normal module layout, with
  **byte-identical public paths** — author code and docs did not churn.
- **CLI core resolution.** `crates/tools/cli/src/core_mode.rs` no longer
  resolves anything: `validate_flags` accepts `--new-core` as a no-op and
  rejects `--old-core` with a hard error naming
  `migrating-to-runtime-v2.md`. No `new_core` flag is threaded into any
  `BuildOptions` / `RunOptions`, and no generated wrapper pins a core
  feature or `default-features = false` on the user crate.
- **`prim-*` gating**, in every form: the twelve `runtime-core` families
  (mirrored on every backend crate), the six idea-ui features, the SDK
  forwards, the wrapper-side plumbing, and the `--primitives` selection
  itself. The rationale and the named successor (per-primitive
  registration in `handlers::register_builtins`) are in the migration
  guide, along with the manifest before/after an author needs.
  `idealyst build --primitives` is still **parsed** —
  `crates/tools/cli/src/removed_flags.rs` rejects it with a hard error
  naming `migrating-to-runtime-v2.md`, the same treatment `--old-core`
  gets, so a size-tuned pipeline learns the lever is gone instead of
  silently shipping the all-families bundle.
- **`runtime-core/hot-reload` from the dev chain.** `#[component]` has no
  hot-dispatch split, so a source change rebuilds and respawns the
  session; the `BuilderAdapter` seam is retained, `#[allow(dead_code)]`,
  as the re-enable point.

### Two generators that had been pinned to the old core

`roku` and `--serverless-lambda` were the named gaps at the defaults
flip. Both now build on the surviving core: roku's snapshot wrapper drives
`backend_roku::newcore::start` → `settle()` → `drain()`, with a generator
test pinning the **settle-before-drain** order (draining first bakes an
incomplete scene into `ui.json`); serverless-lambda needed no render entry
at all — its wrapper only takes the address of `<lib>::app` as an
inventory force-link anchor.

### The facade end state (and stage 3's remaining boundary)

`runtime_core::…` is preserved as the author-facing import path. It
resolves to `runtime-facade` through an unconditional crate-root
`extern crate runtime_facade as runtime_core;`, which shadows the extern
prelude for the whole crate. That is why the migration needed no
tree-wide rename.

**Stage 3 finishes it in three mechanical steps** (also recorded in
`crates/runtime/facade/src/lib.rs`):

1. `crates/runtime/core/src/lib.rs` becomes the facade root, and
   `crates/runtime/core/Cargo.toml` takes the facade's features
   (`robot`, `catalog`/`mcp`, `dev`, `async-driver`) and dependencies
   (`runtime-vocabulary`, `runtime-macros`, `runtime-shared`).
2. Delete every `extern crate runtime_facade as runtime_core;` line —
   `runtime_core` then resolves to the real crate again.
3. Rewrite every `runtime-facade = …` dep line as `runtime-core = …`
   with the same features, drop the `runtime-facade` package, and update
   the CLI's generated wrappers (they emit a `runtime-facade` dep line
   and pass `--features runtime-facade/dev`).

No source path, doc reference, or test import changes in any of those
steps. Until they land, `runtime-facade` and the alias are the mechanism.

One coupling to carry forward: `runtime-vocabulary/robot` does **not**
forward `runtime-shared/robot` (the old `runtime-core/robot` did), so
`runtime-facade/dev` gives the robot registry and catalog anchor but not
the bridge transport. Native dev wrappers therefore declare
`dev = ["runtime-facade/dev", "runtime-shared/robot"]` and the CLI passes
the wrapper-local `dev`. Making `runtime-vocabulary/robot` forward the
shared half would let stage 3 collapse that to one feature.

### §4.1 rows closed during stage 2

- **#2 (hydration)** — `navigators/stack/tests/hydration_web.rs` ported onto
  `backend_web::newcore::hydrate_in`. Caveat: it is a `wasm_bindgen_test`, so
  `cargo check --target wasm32-unknown-unknown` compiles it but no in-repo gate
  runs it (`wasm-pack test --headless` is the only runner).
- **#12 (navigator SDK legs)** — closed. `stack/tests/stack_local.rs` (5, incl.
  `regression_rebuild_cold_deep_link_never_mounts_parent_until_pop`) ported onto
  `crates/dev/host-mock`; `stack/tests/ssr.rs` (3) and `swap/tests/ssr.rs` (3)
  ported onto `backend_ssr::newcore::{render_path_with, render_all}`. The two
  navigator MCP recipes were NOT lost — they are core-independent static data in
  `runtime_shared::recipes` (`crates/runtime/shared/recipes/*.rs`), compile-and-realize
  gated by `newcore-app/tests/recipes_compile.rs`.
  One behavior note found while porting: under `Retain`, a cold deep link
  materializes the synthesized parent at seat time, so the first pop is a
  structural swap (`clear_children` + `insert`), not a fresh `create`.
- **#16 (idea-ui-mail)** — closed better than "mitigated". `idea-ui-mail` was
  old-core-only with no `new-core` feature, which is why the email golden had to
  re-author its templates as a hand-maintained replica. The crate is now on the
  surviving core and its 2 end-to-end tests render the real components through
  `backend_email::newcore::render_email`, so a test that COMPILES the real
  templates exists again alongside the frozen golden.
- **#4 / #14 (per-primitive reactive plumbing, event-plumbing installs)** — new
  suites landed at `crates/runtime/vocabulary/tests/{reactive_prop_plumbing,event_plumbing}.rs`.
- **#13 (props arrival)** — new suite at `crates/dev/newcore-app/tests/props_arrival.rs`.
- **`tests/prim_gating.rs` (§4.3)** — discharged: the gating model is gone by
  decision, not by accident (see §6).

### Capability genuinely lost in stage 2 (one item, named)

**The stack navigator's native push surface.** `navigators/stack/src/{ios.rs,android.rs}`
(UINavigationController / Kotlin `RustNavigator`) were `NavigatorHandler<B>`
implementations against the old per-backend navigator registry. The surviving core
has no navigator-handler seam for an SDK at all — navigation is
`runtime_vocabulary::handlers::navigator`'s built-in. Restoring it needs a per-host
"native push surface" hook in that handler plus a backend-side presenter: vocabulary
+ backend work, not SDK work. Recorded in `stack-navigator`'s Cargo.toml and module
docs.

### Two runtime-server NATIVE CLIENT shells still register per-backend

`crates/backend/ios/rs-shell/src/lib.rs` and the generated Android wrapper's
`register_first_party_sdks` (`crates/tools/build/android/src/lib.rs`) both call
`<sdk>::register(&mut ConcreteBackend)`. SDK registration is now per-`Registry<H>`,
and the replay client itself (`dev_client::WireBackend`) is mid-bound-flip from
`B: Backend` onto the caps surface. These two shells move with that flip — they are
the same seam, not two.

---

## 7. Stage 3 consumer sweep — findings and closures (2026-07-29)

Successor to the stage-2 consumer sweep that exhausted its context with 11
sub-agents in flight. This section records what those orphans actually
landed, what was still half-done, and which of §6's named losses closed.

> ⚠️ **Read §7.4 before quoting any test number from this document.**
> Whole-workspace `cargo test` is **not a valid gate in this repo**:
> resolver-2 unifies the `server` crate's `server` feature ON across the
> selected set, which `cfg`s out the client-side surface that every
> client-only demo uses. Numbers below therefore come from a **split
> invocation** — 156 packages in one run, plus the 10
> mutually-exclusive-`server`-feature crates run individually. Anyone
> reproducing them needs the invocation shape, not just the totals.
>
> ⚠️ **§7.8 is a correction to §6.** Read them together — §6's account of
> the runtime-server shells is wrong.

### 7.1 What the orphaned fleet landed (verified by re-reading the tree)

Verified present and compiling, not merely reported:

- **All `examples/*`** on the unconditional `extern crate runtime_facade as
  runtime_core;` alias with `register_scene_extensions` / `scene_app` /
  `register_scene_extensions_recorder`. `grep -rn "runtime-core" examples/`
  is empty.
- **All `websites/*`** ported and green, including `ssg_parity` against all
  67 frozen files. Two latent breaks were caught there and are worth naming
  because they are the same class this sweep kept finding: `idea-ui-docs`
  exported `app_newcore` while the CLI wrapper calls `{lib}::app()`, and
  `website` exported `register_extensions_recorder` while
  `build-runtime-server`'s template calls `register_scene_extensions_recorder`.
  **Neither was a compile error in the crate itself** — the wrapper is
  generated at build time, so the mismatch only surfaces at
  `idealyst build`. Every generator/consumer name pair in this tree needs
  checking by hand; the type system does not.
- **Canvas is NOT a lost capability** (the stage-2 handoff said it was).
  `canvas-vello` has a real `CanvasPrim` scene handler on both the native
  (`vello/src/render.rs::register`) and web (`render_web.rs::register`)
  legs, and `canvas-native` has real iOS/macOS CoreGraphics and Android
  `android.graphics` painters plus a documented `mount_placeholder`
  degradation path. The whiteboard's drawing surface is not web-only.

### 7.2 Half-done work this stage finished

| Item | State found | Action |
| --- | --- | --- |
| `crates/sdk/server/email` | still on `runtime-core`; `backend_email::render_email` moved to `newcore::` | retargeted to the facade + `backend_email::newcore::{render_email, render_email_with}` |
| 8 SDK demo crates (`contact-form-lambda`, `server-fn-demo`, `dnd-demo`, `kanban-demo`, `sortable-demo`, `pdf-demo`, `jobs-demo`, `pubsub-demo`) | still on `runtime-core`, stale `register_extensions<B: runtime_core::Backend>` | facade + `runtime-vocabulary` direct dep + `register_scene_extensions<H>(&mut Registry<H>)` |
| `pdf-demo` specifically | registered `canvas_vello` through `RegisterExternal` | now `canvas_vello::register(registry)` over `Registry<H: GraphicsOps + StyleServices>` |
| `crates/dev/robot-e2e/examples/robot-e2e-demo` | fully on the old core, unowned | ported; raw Robot surface moved to `runtime_vocabulary::robot::{Robot, Query, Element, ElementKind, watch_signal}`, log macros to `runtime_shared::`, and the three `Signal::update(|n| *n += 1)` sites to `set(get() + 1)` (the world kernel's `update` composes a value, it does not take `&mut`) |
| `crates/tools/cli/src/cmd/docs.rs` | generated `pub use docs_app::{app, register_extensions};` — a name `docs-app` no longer exports | fixed to `{app, register_scene_extensions, scene_app}`, assertion updated |
| `build-macos` / `build-terminal` / `build-sim` | generated dep lines still requested the deleted `new-core` feature on `host-appkit` / `host-terminal` / `variant-*` | dropped; also switched the generated boot calls to the canonical crate-root entries (`host_appkit::run_with`, `host_terminal::run`, `variant_*::run_with`) rather than the `newcore::` compat re-exports |

### 7.3 §6's named losses — dispositions

- **Stack navigator native push surface — still open, now specified.** A
  full hook design (trait shape, install/clear seam mirroring
  `handlers::navigator::url_sync`, the five exact call sites in
  `StackShared<H>`, the app-pop-vs-user-pop split, and the cold-deep-link
  back-stack reconstruction subtlety) is written into
  `crates/sdk/client/navigators/stack/src/lib.rs` module docs. It cannot
  be landed from the SDK: every stack mutation lands in
  `runtime-vocabulary`, which is another owner's crate. **It should be
  restored, not accepted** — what is lost is the interactive swipe-back
  gesture and system-Back integration on iOS/Android, which is
  platform-idiomatic *input*, not a cosmetic divergence, so CLAUDE.md §7
  argues for restoring it rather than against.
- **The two runtime-server native client shells — CLOSED, and the stage-2
  diagnosis was wrong.** They are **not** the same seam as the dev-chain
  bound flip. That flip already landed (`dev_client::WireBackend<B:
  caps::AllCaps>`). What actually killed the shells' registration is that
  under runtime v2 an SDK handler runs on the **server** side of the wire,
  registered on `Registry<WireRecordingBackend>` via the sidecar's
  `register_scene_extensions_recorder` seam — so there is nothing left for
  the client to register. The `fn(&mut ConcreteBackend)` argument was the
  old `Element::External` client registry. Both shells
  (`crates/backend/ios/rs-shell`, and the Android wrapper generated by
  `crates/tools/build/android`) now pass an empty closure and have dropped
  their SDK dependencies. The Android one additionally called
  `drawer_navigator::register`, a crate that no longer exists — i.e. the
  generated Android runtime-server wrapper could not have compiled.
  **Behavior delta, hot-reload mode only**: an SDK that type-dispatches to
  a backend-CONCRETE handler (`codeblock`'s native iOS/macOS/Android
  mounts) renders its portable variant under `idealyst dev`, because the
  registry it sees is the recorder's. Local device builds are unaffected.
- **Bundle-size fixtures — the stage-2 argument was half right.** The four
  deleted `*-canvas` examples were manual scratch fixtures with no
  assertions, so deleting them lost no coverage. But the *real* gate was
  never in `examples/`: it is `tests/prune-regression`'s
  `measure_registration_split` (400 KiB threshold) over
  `tests/lazy-external-split/{eager,lazy,heavy}`, and those three crates do
  not compile. Worse, the axis they measured — deferring a heavy handler's
  registration out of `main.wasm` via `defer_external_registration` — was
  **a genuinely lost capability**: `Registry::register` took `&mut self`
  and `MountCx` held `&Rc<Registry<H>>`, so runtime v2 had no post-boot
  registration seam at all. A replacement gate pinning the always-resident
  built-in handler set now exists at
  `crates/runtime/vocabulary/tests/builtin_surface.rs`.

  **Since closed.** `runtime-scene` grew the seam: `Registry::defer::<T>()`
  declares a payload kind late-bound at the boot seam,
  `Registry::register_deferred::<T, _>(…)` (on `&Rc<Self>`) installs the
  handler at any later time and realizes every parked item of that kind in
  place, and `runtime_scene::defer_registration::<H, _>(…)` is the
  thread-local mailbox a `#[component(lazy)]` body uses when it has no
  registry in hand (realize drains it at the top of every realization).
  Parking is opt-in per kind precisely so the "you forgot to register your
  SDK" panic is not weakened. The fixtures, renamed
  `tests/lazy-payload-split/{eager,lazy,heavy}`, measure the registration
  axis again at the same 400 KiB threshold.


### 7.4 Gate note: whole-workspace `cargo test` is not a valid gate for the demos

`cargo test -p <all 163 in-scope packages>` fails to compile `pubsub-demo`
with `cannot find function use_socket in crate ::server` and friends.
**This is a resolver-2 feature-unification artifact, not a breakage.**
`pubsub-demo` is a client-only build (its own `server` feature off), but
selecting it in the same invocation as `pubsub` / `jobs` / the server
demos unifies `server/server` ON across the graph, which `cfg`s out
exactly the client-side surface the demo uses. The same shape predates
this wave — the mutually-exclusive `server` feature is unchanged from
`HEAD`.

`cargo test -p pubsub-demo` (and each of `server-fn-demo`,
`contact-form-lambda`, `jobs-demo`) is clean in isolation. Gate the
client-only demos separately; do not read a whole-workspace run as
authoritative for them.

### 7.5 Still open, with owners

| Item | Owner | Note |
| --- | --- | --- |
| Stack navigator native push surface | `runtime-vocabulary` + backends | Fully specified in `stack-navigator/src/lib.rs`; SDK side needs no change |
| Reactive-profiler emission | `runtime-world` | Only `runtime_shared::reactive` (the legacy arena) calls `record_txn_enter`/`record_effect_run`/`record_commit`. The Inspector app side is complete and now names the gap in its empty state instead of rendering it as "no activity" |
| ~~Post-boot handler registration (bundle size)~~ | `runtime-scene` | **CLOSED.** `Registry::defer::<T>()` (boot declaration) + `Registry::register_deferred::<T, _>(…)` (post-boot install + in-place drain of parked items) + `runtime_scene::defer_registration::<H, _>(…)` (thread-local mailbox for a chunk body with no registry in hand). An UNDECLARED unknown payload still panics at realize — parking is opt-in per kind. Semantics pinned in `crates/runtime/scene/src/tests.rs`; bytes measured by the restored `tests/lazy-payload-split` gate |
| `webview` native (iOS `WKWebView` / Android `android.webkit`) | SDK | Regressed to a placeholder; needs a `Registry<IosBackend>`/`Registry<AndroidBackend>` handler + `schedule_flush` wrap |
| `markdown` native single-node optimization | SDK | Rendering preserved, perf property lost; `segments.rs` is in git history |
| Linux GTK4 toolbar leg | SDK | Compile-unverified; recipe is `PKG_CONFIG_ALLOW_CROSS=1` against homebrew gtk4 `.pc` files |
| `canvas-native`'s vacuous `self-register` feature; inert `default-features = false` pins on ~11 SDK specs | root manifest | Stage-3 cleanup |

### 7.6 Gate results (2026-07-29, stage-3 consumer sweep)

| Gate | Result |
| --- | --- |
| `cargo check` across the 165 in-scope packages, `--all-targets` | clean |
| `cargo test`, 156 in-scope packages in one invocation | green (see §7.4 for the 10 excluded) |
| The 10 mutually-exclusive-`server`-feature crates, run individually | all clean; `jobs` 8, `todo-sync-demo` 1, the demos have no tests |
| `cargo test -p server --features server` | **52 passed** |
| `cargo test -p idealyst-cli` | **85 passed** (84 + 1 new force-link regression) |
| build generators (`build-{android,ios,web,runtime-server,roku,ssr,serverless-lambda}`, `run-android`) | **78 passed** |
| `cargo test -p mcp-catalog -p mcp-server -p idealyst-cli` | **176 passed, 2 ignored** |
| `cargo test -p idea-ui` | **113 passed** (111 + 1 + 1), 39 pre-existing ignored |
| `cargo test -p build-web` | **26 passed** |
| `cargo check --target wasm32-unknown-unknown` (welcome, website, idea-ui, conformance) | clean |
| **Android emulator smoke** (`newcore-android-smoke`, Pixel_6_Pro_API_34) | **PASS** — `[SMOKE-SELFTEST] committed=true views=13 verdict=PASS`, screenshot `newcore-android-smoke.png` |
| Frozen golden artifacts modified | **zero** (`git diff --stat` over every `goldens*/` tree is empty; the scene-parity corpora are untouched, and the new per-backend corpora are untracked additions) |

**One pre-existing failure, not caused by this wave**: `cargo test -p server`
*without* `--features server` fails to compile `tests/end_to_end.rs` with
`RcBlock<…> cannot be sent between threads safely`. The test
`tokio::spawn`s a client-side `net` future, and on a macOS host `net`'s
client path is objc2-backed and therefore not `Send`. `crates/sdk/client/net`
is byte-unchanged this wave (`git diff --stat` empty) and the only edit to
`end_to_end.rs` is an added `extern crate` alias, which cannot change a
future's auto-traits. The crate's real gate is `--features server` (52
passed).

### 7.7 A defect class this migration nearly shipped: generated-code name drift

**Mechanism.** The CLI generates each platform wrapper crate at build
time — `build-web` emits a `lib.rs` calling `{lib}::app()` and
`{lib}::register_scene_extensions`, `build-runtime-server` emits a
`main.rs` calling `{lib}::register_scene_extensions_recorder`, and so on.
Those call sites are **strings in a generator**, not Rust paths the
compiler resolves. So when an app crate renames or drops the symbol the
wrapper expects, *nothing fails to compile*: the app crate is green, the
generator crate is green, and the mismatch only surfaces at
`idealyst build` / `idealyst dev`, on the platform in question.

It is worse than "untested", because each of these sites **did** have a
test — asserting the wrong string. A generator test of the shape
`assert!(lib_rs.contains("register_extensions"))` passes forever while
the symbol it names no longer exists anywhere.

Four instances, all found by hand-reading generator/consumer pairs
during this wave:

| # | Generator emits | Consumer actually exports | Surfaced at |
| --- | --- | --- | --- |
| 1 | `crates/tools/cli/src/cmd/docs.rs`: `pub use docs_app::{app, register_extensions};` | `docs-app` exports `register_scene_extensions` (+ `scene_app`) | `idealyst docs` |
| 2 | `build-web` wrapper: `{lib}::app()` | `idea-ui-docs` exported `app_newcore` | `idealyst build --web` on the docs site |
| 3 | `build-runtime-server` wrapper: `{lib}::register_scene_extensions_recorder` | `website` exported `register_extensions_recorder` | `idealyst dev` on the website |
| 4 | `build-{macos,terminal,sim}` dep lines: `features = ["new-core"]` on `host-appkit` / `host-terminal` / `variant-*` | those crates deleted the feature | cargo resolution of the generated wrapper |

(#1 and #4 were found and fixed directly here, with their assertions
corrected; #2 and #3 were found by the websites sweep.)

Two more instances landed after that table was written, bringing the
count to **six**:

| # | Generator emits | Reality | Surfaced at |
| --- | --- | --- | --- |
| 5 | `build-web`'s premint wrapper: `runtime-core = { features = ["style-dump"] }`, and a `main.rs` calling `{lib}::app()` at top level | the dep must be `runtime-facade`; and building outside a `World` aborts the moment the app body creates a signal | `idealyst build --web --premint` |
| 6 | `build-runtime-server`: `dev-server` dep with `features = [… "new-core"]` | `dev-server` has no `new-core` feature | cargo resolution of the sidecar wrapper |

Both had a generator test pinning the broken string — #5's asserted
`features = ["style-dump"]` on a `runtime-core` line, #6's asserted that
`new-core` *was present*. Both assertions are now inverted to pin what
actually works, and #5 gained a `!deps_section.contains("runtime-core = ")`
guard scoped to the dependency half (the `[patch]` block legitimately
still redirects `runtime-core` while that crate exists).

**A related assumption class, now false everywhere it appears.**
`runtime-macros` documented the premint path as having *no glue home*,
justified by: dump builds are CLI-driven OLD-core builds, so the
combination cannot occur. With one core it **necessarily** occurs. Any
comment of the form "X and Y cannot both happen because Y is old-core
only" is now wrong by construction. Grep for it when touching a
feature-gated path; the in-scope siblings found and corrected this wave
were prose in `svg/src/web_util.rs` and `video/src/web_util.rs`
("shared by both cores' ops impls").

**Standing rule for anyone touching this tree**: a rename or removal of
`app`, `scene_app`, `register_scene_extensions`,
`register_scene_extensions_recorder`, or any feature named in a
generated dep line is a **cross-crate** change. Grep
`crates/tools/build/*/src/lib.rs` and `crates/tools/cli/src/cmd/` for the
old string, fix the generator, and fix the generator's assertion — the
type system will not do it for you. The generator tests are the only
gate, so an assertion that is merely *green* proves nothing; it has to
name a symbol that exists.

### 7.8 Correction to §6: the runtime-server shells were mis-diagnosed

§6 records the two native runtime-server client shells as moving with the
dev-chain `WireBackend<B: Backend>` → caps bound flip — "they are the
same seam, not two". **That is wrong**, and the note should be read with
this correction.

The bound flip landed independently and is orthogonal
(`dev_client::WireBackend<B: caps::AllCaps>`). What actually invalidated
the shells' `fn(&mut ConcreteBackend)` registration argument is that
under runtime v2 an SDK's mount handler runs on the **server** side of
the wire — installed on `Registry<WireRecordingBackend>` via the
sidecar's `register_scene_extensions_recorder` seam
(`dev_server::sidecar::run_newcore`) — so by the time commands reach the
client they are already ordinary primitive commands. The argument was the
old `Element::External` **client** registry, which is deleted. The
registration is therefore **obsolete, not pending**: it was removed, not
ported.

Corroborating evidence that this path had no coverage at all: the
Android shell's `register_first_party_sdks` called
`drawer_navigator::register`, and `drawer-navigator` was deleted in the
legacy-navigation wave. The generated Android runtime-server wrapper
could not have compiled.

### 7.9 Removals need the same loud treatment as renames

`--old-core` was removed *well*: `core_mode.rs` rejects it with a hard
error naming `docs/migrating-to-runtime-v2.md`, so an author with it in a
script gets told what happened and where to read.

`--primitives` was removed *silently* — the flag simply vanished, so the
same author got an opaque clap "unexpected argument". Same wave, same
kind of change, opposite treatment. It now errors in the `--old-core`
shape, with a test alongside the existing rejection test.

The generalisation worth keeping: **a removal is as breaking as a
rename, and needs the same signposting.** The asymmetry is easy to
introduce because a rename forces you to touch the call sites (so you
think about the message) while a deletion just compiles.

A second instance of the same asymmetry, found alongside it: removing
the six `prim-*` cargo features left **26 rustdoc blocks still
documenting them**, telling authors to write
`default-features = false, features = ["prim-view", …]` — advice that now
produces an unknown-feature error. Deleting a feature does not delete its
documentation, and rustdoc has no compiler to catch the drift.

### 7.10 Completeness: every backend now has live evidence on runtime v2

With the Android emulator run in §7.6, the wave has a live, on-device or
in-browser verdict for **every** backend the framework ships:

| Platform | Evidence |
| --- | --- |
| web | Playwright in a real browser; backend-web browser battery 76/76 on the wrapper's own feature leg |
| SSR / SSG | byte-identity corpora (6/6) + the website's 33-route `ssg_parity` (67 frozen files) |
| macOS | `newcore-macos-smoke`, real window, `committed=true` incl. the tracking-run-loop proof |
| iOS | `newcore-ios-smoke` on iPhone 17 Pro Max sim, `committed=true views=27` + screenshot |
| Android | `newcore-android-smoke` on Pixel_6_Pro_API_34 — `[SMOKE-SELFTEST] committed=true views=13 verdict=PASS`, screenshot `newcore-android-smoke.png` |
| GPU / wgpu | real-Metal headless capture, `pixels_changed=true`, `newcore-gpu-smoke.png` |
| terminal / cpu / roku / email | frozen cell-exact, pixel-exact, wire-exact and byte-exact corpora |

Android was the last one whose evidence predated the current tree; it has
now been re-run against it.

---

## 8. Closing note: the deletion completed (2026-07-30)

Everything above is the **pre-deletion record** and is left exactly as it
was written. This section is the only retrospective addition: it states
that the wave finished and pins the final numbers, so a reader knows the
baseline was discharged rather than abandoned.

### What went

The old core is gone from the tree. `crates/runtime/core/src/` lost 119
files (~58,000 lines by `git diff --diff-filter=D --numstat` against the
branch point) — the `Element` enum, the 159-method `Backend` mega-trait,
the render walker, `Bound`/builders, the `External` table, `batch`, the
legacy reactive arena, and `bridge::LegacyBridge` with
`caps::NavigatorOps::create_navigator`. Counting the whole branch,
including the legacy navigator SDKs and the old-core-only harnesses
retired alongside, 161 files were deleted totalling ~41,100 lines.

Much of that 58,000 was **relocated, not destroyed** — `runtime-shared`,
`runtime-scene` and `runtime-vocabulary` own it now. The net loss figure
is the smaller one; treat the per-path counts above as what left
`runtime-core`, not as code that ceased to exist.

### The package slot

`runtime-facade` was folded into the `runtime-core` package slot. There
is no `runtime-facade` package anymore. `runtime_core::…` still resolves
for every app, SDK, component library and example — it is now a 77-line
root re-exporting `runtime_vocabulary::glue` plus the macro set. See
[`crates/runtime/core/README.md`](../crates/runtime/core/README.md) for
the crate-ownership map.

Backends do **not** depend on `runtime-core`; they consume
`runtime-shared`, `runtime-scene` and `runtime-vocabulary` directly.

### The one capability §6 recorded as lost was restored

§6 named a single genuine capability loss: deferred handler registration
(the old `defer_external_registration`). It is back, with a different
and narrower shape:

| Step | API |
| --- | --- |
| Declare the payload kind at boot | `Registry::defer::<T>()` |
| Install the real handler post-boot | `Registry::register_deferred::<T, _>(handler)` |
| Queue from inside a chunk (no registry in hand) | `runtime_scene::defer_registration` |

`realize` drains the mailbox at the top of every realization, so a
payload that already rendered elsewhere waits behind a
layout-transparent placeholder and completes in place — same position,
no remount. Declaring the kind is what licenses that wait; an
undeclared payload still fails loudly at realize.

**Measured, not asserted.** `tests/lazy-payload-split` builds the same
heavy SDK two ways and diffs `main.wasm`:

| Variant | `main.wasm` | Lazy chunk |
| --- | --- | --- |
| eager (`register` at boot) | 1278 KiB | 4 KiB |
| lazy (`register_deferred` from the chunk) | 766 KiB | 518 KiB |

512 KiB moved out of the main bundle. The gate requires ≥ 400 KiB.

### Wire protocol: deliberately NOT bumped

`wire::PROTOCOL_VERSION` stays at **17**. The deletion did not change
the serialized surface — every `Command` / `AppToDev` variant and every
field kept its name, type and `serde` attribute (verified by diffing the
declaration surface against the branch point: identical). What changed is
Rust-side only: the recorder records a realize pass instead of a walker,
and the external-payload serde registry moved from `runtime_core` into
`wire::payload_serde` — same bytes, different home. A bump signals
incompatibility to a peer binary; there is none here, and bumping would
have forced a needless dev/app lockstep upgrade. The rationale is
recorded on the constant itself.

### Gate status at close

| Gate | Result |
| --- | --- |
| `website` `ssg_parity` | 67/67 frozen files, 33 routes + served doc |
| `wire` | 21/21 across roundtrip, transport and virtual-layout suites |
| `mock-backend` (frozen wire snapshot) | green |
| Frozen corpora (cpu / terminal / roku / email / ssr / scene-parity / parity-goldens) | byte-identical, untouched by the close-out |

Eight `ssg_parity` pages were **deliberately re-baselined** during the
documentation close-out because their own copy changed — they taught
deleted API (`Bound<H>`, the `Backend` mega-trait, `Element::External`,
the render walker, `register_lazy()`) as if it were current. The moves
were text-only: no `head.css` moved and no class hash was added or
dropped on any of the eight. The list and the reasoning live in
[`websites/website/tests/goldens/ssg/README.md`](../websites/website/tests/goldens/ssg/README.md).
