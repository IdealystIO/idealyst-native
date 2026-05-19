# backend-roku

Generator backend for Roku.

## Theme switching: temporarily unimplemented

The theme/tokenization refactor split `framework-core`'s theme APIs into:

- **framework-core**: token primitives (`install_tokens` / `update_tokens` /
  `Tokenized<T>` / `tokens_version_signal`).
- **framework-theme**: the optional theme-as-struct pattern + `install_themes`
  multi-variant helper.

The previous Roku integration depended on two backend-trait hooks that have
been removed:

- `register_theme_variant(name, tokens)` — captured each named variant's
  token map so the device could switch themes at runtime.
- `bind_active_theme_signal(signal_id, initial_name)` — wired a `Signal<String>`
  into the device-side switching machinery.

Both hooks are gone in Phase 1. `Backend::install_tokens` /
`Backend::update_tokens` currently `unimplemented!()` on this backend — the
device-side switching machinery needs to be rewired through `framework-theme`
in a follow-up. Until then, Roku builds that exercise the theme path will
panic at runtime.
