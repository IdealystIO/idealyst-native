# datetime

The current date and time on every target the framework builds for: the
wall-clock instant, the local UTC offset at any instant, and the local IANA
zone name. An optional `chrono` feature hands the same readings over as
chrono values.

```toml
datetime = { workspace = true }                        # inside the workspace
datetime = { workspace = true, features = ["chrono"] } # with chrono values
```

```rust
let now = datetime::now_utc();                 // Timestamp: µs since 1970-01-01T00:00:00Z
let ms = now.unix_millis();
let offset = datetime::local_offset_at(now);   // UtcOffset at that instant (DST-correct)
let here = datetime::local_offset();           // the offset right now
let zone = datetime::local_timezone();         // Some("Europe/Berlin") when known
```

## API

| Item | What it is |
| --- | --- |
| `now_utc() -> Timestamp` | The current instant. Microseconds on native; whole milliseconds on web (`Date.now()`). |
| `local_offset_at(Timestamp) -> UtcOffset` | The local zone's offset at that instant, under the rule in force then: a July instant in New York is `-04:00`, a January one `-05:00`, and 2006-03-20 is `-05:00` because DST started in April that year. |
| `local_offset() -> UtcOffset` | `local_offset_at(now_utc())`, reading the clock once. |
| `local_timezone() -> Option<String>` | The IANA zone name (`"America/New_York"`), or `None` when the platform doesn't report one. |
| `Timestamp` | An instant. `from_unix_{micros,millis,seconds}`, `unix_{micros,millis,seconds}` (rounded toward the past), `micros_since`. Ordered, hashable, `Copy`. |
| `UtcOffset` | Seconds to add to UTC to get local time, always within ±24 h. `from_seconds` / `from_minutes` (`None` out of range), `seconds()`, `minutes()`, `Display` as `+05:30`. |
| `Clock`, `SystemClock`, `FixedClock` | Where reads come from. See "Fake time" below. |
| `with_clock(clock, f)`, `set_clock(clock)`, `reset_clock()` | Swap the clock for one thread for the length of a closure, or for the whole process. |

With the `chrono` feature:

| Item | Replaces |
| --- | --- |
| `now_chrono_utc() -> DateTime<Utc>` | `chrono::Utc::now()` |
| `now_chrono_local() -> DateTime<FixedOffset>` | `chrono::Local::now()` |
| `to_chrono_local(Timestamp) -> DateTime<FixedOffset>` | `dt.with_timezone(&chrono::Local)` |
| `From<Timestamp> for DateTime<Utc>`, `From<DateTime<Tz>> for Timestamp`, `UtcOffset` ⇄ `FixedOffset` | conversions both ways |

That is the whole surface. Formatting, parsing and calendar arithmetic are
chrono's job (or whichever date library the app uses); this crate only reads
the clock and the zone.

## Moving an app off chrono's clock

chrono's `Utc::now()` and `Local::now()` need its `clock` feature, and on
web they also need `wasmbind`, which links `wasm-bindgen` + `js-sys` into the
app. An app that links them builds its web target in hybrid mode (the
`wasm-bindgen` CLI runs on every build; on CrewForge that is +4.5 s per
rebuild and +3.4 GB peak memory). Without `wasmbind`, chrono's clock falls
back to `std::time::SystemTime`, which panics on `wasm32-unknown-unknown`.

1. Depend on `datetime` with the `chrono` feature.
2. Turn chrono's clock off: `chrono = { version = "0.4", default-features =
   false, features = ["std"] }` (add back `serde` or anything else the app
   uses, but not `clock`, `now` or `wasmbind`; `clock` reaches
   `iana-time-zone`, which links `js-sys` + `wasm-bindgen` on wasm32). A
   crate that only runs on a server can keep `clock`.
3. Replace each clock read:

| Before | After |
| --- | --- |
| `Utc::now()` | `datetime::now_chrono_utc()` |
| `Utc::now().naive_utc()` | `datetime::now_chrono_utc().naive_utc()` |
| `Utc::now().date_naive()` / `Utc::today()` | `datetime::now_chrono_utc().date_naive()` |
| `Utc::now().timestamp_millis()` | `datetime::now_utc().unix_millis()` |
| `Local::now()` | `datetime::now_chrono_local()` |
| `Local::now().date_naive()` / `Local::today()` | `datetime::now_chrono_local().date_naive()` |
| `Local::now().naive_local()` | `datetime::now_chrono_local().naive_local()` |
| `Local::now().offset().fix().local_minus_utc()` | `datetime::local_offset().seconds()` |
| `dt.with_timezone(&Local)` | `datetime::to_chrono_local(dt.into())` |

`now_chrono_local()` returns `DateTime<FixedOffset>`, not
`DateTime<Local>`, because chrono's `Local` type is the clock this crate
replaces. The methods an app calls on `Local::now()` (`date_naive`, `time`,
`naive_local`, `offset`, `format`, the `Datelike` / `Timelike` accessors)
behave the same; a function signature that names `DateTime<Local>` changes to
`DateTime<FixedOffset>`. Turning a local wall time into an instant
(`Local.from_local_datetime(..)`, which has to handle DST gaps and overlaps)
is not covered.

## Platforms

| Target | Instant | Offset at an instant | Zone name |
| --- | --- | --- | --- |
| Web (wasm32) | `Date.now()` | `new Date(ms).getTimezoneOffset()` | `Intl.DateTimeFormat().resolvedOptions().timeZone` |
| iOS, macOS | `SystemTime` | libc `localtime_r` (`tm_gmtoff`) | `$TZ`, else CFTimeZone |
| Android | `SystemTime` | bionic `localtime_r` | `$TZ`, else the `persist.sys.timezone` property |
| Linux, BSD | `SystemTime` | libc `localtime_r` | `$TZ`, else `/etc/localtime` / `/etc/timezone` |
| Windows | `SystemTime` | `SystemTimeToTzSpecificLocalTimeEx` with the dynamic zone (per-year DST rules) | the WinRT calendar zone |
| anything else (e.g. wasm32-wasi) | `SystemTime` | UTC | `None` |

The web arm binds `Date` and `Intl` through web-glue, so it works in a page
and in a worker, and it needs no `window`. The native arms also cover the
terminal, CPU, desktop (wgpu) and SSR/server builds, all of which run as
native processes and use the host's clock and zone.

On unix, `$TZ` comes first for the name because `localtime_r` also honors
it, so the name and the offsets always describe the same zone. A `$TZ`
holding a POSIX rule string (`EST5EDT,M3.2.0,M11.1.0`) still drives the
offsets, but `local_timezone()` returns `None`, because no zone name
describes such a rule. Every read is fresh (`tzset` runs before each
`localtime_r`), so a zone change in the system settings mid-run shows up on
the next call.

## Fake time

Every read goes through a `Clock`. `SystemClock` is the platform; a
`FixedClock` is stopped at one instant with one offset and an optional zone
name; implement `Clock` yourself for anything else (e.g. an offset that
changes at a chosen instant, to test DST handling).

```rust
use datetime::{with_clock, FixedClock, Timestamp, UtcOffset};

let clock = FixedClock::new(Timestamp::from_unix_millis(1_700_000_000_000))
    .with_offset(UtcOffset::from_minutes(-300).unwrap())
    .with_timezone("America/New_York");
with_clock(clock, || {
    // Every datetime read on this thread, including now_chrono_local(), sees the fake.
});
```

- `with_clock(clock, f)` applies to the current thread for the length of
  `f`. It nests, and it is restored even if `f` panics. Rust runs tests on
  parallel threads, so this is the one to use in tests.
- `set_clock(clock)` applies to every thread until `reset_clock()`, which is
  what a screenshot run or an end-to-end fixture needs. A `with_clock`
  scope still takes precedence on its own thread.

When neither is set, a read costs one atomic load on top of the platform
call.

## Where it lives

This is an SDK crate, not part of `runtime_core::time`. `runtime_core::time`
already has a wall clock (`epoch_millis()`, `local_offset_minutes()`, the
`WallClockSource` a backend installs), and it serves UI inside a mounted app,
such as a date picker's "today". It doesn't fit this job, for three reasons:

- **It needs a mounted backend.** The core clock reads `0` (1970) until a
  backend installs a source, which happens at `mount`. Code that reads the
  date without a mount (`#[server]` functions and other server code, tests,
  CLI tools, anything that runs before `mount`) would get 1970. This crate
  reads the platform directly and works from the first line of `main`.
- **Zone lookup is per-target code.** The offset at an arbitrary instant and
  the zone name need libc, Win32, CFTimeZone and `Intl`. Core keeps
  compile-target switches out (it picks platform defaults by the runtime
  `Platform`, not by `#[cfg]`). An SDK may hold per-target arms, as
  `haptics`, `connectivity` and the other device SDKs do.
- **chrono interop is a convenience.** CLAUDE.md §3 keeps conveniences out
  of the core vocabulary.

The two don't conflict: the core seam is what the framework's own UI reads
inside a mounted app, and both read the same platform clock for the instant
and the same zone lookup for the offset. On native targets that lookup is
the `zone-offset` crate (libc `localtime_r` on unix,
`SystemTimeToTzSpecificLocalTimeEx` on Windows): this crate's
`local_offset_at` calls it, and so does core's default native wall clock
(`SystemWallClockSource`), which every non-web backend installs at mount.
On web both read `Date`. So inside a mounted app
`runtime_core::time::local_offset_minutes()` and `datetime::local_offset()`
report the same offset on every backend. `zone-offset` sits below both
because runtime crates may not depend on SDK crates, and this crate must
not depend on the runtime.

## Tests

- `cargo test -p datetime --features chrono` (also without the feature):
  unit tests; `tests/portable.rs` (real clock is plausible, scoped fake
  clocks, nesting and unwinding, chrono conversions);
  `tests/global_clock.rs` (`set_clock`, in its own binary);
  `tests/system_zone.rs`, which re-runs the test binary as a child process
  with `$TZ` set to New York, Berlin, Sydney, Kolkata, UTC and a POSIX rule,
  and checks offsets at fixed instants: either side of the 2024 US and EU
  DST changes, both hemispheres, a half-hour zone, and 2006's pre-2007 US
  rule.
- `cargo test -p datetime --target wasm32-unknown-unknown --features chrono`
  (browser, through the workspace runner): every reading is checked against
  `Date` / `Intl` called independently. Chrome takes its zone from `$TZ`, so
  `TZ=America/New_York cargo test …` also checks exact New York DST values.
- The wasm32 graph has no wasm-bindgen:
  `cargo tree --target wasm32-unknown-unknown -i wasm-bindgen -p datetime
  --features chrono -e normal,build` prints nothing. Without `-e`, the only
  match is the `wasm-bindgen-test` dev-dependency that the browser test
  harness needs, which never reaches an app.
- Cross-target: `cargo check -p datetime --features chrono --tests --target
  <t>` for `aarch64-apple-ios`, `aarch64-linux-android`,
  `x86_64-pc-windows-gnu`, `x86_64-unknown-linux-gnu`.
