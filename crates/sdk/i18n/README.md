# i18n — internationalization

Lightweight, Rust-native, strongly-typed internationalization. You declare
your translations inline in Rust; the `i18n!` macro generates a typed
`Locale` enum and one function per message. A missing translation or a
typo'd placeholder is a **compile error**, not a runtime surprise. Switching
language re-renders affected text **in place** — it's built on the same
reactive machinery as `text()` and `viewport_size()`, so there's no manual
refresh and no per-platform code.

- **Strongly typed** — keys and interpolation arguments are checked at
  compile time.
- **Reactive** — change the locale, every visible string updates itself.
- **Bundled or on-demand** — ship languages in the binary, or load them over
  the network only when a user picks them.
- **Interpolation only, with an escape hatch** — `{name}` substitution out of
  the box; plug in a Fluent/ICU formatter when you need plurals or gender.

---

## 1. Add the dependency

```toml
[dependencies]
i18n = { workspace = true }

# Add the `lazy-fetch` feature only if you use opt-in (network-loaded)
# locales — it pulls in the `net` SDK + a ready-made web loader.
# i18n = { workspace = true, features = ["lazy-fetch"] }
```

## 2. Declare your messages

Put the catalog in its own module (`t` is the conventional name) so call
sites read as `t::greeting(...)`:

```rust
pub mod t {
    i18n::i18n! {
        // Exactly one locale must be `(default)` — it's the reference
        // locale and the fallback when a translation is unavailable.
        locales: { En = "en" (default), Fr = "fr", Es = "es" }

        // A message with a typed interpolation argument.
        greeting(name) {
            En: "Hello, {name}",
            Fr: "Bonjour, {name}",
            Es: "Hola, {name}",
        }

        // A message with no arguments.
        tagline {
            En: "Type-safe i18n.",
            Fr: "Internationalisation typée.",
            Es: "i18n con tipado seguro.",
        }

        items(count) {
            En: "{count} items in your cart",
            Fr: "{count} articles dans votre panier",
            Es: "{count} artículos en tu carrito",
        }
    }
}
```

## 3. Use messages in components

Each message is a function returning `Reactive<String>`. Pass it straight to
any reactive-text prop (`text(...)`, idea-ui's `Typography`, `Button`, …):

```rust
ui! {
    Stack() {
        Typography(content = t::greeting("Ada"))   // "Hello, Ada"
        Typography(content = t::tagline())
        Typography(content = t::items(3))          // "3 items in your cart"
    }
}
```

## 4. Switch language

The macro generates a typed `set_locale` / `current_locale` / a `Locale`
enum. Wire `set_locale` to a control and every visible message updates:

```rust
let pick_fr: Rc<dyn Fn()> = Rc::new(|| t::set_locale(t::Locale::Fr));

ui! {
    Button(label = "Français", on_click = pick_fr)
}
```

That's the whole loop for bundled languages — no loaders, no setup, works
offline on every backend.

---

## Adding a new language

1. Add the locale to the `locales: { … }` header: `De = "de"`.
2. Add a `De: "…"` line to **every** message. If you miss one, the compiler
   tells you exactly which message and which locale:

   ```
   error: message `greeting` is missing a translation for bundled locale `De`
   ```
3. (Optional) add a button calling `t::set_locale(t::Locale::De)`.

Adding the locale variant but **not** the translations is the safety net —
you can't ship a half-translated bundled language by accident.

---

## What the compiler checks

The strong-typing guarantees, each a `compile_error!` spanned at the
offending line:

| Mistake | Error |
| --- | --- |
| Bundled locale missing a message | `message '…' is missing a translation for bundled locale '…'` |
| Placeholder with no matching arg (`{nmae}`) | `placeholder '{nmae}' has no matching argument` |
| Declared arg never used in the default translation | `argument '…' is never used in the default translation` |
| Inline translation for a `lazy` locale | `'…' is a lazy locale; its strings come from a fetched pack` |
| Zero or multiple `(default)` locales | `exactly one locale must be marked '(default)'` |

## Interpolation

Arguments are typed (anything `Display`) and substituted by name. `{{` and
`}}` are literal braces:

```rust
items(count) { En: "{count} of {count} — {{literal braces}}" }
// t::items(2)  ->  "2 of 2 — {literal braces}"
```

> **Note — arguments are snapshots.** `t::items(count)` captures the value at
> call time. The returned text re-renders when the **locale** changes, not
> when `count` changes. For a value that changes, re-call the message inside
> a reactive scope, e.g. `Typography(content = rx!(t::items(n.get()).get()))`.

## Plurals / gender / ICU (custom formatter)

Built-in formatting is interpolation only. To add plural/gender/number
rules, install a formatter once at startup — it receives the template and
the (already-stringified) args and returns the final string:

```rust
i18n::install_formatter(|template, args| {
    // e.g. parse ICU `{count, plural, …}` here, or delegate to `fluent`.
    my_icu_format(template, args)
});
```

This is the swap-in seam for a richer layer built on top of `i18n`.

---

## Opt-in (network-loaded) languages

Bundled locales are compiled into the binary. A locale marked `(lazy)` is
**opt-in**: its strings are *not* in the binary at all — they live in a JSON
pack loaded on demand. Use this to keep rarely-used languages off the
critical path / out of the wasm bundle.

### 1. Mark the locale `lazy` and ship a pack

```rust
locales: { En = "en" (default), Fr = "fr", Ja = "ja" (lazy) }
```

A `lazy` locale must **not** have inline `Ja: "…"` lines (that's a compile
error — its strings are external). Instead, write a flat JSON pack keyed by
message name, e.g. `locales/ja.json`:

```json
{
  "greeting": "こんにちは、{name}",
  "tagline": "型安全な国際化。",
  "items": "カートに{count}個の商品"
}
```

Until the pack loads, opt-in messages fall back to the default locale; when
it arrives, the same reactive text upgrades in place.

### 2. Install a pack loader

Tell `i18n` how to fetch a pack. `set_locale(Locale::Ja)` then loads it
automatically (once; subsequent switches are instant).

**On the web** — enable the `lazy-fetch` feature and use the built-in loader.
Serve the packs at the given base (a relative base is resolved against the
page origin):

```rust
i18n::set_pack_loader(i18n::net_pack_loader("/locales"));
// expects GET /locales/ja.json
```

**On native (iOS/Android/desktop)** — there's no page origin, so either pass
an absolute base, or install packs from the app bundle / disk:

```rust
// absolute CDN base:
i18n::set_pack_loader(i18n::net_pack_loader("https://cdn.example.com/locales"));

// or load from disk / a bundled asset yourself:
i18n::set_pack_loader(|code| {
    if let Ok(json) = std::fs::read_to_string(format!("assets/locales/{code}.json")) {
        let _ = i18n::install_pack_json(code, &json);
    }
});
```

The loader is just `Fn(&str)` — it's handed a locale code and must
eventually call `i18n::install_pack` / `install_pack_json`. Fetch however you
like (the `net_pack_loader` is a convenience, not a requirement).

### 3. (If your default locale isn't `"en"`) seed it at startup

```rust
t::init(); // sets the active locale to your declared `(default)`
```

---

## SSR / SSG

Set the locale (and, for opt-in locales, install the pack from a build-time
file) **before** rendering — exactly like setting the viewport before SSR.
Loop locales × routes to emit per-locale static variants:

```rust
for &locale in t::Locale::ALL {
    i18n::set_locale_code(locale.code());
    if locale.is_lazy() {
        i18n::install_pack_json(locale.code(), &read_pack(locale.code()))?;
    }
    let page = backend_ssr::render_path("/", app);
    write(format!("dist/{}/index.html", locale.code()), render_document(&page, None, None));
}
```

The strings are baked into the first paint, per locale, tied to a path. See
[`examples/i18n-demo`](../../../examples/i18n-demo) for a runnable end-to-end
demo (bundled switch + opt-in fetch + an SSG build via
`cargo run -p i18n-demo --example ssg`).

---

## API reference

Generated by `i18n! { … }`:

| Item | Description |
| --- | --- |
| `Locale` (enum) | One variant per declared locale. `Copy`/`Eq`/`Hash`. |
| `Locale::code()` / `from_code(&str)` | Code string ↔ variant. |
| `Locale::is_lazy()` | Whether it's an opt-in locale. |
| `Locale::{ALL, BUNDLED, DEFAULT}` | All / bundled-only locales; the default. |
| `set_locale(Locale)` | Switch language (loads the pack for a lazy locale). |
| `current_locale() -> Locale` | The active locale. |
| `init()` | Seed the active locale to `DEFAULT`. |
| `greeting(..) -> Reactive<String>` | One function per message. |

From the `i18n` crate:

| Item | Description |
| --- | --- |
| `set_locale_code(&str)` / `current_locale_code()` | Untyped locale access. |
| `install_pack(code, map)` / `install_pack_json(code, json)` | Install an opt-in pack. |
| `set_pack_loader(Fn(&str))` / `ensure_pack_loaded(code)` | Opt-in loader seam. |
| `net_pack_loader(base)` *(feature `lazy-fetch`)* | Ready-made network loader. |
| `install_formatter(..)` / `clear_formatter()` | Swap in a custom message formatter. |
