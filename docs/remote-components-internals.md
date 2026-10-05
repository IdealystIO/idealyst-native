# Remote components: how they work

This explains what happens inside the framework when an app runs a remote
component. To use the feature, start with
[remote-components.md](remote-components.md).

## The one-sentence version

A bundle runs the real framework code, compiled to wasm and interpreted on the
device. Its reactive state lives in the app's own reactive graph, and the UI tree
it builds crosses to the app as data, which the app renders with its own
backend.

There is one reactive graph and one renderer. The bundle carries only its own
code, plus the parts of the framework it calls.

## The pieces

| Piece | Where | What it does |
|---|---|---|
| `#[component(remote)]` | runtime-macros | Compiles one component three ways (below) |
| The bridged kernel | runtime-world (`bridge` feature) | Runs a bundle's signals, effects and scopes on the app's graph |
| The element codec | runtime-vocabulary (`remote` feature) | Sends the bundle's UI tree to the app as data |
| `remote-host` | its own crate | The loader: instantiates a bundle in wasmi, wires its imports, mounts its components |
| `remote-bundle` | its own crate | The release format: metadata, requirements, signature, trust check, and `check` |
| `build-remote` | CLI tooling | `idealyst build --remote` |
| `ota`, `ota-index`, `ota-publish` | their own crates | Over-the-air delivery (see [ota.md](ota.md)); built only on the pieces above |

## One source, three builds

A bundle crate's source compiles differently depending on the build:

- **Bundle build** (`--cfg idealyst_stream_guest`, for `wasm32-unknown-unknown`):
  - A `#[component(remote)]`'s body is compiled, along with an exported mount
    function named after it (`module_path::Name`).
  - Every other `#[component]` becomes a stub that asks the app for its own
    copy.
  - A `#[host_fn]` becomes a stub that calls the app.
- **Native app build**: a `#[component(remote)]`'s body is left out. Its
  function sends the props to the installed loader and mounts whatever the
  bundle builds. Every `#[component]` registers itself, so bundles can ask for
  it by name.
- **Web build**: `remote` does nothing, and the body compiles like any
  component's.

The bundle flag is a compiler `--cfg`, not a cargo feature, so it can never reach
an app build through cargo's feature unification. The macros don't emit the cfg
themselves: they go through helper macros in the vocabulary, so a crate that
uses `#[component(remote)]` never mentions it.

## The bridged kernel

runtime-world's reactive layer (signals, effects, memos, scopes, context) runs on
an `Engine`:

- **Natively**, the engine is the arena that holds every signal's value and its
  subscribers.
- **In a bundle**, the engine is `Bridged`. It keeps in the bundle only what
  can't leave it: signal values, effect bodies, cleanup closures and context
  values. Every graph operation (creating a slot, subscribing, staging a write,
  flushing, entering a scope) is a wasm import that the app answers.

So a bundle's signal is a real slot in the app's arena, holding a proxy that
reaches into the bundle for the value. The app's flush runs the bundle's
effects, and the app's scopes own the bundle's state: unmounting a remote
component tears its state down exactly like a native component's.

Adding a method to `Engine` doesn't compile until every engine implements it.
The whole kernel test suite also runs through the bridge in one process
(`--features loopback-engine`), so the two engines can't drift apart.

### Signals the app shares

A `ReadSignal`/`Signal` prop, or one inside context, crosses as a **handle**: the
app exports its signal, and the bundle imports it.
- **Reads** subscribe in the app's graph, so when the app writes, the bundle's
  effects rerun.
- **Writes** from the bundle land in the app's signal.
- **Exports are counted**, so two mounts sharing one signal don't withdraw it
  from each other.

### Signals the bundle hands to native code

When remote code passes a signal it created to an app component, the signal is
**promoted**. The app's side, which knows the value's type, takes the value
over:
- The slot keeps its subscribers, and the bundle keeps its handle.
- The value now lives natively, so native code reads and writes it at native
  speed.
- Promotion has two phases, so a value the app can't decode leaves the bundle
  untouched.

## The element codec

When a remote component builds its tree, the bundle encodes the `Element` as
data:

- **Closures stay in the bundle.** A dynamic text getter, a press handler, a
  list's item builder or a stylesheet's style function each go into a table in
  the bundle under an id, and the id is sent instead. The app decodes the tree
  into real primitives whose closures call the bundle by id. When the app drops
  such a closure, it releases the id, and the bundle frees the closure.
- **Scopes cross as ids** the app claims, so the component's state belongs to
  the app's tree.
- **Styles keep token names.** A stylesheet crosses as its shape: its variant
  axes, and functions that live in the bundle. The app rebuilds the same sheet,
  so its state, breakpoint and container overlays work exactly as for a native
  sheet. Token names resolve against the app's theme.
- **App components cross by name.** A stub sends only the props its call site
  set, by name. The app starts from its own defaults and overwrites those. A
  prop the app's version of the component doesn't have fails, naming it, rather
  than being misread.
- **Values with behaviour cross by key.** Things a library defines with code
  behind them (idea-theme's tones and variants) cross as a key. The app rebuilds
  its own registered value from the key.
- **Every builtin primitive has a recorded decision.** A test fails when a new
  primitive is added without saying whether it crosses. All of them cross
  except `graphics` (a GPU surface) and `lazy` (meaningless inside a bundle).
- **A tree that fails to decode leaks nothing.** Every reply that carries a tree
  also lists what crossed with it (callback ids, sheets, scopes). A failed decode
  releases whatever it never reached. A subtree built later (a list row, a
  screen) that fails shows the error in its own place.

### Handles and refs

A node handle is a node plus a table of operations. The bundle's handles use a
forwarding table: each method call crosses to the app and runs on the real
handle, which the app holds once the backend fills the ref. A handle belongs to
the bundle whose tree holds it, and ends when that tree unmounts. A call from
another bundle is refused, and that bundle is stopped.

### Context

A type that derives `Remote` registers itself as context under its name, in both
builds. When remote code `inject`s it and the bundle's own tree provides none,
the bundle gets the app's value, encoded like a prop. Context types that don't
derive `Remote` aren't visible to bundles.

## Host functions

`#[host_fn]` emits two things:
- **In the app:** the function itself, plus a record listing its path, a
  fingerprint of its signature, and an entry point that decodes arguments and
  encodes the result.
- **In a bundle:** a stub that encodes the arguments and calls one wasm import
  named `<path>#<fingerprint>`.

At load, the app matches the bundle's imports against its allowlist. A missing
function or a changed fingerprint refuses the bundle before any of its code
runs. The fingerprint is a stable hash (FNV-1a) of the signature's type
spellings, so it doesn't depend on the compiler version.

An async host function returns at once. The app runs the real future on its
own executor, and delivers the result to the bundle as a one-shot callback,
which completes the bundle's future.

### Generic host functions

A generic function only exists in the app as the copies the app compiled, so
each type parameter is handled by its bound:

- **`Key`.** The app compiles the function once, with the parameter as
  `KeyBytes`: the key's bytes, whose order, equality and hash are the key's. The
  bundle encodes each key so that comparing the bytes gives the same answer as
  comparing the keys:
  - numbers are big-endian, with the sign bit flipped for signed ones;
  - strings and lists are escaped and terminated, so a prefix sorts first and
    another field can follow;
  - `Option`, tuples, arrays and derived fields are their parts in order;
  - `Reverse` inverts the bytes, which reverses the order because every
    encoding is prefix-free;
  - `#[derive(Key)]` generates `Ord`, `Eq` and `Hash` from the same fields, so a
    type can't carry an ordering its bytes disagree with.
- **`Opaque`, or no bound.** The app compiles the function once, with the
  parameter as `OpaqueBytes`: the value's encoding, carried unread.
- **`Numeric`.** The app compiles the function for all ten number types. The
  call starts with a tag byte naming the type.
- **Any other trait.** The app lists the types (`host_fn_instances!`). The call
  starts with the type's name (`RemoteName`, `module_path::Name`). An unlisted
  name is an error that stops the bundle.

The bundle's stub walks each argument's type, writing exactly the bytes the
app's substituted type reads: a key or opaque value carries its length, other
values cross as themselves. A list of fixed-width keys (`Key::WIDTH`) crosses as
one run with a single stride. The fingerprint spells a type parameter by its
position and kind, so renaming one keeps old bundles loading.

## The wire

- **Values** are postcard-encoded, except that a list of numbers is one
  little-endian block (a memcpy each way). The format has a version
  (`CODEC_VERSION`). A bundle reports its version through an export, and the
  loader refuses a bundle built with another version, before it can misread
  anything.
- **Bytes cross through two buffers in the bundle's memory.** The app asks the
  bundle for room, writes the arguments there, and calls in. The bundle copies
  the arguments out before running anything, because the call may re-enter the
  bundle and reuse the buffer. The receive buffer only grows and is reused, so a
  call doesn't clear it byte by byte.

## Several bundles

The loader (`remote_host::remote`) holds named bundles. A remote component
mounts from the first bundle whose exports include its mount function, so
which bundle serves what is read from the bundles themselves, not configured.

A mounted remote component re-mounts when the number it follows changes
(`Loader::generation_of`). Each bundle has its own number, bumped when it is
replaced, removed or stopped, so replacing one bundle leaves the others'
components, and their state, alone. Components no bundle provides follow a
shared "pending" number, bumped whenever a bundle is set.

All these numbers take their values from one sequence. A component switches
numbers when its bundle arrives, from "pending" to its bundle's own. Two
counters that both started at 0 gave that switch the same value, and the
placeholder never re-mounted.

## Panics and invalid requests

A Rust panic in wasm aborts: it traps the interpreter, and no destructors run, so
the bundle's internal state is left half-updated. So the first trap **stops**
the bundle for good:

1. **Kernel frames are closed.** The app records every frame a bundle opens
   (an entered world, a collecting scope, tracking switched off), and unwinds
   them on a trap. Without that, the app would be left inside frames the bundle
   never closed.
2. **Every later call into the bundle gets a safe answer.** An effect doesn't
   run, a handler does nothing, a getter returns its last value, a subtree
   renders nothing.
3. **Every remote component remounts.** The stopped bundle refuses the mounts,
   so each component shows the bundle's panic message in its place.
4. **The app carries on.** A reload brings the components back.

Release apps are built with `panic = "abort"`, so the app itself must never
panic on the bundle's behalf. A bundle that sends something invalid (a kernel
request out of order, a reply that doesn't decode, a pointer out of bounds,
bad host-function arguments) is reported as a fault and stopped the same way.

## Release bundles and signatures

`idealyst build --remote` compiles each declared bundle crate's library with
`cargo rustc --crate-type cdylib` for wasm32, under the bundle flag, and under
its own crate name. That name is what the app's stubs ask for, so nothing needs
renaming.

The build then:
- checks the imports against the three modules the loader provides
  (`idealyst_kernel`, `idealyst_ui` and `idealyst_host_fn`);
- reads the codec version from an `idealyst.codec` custom section, which the
  vocabulary stamps into every bundle;
- lists what the bundle requires of an app, and strips the code that listed
  it (below);
- writes the release format.

A release bundle is still one `.wasm` file, with custom sections that wasm
engines ignore:

- **`idealyst.bundle`:** the metadata (name, crate, version, codec) as JSON.
- **`idealyst.requires`:** what the bundle needs from the app (below), as
  JSON.
- **`idealyst.signature`:** always the last section. It holds the signing key's
  id (the first 8 bytes of the SHA-256 of the public key) and an Ed25519
  signature over every byte before it, the metadata included.

So the signature covers exactly what the app will run and what the bundle says
it is, and nothing can be appended after it unnoticed.

The app's `Trust` is checked before the module is even parsed, at install and
at every reload. A bundle that claims a key the app trusts must match it, even
when the app doesn't require signatures.

## What a bundle requires

A release bundle lists what it needs from the app (`idealyst.requires`), and
the loader checks the list against what the app provides before running any
of the bundle (`remote_bundle::check`).

**Shapes.** Values cross positionally, so a type's name isn't enough: a
`#[derive(Remote)]` struct that gained a field keeps its name and would be
misread. Every type that crosses has a *shape*, a canonical string of its
structure (`Invoice{lines:list<InvoiceLine{item:str,cents:u64,qty:u32}>,tax_percent:u32}`),
written by the `RemoteShape` trait (`runtime-vocabulary`'s `remote::shape`).
The derive writes its fields in order, and containers write their parts. A type
with no shape is `?`, which matches anything. Both sides run the same trait
code, so equal types give equal strings.

**Records of what the bundle uses.** Each place bundle code reaches the app
keeps a 40-byte constant, a `Site` (`remote::site`), with a magic prefix:

- a `ui!` call of an app component: the component, the props that call site
  sets, and a function that writes the shapes of its props. `ui!` passes the
  set as a type (`BuildElement::__build_site`), so the record can be a
  constant;
- a host function's stub: its import name and signature shape;
- a remote component's mount export: its parameters;
- a `#[derive(Remote)]` type's context registration: its shape.

The code that uses each record also passes it to `core::hint::black_box`.
Without that, the optimizer would read the fields it needs straight out of
the constant and drop the record. With it, the linker keeps a record exactly
as long as the code using it is reachable. A component used only from a
function nothing calls leaves no record.

A custom section would be simpler, but it keeps everything compiled, reachable
or not. A release build compiles every helper of every library the bundle
depends on, so the list would include components the bundle never uses.
Computing the list when the bundle runs is too late: it has to be known before
an app downloads the bundle.

**Reading them.** At release, `build_remote::requires`:
1. finds the records by their magic in the data segments;
2. runs each shape function once in wasmi, through the function table (bundles
   are linked with `--export-table`), with every import trapping;
3. assembles the list.

`strip_shapes` then points each record's table slot at one trapping function
and lets walrus remove what nothing else reaches. The shape code exists only
for the build tool, and stripping it costs nothing in download size: the
showcase's release is 130.9 KB brotli, the same as before the lists existed.

Context types are the one imprecise part. The app registers every derived type
as context at load, so there is no call site to tie a record to, and the list
holds every one compiled in. A context mismatch is therefore a warning, not an
error.

## What it costs

Measured on an Apple M3 Max with the showcase app, against the same component
code compiled natively:

| | Remote | Native |
|---|---|---|
| Bundle (the showcase's screens) | ~450 KB raw, ~125 KB brotli | — |
| Load a bundle | ~2 ms | — |
| Mount a screen of 3 idea-ui cards | ~1 ms | ~75 µs |
| A press handled by remote code | ~9 µs | ~0.4 µs |
| The app changes a signal a remote screen reads | ~12 µs | ~0.5 µs |
| A theme swap | ~50 µs | ~50 µs |

- **Most of the time is the interpreter running bundle code.** In a profile of
  repeated mounts, 87% of the time is wasmi executing the bundle's framework
  code. Sending the tree to the app, and the app building it, each take under
  1%.
- **UI work** runs 8–25× slower than native, and stays well within a frame.
- **Heavy computation** runs 16–340× slower, and belongs in a host function.
- **The interpreter runs in a loop that never grows the native stack.** Its
  faster default mode can overflow a thread's stack, depending on how the app is
  compiled; the loop costs about 1.8× on UI work.

The full measurement tables, and the history of the designs that came before
this one, are in [`crates/streaming/README.md`](../crates/streaming/README.md).
