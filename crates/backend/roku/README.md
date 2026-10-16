# backend-roku

Generator backend for Roku.

## Theme switching: tokens resolve to literals

Token primitives (`install_tokens` / `update_tokens` / `Tokenized<T>` /
`tokens_version_signal`) live in `runtime-shared` (re-exported as
`runtime_core::…`); the optional theme-as-struct pattern and the
`install_themes` multi-variant helper live in
[`idea-theme`](../../ui/idea-theme).

`StyleOps::install_tokens` / `update_tokens` are
deliberate **no-ops** on this backend (same posture as iOS / Android): the
Roku wire protocol has no runtime variable layer, so `style::lower_style`
resolves every `Tokenized<T>` to a literal at `apply_style` time. When the
app calls `update_tokens(...)`, the tokens-version signal re-fires every
styled effect, each re-applies, and the wire stream picks up the new values
automatically — nothing needs to be emitted here. See the rationale comment
on `install_tokens` in `src/newcore.rs`, pinned by
`regression_roku_install_and_update_tokens_no_panic` in `src/lib.rs`.
