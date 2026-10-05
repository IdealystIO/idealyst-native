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

/// One workload by name (the benchmark's table).
pub const WORKLOADS: &[(&str, fn(u32) -> u64, u32, &str)] = &[
    ("fib", fib, 27, "recursive Fibonacci(27): calls"),
    ("sort", sort, 200_000, "sort 200k random u32s: memory, branches"),
    ("hash", hash, 256, "FNV-1a over 256 KB x8: integer math"),
    ("matmul", matmul, 96, "96x96 f64 matrix multiply: floating point"),
    ("text", text, 20_000, "build + parse 20k JSON-like records: allocation, strings"),
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
}
