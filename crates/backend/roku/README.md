# backend-roku

Generator backend for Roku.

## Theme switching: no-op by design

The theme/tokenization refactor split the theme APIs into:

- **`runtime-shared` (re-exported as `runtime_core::…`)**: token primitives
  (`install_tokens` / `update_tokens` / `Tokenized<T>` /
  `tokens_version_signal`).
- **[`idea-theme`](../../ui/idea-theme)**: the optional theme-as-struct
  pattern + `install_themes` multi-variant helper.

An earlier Roku integration depended on two backend hooks that have since
been removed:

- `register_theme_variant(name, tokens)` — captured each named variant's
  token map so the device could switch themes at runtime.
- `bind_active_theme_signal(signal_id, initial_name)` — wired a `Signal<String>`
  into the device-side switching machinery.

Both hooks are gone. `StyleOps::install_tokens` / `update_tokens` are now
deliberate **no-ops** on this backend (same posture as iOS / Android): the
Roku wire protocol has no runtime variable layer, so `style::lower_style`
resolves every `Tokenized<T>` to a literal at `apply_style` time. When the
app calls `update_tokens(...)`, the tokens-version signal re-fires every
styled effect, each re-applies, and the wire stream picks up the new values
automatically — nothing needs to be emitted here. See the rationale comment
on `install_tokens` in `src/newcore.rs`, pinned by
`regression_roku_install_and_update_tokens_no_panic`.
