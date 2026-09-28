# Bench

Written by `cargo run --release --bin bench`; every run rewrites it.

| machine | |
|---|---|
| CPU | Apple M1 Max |
| OS | Darwin 24.6.0 arm64 |
| load | { 7.32 18.75 20.01 } |
| rustc | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| profile | release |

Each row: a warm-up that doubles the repetitions until the timed part
of one run takes 10 ms, then 15 samples of that many repetitions. `median` is the
median of the samples in ns per operation, `spread` is the fastest
and the slowest sample, `ops` is the operations timed per sample.
Allocations run in the youngest frame: frame 1, or for `leave` alone
each of the nested frames while it is the youngest. A block holds 510
words after its header, measured by this run.

| what | size | median ns | spread ns | ops |
|---|---|---|---|---|
| pool `pop` + `push` | 1 × 4 KB | 3.59 | 3.47–3.64 | 4194304 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 2 words | 2.54 | 2.44–2.59 | 6528000 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 4 words | 2.56 | 2.55–2.62 | 6502400 |
| `alloc`, refills and `enter`/`leave` spread over 100 blocks | 8 words | 2.50 | 2.47–2.54 | 6451200 |
| `alloc` inside one block, fast path only | 2 words | 2.45 | 2.38–2.48 | 4161536 |
| `alloc` inside one block, fast path only | 4 words | 2.36 | 2.32–2.43 | 8257536 |
| `alloc` inside one block, fast path only | 8 words | 2.28 | 2.20–2.34 | 8126464 |
| `enter` + `leave` of an empty frame | 0 × 4 KB | 1.29 | 1.28–1.32 | 8388608 |
| `enter` + fill + `leave`, per block | 1 × 4 KB | 3.84 | 3.83–3.90 | 4194304 |
| `enter` + fill + `leave`, per block | 10 × 4 KB | 4.07 | 3.87–4.46 | 2621440 |
| `enter` + fill + `leave`, per block | 100 × 4 KB | 6.47 | 6.38–6.61 | 1638400 |
| `leave` alone, per block | 1 × 4 KB | 1.74 | 1.60–1.79 | 6553600 |
| `leave` alone, per block | 10 × 4 KB | 1.74 | 1.70–1.77 | 6553600 |
| `leave` alone, per block | 100 × 4 KB | 2.35 | 2.34–2.47 | 6553600 |

eoc, from README §7: 163 ns per object, 45 to allocate and 117 to
collect.
