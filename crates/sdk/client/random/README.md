# random

Random numbers that behave the same on every platform.

```rust
let roll = random::range(1..=6);          // dice
let pick = random::choose(&["a", "b"]);   // Option<&&str>
random::shuffle(&mut deck);
let p = random::float();                  // [0, 1)

let mut token = [0u8; 32];
random::secure_bytes(&mut token);         // OS entropy, for secrets
```

## Two layers

| | What | Source | Use for |
| --- | --- | --- | --- |
| **Entropy** | `secure_bytes`, `secure_u64` | The OS cryptographic source: `getentropy` (Apple), `getrandom(2)` (Linux, Android), `ProcessPrng` (Windows), `crypto.getRandomValues` (web) | Tokens, ids, keys, nonces — anything an attacker must not guess |
| **Generation** | `Rng`, and the free functions `range` / `float` / `bool` / `chance` / `shuffle` / `choose` / `u64` | xoshiro256++, implemented in this crate | Games, sampling, jitter, content — anything that should be fast, and reproducible when seeded |

The generator is ours rather than the platform's because platform helpers
disagree: `Math.random()` is a different algorithm in each browser engine,
`arc4random_uniform` / `java.util.Random` / `Math.random` map bits to ranges
differently, and none can be seeded the same way across platforms. Here every
derived operation is plain integer arithmetic in this crate, so a seed yields
the identical sequence — and the same rolls, floats and shuffles — on iOS,
Android, desktop and web.

`Rng` is **not** for secrets: its output is predictable from earlier output.

## Seeding

The free functions draw from a per-thread `Rng` seeded from OS entropy on
first use. Seed it for tests, replays, or reproducible content:

```rust
random::seed(42);
assert_eq!(random::range(1..=6), random::Rng::seeded(42).range(1..=6));
```

Or own a generator: `let mut rng = random::Rng::seeded(7);`.

## Web

Web entropy goes through `web-glue` (the framework's own JS boundary), not
`getrandom`, whose web backend links wasm-bindgen and would put every app
using this crate into the hybrid build. `tests/no_wasm_bindgen.rs` pins
that.

### `getrandom` 0.2 for dependencies (`getrandom-0-2` feature)

Some dependencies reach `getrandom` 0.2 (e.g. `rand` 0.8 inside `tract`),
which has no default wasm backend; its `js` backend links wasm-bindgen. Enable
`random`'s `getrandom-0-2` feature instead: it registers a custom backend fed
by the web-glue entropy above. getrandom allows one registration per binary,
so SDKs enable this feature rather than registering their own — and must name
the crate (`use random as _;`), because rustc does not link a dependency
nothing references. `denoise` is the example.

## Tests

- `tests/determinism.rs` — reference xoshiro256++ vectors, a 10,000-draw
  differential against `rand_xoshiro` for several seeds, unbiased ranges,
  half-open floats, shuffle, `seed`, OS entropy.
- `tests/web_random.rs` — the same golden sequences in the browser, plus
  `getRandomValues` chunking past its 65,536-byte limit. Run with
  `cargo test -p random --target wasm32-unknown-unknown` and a `CHROMEDRIVER`
  matching your Chrome.
- `tests/web_getrandom02.rs` — with `--features getrandom-0-2`, getrandom 0.2
  in the browser draws from the registered backend.
