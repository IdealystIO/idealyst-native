//! Compute workloads for `examples/measure.rs`: the same source runs as
//! native code in the app and, in the bundle, as wasm through the
//! interpreter — the comparison remote code's own logic pays.
//!
//! Each returns a checksum, so the two builds are checked to compute the
//! same thing and the optimizer can't drop the work. In the bundle each is
//! also a wasm export (`__bench_<name>`), called directly by the benchmark.

/// Deterministic pseudo-random stream (xorshift32).
fn xorshift(mut x: u32) -> impl FnMut() -> u32 {
    move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }
}

/// Call-heavy: naive recursive Fibonacci.
pub fn fib(n: u32) -> u64 {
    if n < 2 {
        n as u64
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

/// Memory and branches: sort `n` pseudo-random numbers.
pub fn sort(n: u32) -> u64 {
    let mut next = xorshift(0x9e37_79b9);
    let mut v: Vec<u32> = (0..n).map(|_| next()).collect();
    v.sort_unstable();
    v.iter().step_by(97).fold(0u64, |acc, x| acc.wrapping_mul(31).wrapping_add(*x as u64))
}

/// Integer arithmetic over a buffer: FNV-1a over `n` KB, 8 passes.
pub fn hash(kb: u32) -> u64 {
    let mut next = xorshift(7);
    let buf: Vec<u8> = (0..kb * 1024).map(|_| next() as u8).collect();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..8 {
        for b in &buf {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Floating point: multiply two `n`×`n` matrices.
pub fn matmul(n: u32) -> u64 {
    let n = n as usize;
    let a: Vec<f64> = (0..n * n).map(|i| (i % 7) as f64 * 0.5).collect();
    let b: Vec<f64> = (0..n * n).map(|i| (i % 5) as f64 * 0.25).collect();
    let mut c = vec![0.0f64; n * n];
    for i in 0..n {
        for k in 0..n {
            let aik = a[i * n + k];
            for j in 0..n {
                c[i * n + j] += aik * b[k * n + j];
            }
        }
    }
    c.iter().sum::<f64>() as u64
}

/// Allocation and text: build a JSON-like list of `n` records, then parse
/// the numbers back out.
pub fn text(n: u32) -> u64 {
    let mut s = String::from("[");
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"id\":");
        s.push_str(&i.to_string());
        s.push_str(",\"name\":\"item-");
        s.push_str(&(i * 7).to_string());
        s.push_str("\"}");
    }
    s.push(']');
    s.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty()).map(|t| t.parse::<u64>().unwrap_or(0)).sum()
}

// ---------------------------------------------------------------------------
// The same sorts, done by the APP for the bundle (`#[host_fn]`s). What that
// costs a bundle is the transport: encoding the values in interpreted
// wasm, the app decoding, sorting and encoding, the bundle decoding the
// answer. A list of numbers crosses as one byte run
// (`runtime_vocabulary::remote::bulk`); before it did, `sort_host` took
// longer than sorting in the bundle (339 ms against 182).
// - `sort_host`: `Vec<u32>` there and back.
// - `sort_host_key`: items with a two-field key. The bundle encodes the
//   keys order-preserving (big-endian, fields in order) at a fixed stride,
//   the app sorts those byte records and returns the permutation, and the
//   bundle applies it to its own items — only keys cross.
// In the app build each host fn is the plain function, so the "native"
// column is the whole workload native.
// ---------------------------------------------------------------------------

use runtime_core::host_fn;

#[host_fn]
pub fn bench_sort_u32(v: Vec<u32>) -> Vec<u32> {
    let mut v = v;
    v.sort_unstable();
    v
}

/// The permutation that sorts `keys`, records of `stride` bytes (stable,
/// like `sort_by_key`).
#[host_fn]
pub fn bench_order(stride: u32, keys: Vec<u8>) -> Vec<u32> {
    let s = stride as usize;
    let n = if s == 0 { 0 } else { keys.len() / s };
    let mut idx: Vec<u32> = (0..n as u32).collect();
    idx.sort_by(|a, b| keys[*a as usize * s..][..s].cmp(&keys[*b as usize * s..][..s]));
    idx
}

fn checksum(v: &[u32]) -> u64 {
    v.iter().step_by(97).fold(0u64, |acc, x| acc.wrapping_mul(31).wrapping_add(*x as u64))
}

pub fn sort_host(n: u32) -> u64 {
    let mut next = xorshift(0x9e37_79b9);
    let v: Vec<u32> = (0..n).map(|_| next()).collect();
    checksum(&bench_sort_u32(v))
}

/// Items with a two-field key `(group, id)`; sorted by the key.
pub fn sort_host_key(n: u32) -> u64 {
    let mut next = xorshift(0x9e37_79b9);
    let items: Vec<(u16, u32)> = (0..n).map(|_| { let x = next(); ((x >> 24) as u16, x) }).collect();
    let mut bytes = Vec::with_capacity(items.len() * 6);
    for (g, id) in &items {
        bytes.extend_from_slice(&g.to_be_bytes());
        bytes.extend_from_slice(&id.to_be_bytes());
    }
    let order = bench_order(6, bytes);
    let sorted: Vec<u32> = order.iter().map(|i| items[*i as usize].1).collect();
    checksum(&sorted)
}

/// The same, sorted in wasm (the baseline `sort_host_key` must beat).
pub fn sort_key(n: u32) -> u64 {
    let mut next = xorshift(0x9e37_79b9);
    let mut items: Vec<(u16, u32)> = (0..n).map(|_| { let x = next(); ((x >> 24) as u16, x) }).collect();
    items.sort_by_key(|(g, id)| (*g, *id));
    let sorted: Vec<u32> = items.iter().map(|(_, id)| *id).collect();
    checksum(&sorted)
}

/// One workload by name (the benchmark's table).
pub const WORKLOADS: &[(&str, fn(u32) -> u64, u32, &str)] = &[
    ("fib", fib, 27, "recursive Fibonacci(27): calls"),
    ("sort", sort, 200_000, "sort 200k random u32s: memory, branches"),
    ("hash", hash, 256, "FNV-1a over 256 KB x8: integer math"),
    ("matmul", matmul, 96, "96x96 f64 matrix multiply: floating point"),
    ("text", text, 20_000, "build + parse 20k JSON-like records: allocation, strings"),
    ("sort_host", sort_host, 200_000, "the same sort, by the app (a host fn)"),
    ("sort_key", sort_key, 200_000, "sort 200k items by a (u16, u32) key"),
    ("sort_host_key", sort_host_key, 200_000, "the same, the app sorting key bytes"),
];

// The bundle's exports, called directly by the benchmark (`KernelBundle::call`).
#[cfg(idealyst_stream_guest)]
mod exports {
    #[no_mangle]
    pub extern "C" fn __bench_fib(n: u32) -> u64 {
        super::fib(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_sort(n: u32) -> u64 {
        super::sort(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_hash(n: u32) -> u64 {
        super::hash(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_matmul(n: u32) -> u64 {
        super::matmul(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_text(n: u32) -> u64 {
        super::text(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_sort_host(n: u32) -> u64 {
        super::sort_host(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_sort_key(n: u32) -> u64 {
        super::sort_key(n)
    }
    #[no_mangle]
    pub extern "C" fn __bench_sort_host_key(n: u32) -> u64 {
        super::sort_host_key(n)
    }
}
