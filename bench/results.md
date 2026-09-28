# Bench

Written by `cargo run --release --bin bench`; every run rewrites it.

| machine | |
|---|---|
| CPU | Apple M1 Max |
| OS | Darwin 24.6.0 arm64 |
| load | { 9.73 18.73 22.62 } |
| rustc | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| profile | release |

Each row: a warm-up that doubles the repetitions until one run takes
10 ms, then 15 samples of that many repetitions. `median` is the
median of the samples in ns per operation, `spread` is the fastest
and the slowest sample, `ops` is the operations timed per sample.
Allocations run in frame 1, the youngest; a block holds 510 words
after its two-word header.

| what | size | median ns | spread ns | ops |
|---|---|---|---|---|
| pool `pop` + `push` | 1 × 4 KB | 3.51 | 3.48–3.56 | 4194304 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 2 words | 2.50 | 2.45–2.58 | 6528000 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 4 words | 2.60 | 2.54–2.68 | 6502400 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 8 words | 2.49 | 2.44–2.66 | 6451200 |
| `alloc` inside one block, fast path only | 2 words | 2.41 | 2.38–2.46 | 4161536 |
| `alloc` inside one block, fast path only | 4 words | 2.31 | 2.23–2.35 | 4128768 |
| `alloc` inside one block, fast path only | 8 words | 2.21 | 2.20–2.24 | 4063232 |
| `enter` + `leave` of an empty frame | 0 × 4 KB | 1.27 | 1.26–1.29 | 8388608 |
| `enter` + fill + `leave`, per block | 1 × 4 KB | 3.93 | 3.83–4.05 | 4194304 |
| `enter` + fill + `leave`, per block | 10 × 4 KB | 3.95 | 3.82–4.41 | 2621440 |
| `enter` + fill + `leave`, per block | 100 × 4 KB | 6.54 | 6.37–6.75 | 1638400 |
| `leave` alone, per block | 1 × 4 KB | 1.89 | 1.81–2.12 | 3276800 |
| `leave` alone, per block | 10 × 4 KB | 2.08 | 2.01–2.12 | 1638400 |
| `leave` alone, per block | 100 × 4 KB | 2.68 | 2.59–2.95 | 1638400 |

eoc, from README §7: 163 ns per object, 45 to allocate and 117 to
collect.
