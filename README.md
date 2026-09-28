# eorm — the EO memory model, in Rust

A small Rust program that runs hand-written EO programs on real memory
exactly the way `MEMORY.md` says, and measures whether the theory holds.

Status: M0 done, 2026-09-28: the pool, regions and frames are built,
tested and measured. M1 is next.

## 1. Why this exists

EO's runtimes are slow because of their object model, not their host
language. The Java runtime spends ~150 Java objects per EO object. eoc
(`~/code/rust/eoc`) is lighter but still pays 163 ns to allocate and free
each of 970 million objects across the test suite, and its collector is
40% of all CPU — a share no optimization pass moved.

eojse (`~/code/javascript/eo-runtime`) simulated a flat memory in
JavaScript and found the two ideas we keep: an instance is born at
dispatch with ρ attached, and a forced attribute overwrites its slot. It
also found the wall: a cached attribute makes an old object point at a
young one, and protecting the young object where it lies brings back
mark, holes, compaction and pointer fixing — a garbage collector.

`MEMORY.md` is the theory that removes the wall: a frame's memory is a
list of blocks; a frame's result is copied to its one destination when
the frame ends; an object never points into a region younger than its
own; nothing is ever searched to learn what is alive. This project exists
to find out, in nanoseconds and bytes, whether that theory is right.

## 2. What it is and is not

It **is**: Rust, real memory (4 KB blocks from the operating system,
objects as words at real addresses, copies as `memcpy`), a tiny machine
(dispatch, application, dataization, thunks, the memo law), a handful of
atoms, and eight hand-written programs with counters around them.

It **is not**: a parser, a compiler, a runtime for real EO programs, or a
replacement for eoc. Programs are written by hand as shape tables and
bodies in Rust, the way eojse wrote `memory.push(...)`. When the four
questions below are answered, the memory module moves into eoc, where the
2,180-test differential suite checks the semantics against Java.

Two lessons from eojse are conditions here: **real memory, not a
simulation** (a JS array has no cache lines, and the open questions are
about cost), and **no parser** (a week that teaches nothing about
memory).

Out of scope too: `MEMORY.md` §18, what a type is worth. Every program
here is a shape table its author wrote, so every shape is known in
advance, which is the typed case. The machine must not use that: it reads
the shape off the header and indexes the table, because the cost this
project measures is the one the model pays with no type at hand. What a
type removes is measured in eoc, against the inference tables.

## 3. The four questions

Each is pass or fail. The project is done when each has a written answer
in §9.

1. **Does the invariant hold by construction?** An object never points
   into a region younger than its own (`MEMORY.md` §10). Debug builds
   assert it on every store, in every program. One failure means a
   collector is hiding in the theory.
2. **Do effects run exactly once under copying?** `twice` and `chain`
   with the counting atom. This is the memo law surviving the Cheney copy
   of a young part that is a graph, not a tree (`MEMORY.md` §9).
3. **Does copy depth stay flat as input grows?** `list` and strict
   `trees` at increasing sizes. This is the relink budget doing its job
   (`MEMORY.md` §9, "copy small, relink large").
4. **What does it cost?** Nanoseconds per allocation and per `leave`,
   against eoc's 163 ns per object. Peak memory and time on `trees`,
   against eoc's 1,080 MB and 1,109 ms at depth 20.

## 4. Parts

| part | file | what |
|---|---|---|
| values | `value.rs` | the tagged 64-bit word: float, bool, small payload, pointer, empty (`MEMORY.md` §3) |
| pool | `pool.rs` | 4 KB blocks from the OS, intrusive free list (the next pointer lives inside the free block; the pool never allocates for itself) |
| regions | `region.rs` | per-frame descriptor: two block lists (objects, bytes), bump and limit each; `alloc`, `bytes`, `release` (§7) |
| frames | `frames.rs` | the frame stack: owns the pool and one region per open frame; `enter`, `leave`, `alloc` into any open frame, `region_of` (§6, §7) |
| copy | `copy.rs` | the Cheney copy of a young part under a budget, forwarding word in the dying original's header; relink of both lists past the budget (§9) |
| shapes | `shape.rs` | the static table per program: voids, memo slots, φ, λ, bodies as Rust functions (§4) |
| objects | `object.rs` | header, ρ, slots; thunks (§4) |
| machine | `machine.rs` | dispatch with φ fall-through, application copy and in-place fill, the dataize loop, the memo law exactly as eoc matched it (eoc decision 53) |
| atoms | `atoms.rs` | `plus`, `minus`, `lt`, `eq`, `concat`; `tick` — an effectful atom that counts how often it ran |
| programs | `programs/` | the eight programs of §6 |
| counters | `count.rs` | the four counters of `MEMORY.md` §15, time, peak blocks |
| bench | `bench/results.md` | the table of §7, rewritten by `cargo run --release --bin bench` |

## 5. Milestones

Work one milestone at a time. Each ends with a report and waits for "go".

### M0 — pool and regions

Goal: memory in and out with no objects yet.

- `pool.rs`, `region.rs`, `enter`/`leave` for immediate results only.
- Angry tests: a block goes out and comes back; the owner word is the
  frame number; an allocation that does not fit takes a new block; `leave`
  returns every block of both lists; an older region grows a side block
  while a younger frame is open; the pool allocates nothing of its own
  (assert on the global allocator).
- Done when: tests green; `alloc` and `enter`/`leave` measured in ns.
- Done, 2026-09-28. Carried forward: past 32 blocks a frame costs more
  per block, likely because block headers share a few cache sets (#34);
  `leave` costs per block, not per object.

### M1 — objects and the machine

Goal: `fibo` runs.

- `value.rs`, `shape.rs`, `object.rs`, `machine.rs`; atoms `plus`,
  `minus`, `lt`.
- Dispatch reads the header and indexes the shape table; φ fall-through;
  application copies and fills; dataize follows φ to a payload word.
- Done when: `fibo 25` answers 75025 with the debug invariant on; ns per
  object born; frames per call.

### M2 — laziness and the memo law

Goal: effects run once.

- Thunks in void slots, forcing overwrites the slot; the memo law of eoc
  decision 53: the pre-stamp value is cached on the holder, a result that
  still declares ρ is copied and stamped on every read, an effectful λ is
  never memoized; ρ-stamping at dispatch.
- `leave` copies the young part: a plain Cheney copy with the forwarding
  word, no budget yet.
- Programs `chain`, `twice`; atom `tick`.
- Done when: `tick` counts exactly once where Java would run it once; the
  invariant holds; copy depth of `v` in `chain` is 2.

### M3 — copy small, relink large

Goal: no quadratic anywhere.

- The budget; relink of both lists with owner words rewritten; the
  partially copied words counted as waste.
- Programs `trees` lazy, `trees` strict, `list`, `bytes`.
- Counters copy depth and share count.
- Done when: copy depth is flat with relink and grows without it, both
  recorded; strict and lazy `trees` give the same answer; `bytes` relinks
  an oversized payload without copying it.

### M4 — counters and the bench

Goal: the table.

- All four counters, time, peak blocks, objects born, frames.
- `bench/results.md` in the format of §7, with eoc's numbers beside ours.
- Done when: §9 has an answer for each of the four questions.

### After M4

If all four pass: `MEMORY.md` §16 — `heap.rs` in eoc becomes the pool and
the region descriptor, the frame stack entry grows by six words, the
suite must stay `diff=0`, `trees` is the score. The counters of
`MEMORY.md` §15 also go into eoc's `written()`
(`eo-rt/src/reduction.rs:896`) to get the real distributions from the
suite. If one fails: it failed in a small room; fix the theory in
`MEMORY.md` first.

## 6. Programs

Each program is a shape table and bodies in Rust under `programs/`,
with the EO it stands for in its docblock.

| program | EO shape | what it tests |
|---|---|---|
| `fibo n` | `[n] > fibo` with `if`, `lt`, `minus`, `plus` | many frames, every result an immediate; escape rate ≈ 0; allocation cost |
| `chain` | `g` born in frame 1, `h` in frame 3, `v` in frame 7; `v` cached into `h`, later `h` cached into `g` | the invariant across chain links; `v` copied twice; `tick` behind `v` runs once |
| `twice` | `pair x x` with `x` an unforced `tick` | sharing inside one young part; without forwarding `tick` runs twice |
| `loop n` | `[i] > loop` recursing through φ to `n` | per-step frame reset; memo chain grows linearly (expected, same as Java); time per step flat |
| `trees d` lazy | `fork (build (d.minus 1)) (build (d.minus 1))`, then `sum` | each node copied once into a settled parent; peak memory against eoc's 1,080 MB |
| `trees d` strict | same, built eagerly | copy per level without relink; flat with it |
| `list n` | `(build (n.minus 1)).with n` | the quadratic detector |
| `bytes` | a string grown by `concat` past a block, cached into an old holder | payload copy; oversized relink without `memcpy` |

## 7. The bench table

One row per program and size, rewritten fresh on every run:

```
program · size · answer · time · peak blocks · objects born · frames ·
escapes · young words copied · relinks · waste words · copy depth max ·
share count · ns per object · ns per leave
```

eoc's numbers to put beside them: 163 ns per object (45 allocate, 117
collect); `trees` depth 20: 1,109 ms, 1,080 MB, 12.5 M heap nodes for
2.1 M tree nodes; `fibo 30`: ~1.05 s. Rust hand-written `trees`: 42 ms,
34 MB. Plain OO Java: 10.5 ms, 89 MB.

## 8. Working rules

The same rules as eoc, kept here so the project can be picked up cold.

- Simple words in every answer and every document; no jargon.
- One milestone at a time. A milestone report ends "Say go when ready"
  and waits.
- Commit after each change: one line, `feat: <name> (M<n>)` or
  `docs: <name>`, no body, no trailers, never mention Claude.
- TDD and angry tests: one assertion per test, as its last statement; a
  failure message phrased negatively; test names as sentences; no shared
  fixtures, no setup or teardown.
- Measurement first. Every number in this file has a command that
  produced it.
- When a milestone closes, the status lines at the top of this file and
  of `MEMORY.md` move with it.
- Every optimization is a flag, default on, with one switch that kills
  them all. Correctness never lives inside an optimization.
- ASCII docblocks on every struct explaining purpose, not usage. No
  inline comments. No blank lines in method bodies. No `-er` names. Every
  struct final and small, one to four fields.
- Never suppress a warning (`#[allow(...)]`); fix the code.
- GNU tools on the command line.
- Max owns vision and architecture; push back on technical mistakes.

## 9. Answers

Filled in as milestones close.

1. Invariant by construction — *open*.
2. Effects once under copying — *open*.
3. Copy depth flat — *open*.
4. Cost — *open*. So far, from `bench/results.md`, made by
   `cargo run --release --bin bench`: an allocation costs about 2.5 ns
   against eoc's 45, and `leave` gives blocks back at about 2 ns per 4 KB
   block. That is only the floor of collecting: nothing is copied until
   M2 or relinked until M3.

## 10. Decisions

- **D1** — Rust with real memory, not a JavaScript simulation. The open
  questions are about cost; a simulation cannot measure cost.
- **D2** — No parser. Programs are shape tables and bodies written by
  hand.
- **D3** — The memo law is eoc decision 53, Java's rule, taken as given.
  This project tests memory, not semantics; semantics are checked later
  by eoc's suite.
- **D4** — `MEMORY.md` lives here and is the source of truth. eoc's copy
  is frozen at commit `a7b6d6a` with a pointer to this one.

## 11. Open questions carried from `MEMORY.md` §17

⊥ as an immediate; threads; frame numbers as `u32`; how a constructor
body compiled to plain Rust values (eoc's direct lane) hands objects to
the machine. None blocks M0–M4.
