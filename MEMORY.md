# The memory model

How EO objects live in memory, how they are born, and how their memory is
given back — without a garbage collector.

Status: design, 2026-09-28. This document is the source of truth for
memory. Where it disagrees with `DESIGN.md` (pillars P7 and P9) or with
eojse's README, this document wins. §6 and §7, the frames, pool, blocks
and regions, are built and measured (M0 in `README.md`); the rest is
still design, and §15 says what to measure before it is built.

## 1. The idea in one paragraph

Every evaluation of an attribute body is a frame. A frame gets a slice of
memory, allocates into it by bumping a pointer, and gives the whole slice
back by moving the pointer when it ends. The one thing a frame leaves
behind is its result, and the result has exactly one destination — the
object whose slot asked for it, or the frame that called. When the frame
ends, the result's young part is copied to that destination and the slice
is reset. Nothing is ever searched to find out what is alive: liveness is
structural. There is no mark, no sweep, no compaction, no free list of
objects, no root scanning.

## 2. What EO gives us

Six facts about EO make this possible. Each rule below leans on one of
them.

- **F1 — Objects never change.** After birth, the only write into an
  existing object is a slot receiving its value: a bound attribute read
  for the first time, or a lazy argument being forced. Nothing else.
- **F2 — Nothing is computed until asked.** Evaluation happens only
  because a slot wants its value or a dataization wants bytes. So the
  destination of a value is known before the value exists.
- **F3 — Identity is unobservable.** No EO program can tell one copy of an
  object from another. The runtime may copy or share freely. The only
  observable count is how many times an *effectful* atom ran, which the
  memo law already governs (eoc decision 53).
- **F4 — All data is bytes, and only `Φ.bytes` carries it.** `bytes` is
  `[@] > bytes`; its φ is the only void in the whole home that receives a
  `⟦ Δ ⤍ … ⟧` formation. `number` is `[@] > number`; its φ receives a
  `bytes`. Nothing else has a slot that holds raw data.
- **F5 — `malloc` stores bytes, not pointers.** A memory block is a
  payload, so writing into it never creates a pointer between objects.
- **F6 — Evaluation is a stack.** A body evaluation finishes before its
  caller continues. Frames nest; they never overlap or outlive each
  other.

## 3. Values

A value is one 64-bit word. It is one of:

| kind | what it holds | allocates |
|---|---|---|
| number | an f64 | nothing |
| bool | true or false | nothing |
| small payload | up to 6 bytes, with their length | nothing |
| pointer | the address of an object | — |
| empty | a void slot not yet bound | nothing |

An immediate word (number, bool, small payload) stands for an EO object —
`Φ.number(φ ↦ Φ.bytes(φ ↦ …))`, `Φ.true`, `Φ.bytes(φ ↦ …)` — but no
object exists for it. When a program dispatches on it (`x.plus`, `b.eq`),
the machine **inflates** it: allocates the instance in the current frame
and puts the word in its φ slot. That instance dies with the frame like
any other.

The exact bit layout (NaN-boxing, tag bits) is pillar P1's business, not
this document's.

## 4. Objects

An object is a few consecutive words in an object block:

```
header   shape number · flags
ρ        the object this one was reached from
slot 0   one word per void, in shape order
…
slot k   one word per bound attribute the shape remembers
```

Code is not in the object. The shape table, built once per program, says
for each shape: which names are voids and at what offset, which bound
attributes have a slot, where φ and λ are, and the code of every body.
Reading `x.a` is: load the header, index the shape table by (shape, `a`),
act on what it says. No names at run time, no hashing. A site whose
receiver has a singleton type knows the shape before the run and pays
neither the load nor the index (§18).

A void slot holds a value word, or a pointer to a **thunk**: a two-word
object (header naming the code, one word for the environment). Forcing a
thunk runs the code and overwrites the slot with the result (F1's one
write). A thunk is born in the same region as the object holding it.

Sizes: `[left right]` is 4 words. A thunk is 2. A `bytes` instance is 3
(header, ρ, φ). Compare eoc today: about 500 resident bytes and six heap
nodes per tree node in `bench/rivals/trees`.

## 5. Bytes

By F4, exactly one slot in the whole home holds raw data: the φ of
`Φ.bytes`. So:

- The `⟦ Δ ⤍ … ⟧` formation never exists as an object. A `bytes`
  instance's φ slot **is** the payload word.
- The payload word is either a small payload (≤ 6 bytes, inline) or a
  pointer into a **byte block** (§7), where the bytes lie length-prefixed.
- Dataization ends when it reaches a `bytes` instance or an immediate.
  "Is this data?" is one compare on the header or the tag.
- Atoms take payload words straight out of the φ slot and hand payload
  words back. There is no wrapping or unwrapping.
- `number`'s φ slot holds a bytes value. A number immediate is that whole
  two-level tower collapsed into one word.

Payloads are born in byte blocks that belong to the same region as the
object they belong to, and die with it. Payloads larger than a block get
a dedicated block of their own size in the same region's list.

## 6. Frames

A **frame** is one evaluation of an attribute body:

- reading a bound attribute of an object for the first time (a slot fill);
- forcing a thunk held by a void slot (a slot fill);
- running an atom (λ is a body written in Rust);
- one step of dataization, `x := value of x.φ` (a slot fill on `x`).

Frames nest by F6. The machine's own frame stack (eoc M9) holds one entry
per frame, and the entry holds the frame's region descriptor (§7). The
frame's depth is its **frame number**; deeper is younger.

Frame 0 is the **program region**: static templates, the root, and
anything cached into them. It is never given back.

## 7. Blocks and regions

Memory is a pool of equal blocks — 4 KB to start; tuning, not design.
Each block has a header: owner frame number and a link to the next
block of the same region. A kind word joins them with oversized blocks
(§13).

A **region** is a frame's memory: two lists of blocks, one for objects and
one for bytes, described by six words in the frame's stack entry —
current block, bump pointer, limit, for each list.

Allocating `n` words:

```
if bump + n > limit:
    block = pool.pop()            take a fresh block
    block.owner = this frame
    block.next  = current; current = block
    bump = block.start; limit = block.end
object = bump
bump  += n
```

One compare on the common path. When a body creates more than fits, its
region simply gets another block; nothing moves, no other region notices.

Ending a frame: push every block of both lists back onto the pool. Cost
is proportional to the number of blocks, not to the objects in them.
The pool is a free list; a pop and a push. When it runs dry, ask the
operating system for a few megabytes and cut them up.

"Which is older, this object or that one?" is: mask each address to its
block base, read the owner word, compare two integers. Position in RAM
means nothing.

Because a region is a list rather than an address range, an *older*
region can grow while younger frames sit above it on the stack — it just
takes another block from the pool. That is what §9 needs.

## 8. Where things are born

**Everything a frame creates is born in the frame's own region.** The
dispatch copies it makes, the application copies, the thunks it stores in
those copies, the inflated instances, the payloads its atoms build.

This is the young region. Almost all of it is garbage by the time the
frame ends — eoc measured 99.96% across the suite — and all of it is
freed by §7's pointer move.

## 9. Where results go

A frame ends with a result. The result has one destination:

- a **slot fill** stores it into the holder's slot, and hands the same
  pointer to the parent frame;
- a **return** (an atom's result, a dataization's bytes) hands it to the
  parent frame.

If the result is an immediate, store or hand it over and reset. Done.

If the result is a pointer into the frame's own region, **copy its young
part to the destination region** first: the object, and everything it
reaches that lives in a region younger than the destination. Pointers
into regions as old as the destination or older are kept as they are.
The copy is a small work-list walk; pointers inside the copied group are
rewritten as the group is copied. Then store or hand over the pointer to
the copy, and reset the frame.

**The young part is a graph, not a tree.** `pair x x` with `x` an
unforced argument reaches the same thunk twice; ξ and thunk environments
make cycles. A copy that does not notice would make two thunks, and an
effectful atom behind them would run twice — the memo law forbids that.
So the copy is Cheney's: when an object is copied, its dying original's
header is overwritten with the address of the copy, and every later
pointer to the original is redirected there instead of copying again.
The forwarding word lives only in memory that the reset is about to
throw away; nothing persists past the frame.

**Copy small, relink large.** Copying is right while the young part is a
handful of objects. It is wrong for a frame that hands over a big
structure it just built — with strict evaluation (§12) a subtree would be
copied again at every level it passes through, and a list built by
recursion would go quadratic. So the copy runs under a budget; past it,
stop copying and **splice the frame's blocks into the destination
region** instead: relink both block lists, rewrite their owner words, the
way §13 already moves an oversized payload. The result stays where it is,
nothing else is touched, and the frame's garbage travels with it into the
destination region, where it lives until that region ends. Words already
copied before the budget ran out are wasted once, bounded by the budget.

The destination region is the holder's region for a slot fill and the
parent frame's region for a return. Both are older than the frame that
is ending, and for a slot fill the holder is at least as old as the
parent (§10), so one copy serves both the slot and the parent.

The young part is usually small: the result object, its ρ chain up to
the first object that already lives in an older region, and the thunks
born with it. A structure built lazily arrives one node per frame, each
node written into a parent that has already settled, so the whole
structure is copied once — never per level.

**No forwarding pointers after the frame.** The only pointer that
survives a frame is the one it stores or hands over, and that pointer
already names the copy. Nothing else can hold the young original,
because nothing older ever saw it (§10). This is why eojse's `fwd`,
`ref_holders` and `written_attrs` do not exist here: forwarding is a
scratch word inside one copy, never a fact the runtime has to remember.

## 10. The invariant

**An object never points into a region younger than its own.**

It holds by construction:

- At birth, an object's slots hold what its frame holds: things born in
  the same frame (same region) or reached through ξ, ρ and Φ (older, by
  induction). A body can name nothing else — EO scoping guarantees it.
- A slot fill (F1's one write) stores a pointer to a copy that §9 has
  just placed in the holder's own region.
- An application of an old object never writes into it: it makes a copy
  in the current frame and fills the copy.
- `malloc` writes bytes (F5).

The invariant is what makes the reset in §7 safe: when a frame ends,
nothing outside its region points into it, because the only pointer that
could — the result — now points at a copy elsewhere.

A debug build asserts the invariant on every store. A production build
does nothing.

## 11. The chain

The holder of a slot may later be cached into an older holder, and that
one into an older one still. The model never asks how long a holder will
live. Each link of the chain is created by a slot fill, each slot fill
ends a frame, and each frame's end copies the result's young part —
including anything that was copied to the holder earlier, because that is
younger than the new destination. The chain is handled one link at a
time, when the link appears.

An object can therefore be copied more than once, once per link, each
time toward the root. Widely shared objects settle low quickly.

## 12. Optimization: born at the destination

When a body only constructs — a formation literal, or an application of a
dispatch chain, with no dataization inside it — everything the body
allocates is part of its result. For such a body the compiler emits: set
the allocation pointer to the holder's region for the duration, evaluate,
store, restore. No copy, because there is nothing that would not be
copied anyway. `point 3 4 > origin` is this shape.

This is an optimization per body, decided from its syntax, never the
default. As the default it would send every computation's temporaries to
long-lived regions and free nothing: every `fibo` instance would end up in
the root's region. A type widens what the syntax can see (§18).

It does not catch a body with an `if` or a strict `minus` in it, which is
what a tree builder looks like. That matters once the eraser (P6) makes
construction strict: a strictly built subtree is a finished structure
when its frame ends, and §9's plain copy would move it again at every
level above. The relink rule in §9 is what keeps strictness affordable;
the eraser must not be turned on for builders without it.

Other optimizations the model licenses, all compiler work and all
optional: skip the frame for a body that allocates nothing; fuse the
dispatch copy with the application that follows it (`x.f a b` fills the
fresh copy in place — nobody else has seen it); allocate the ρ-stamped
copy of a returned value directly as the copy of §9 with ρ set, instead
of copying twice.

## 13. Payloads across frames

A payload is copied like an object: allocate in the destination's byte
blocks, `memcpy`. A payload in a dedicated oversized block is not copied
at all — the block is unlinked from one region's list and linked into the
other's, and its owner word rewritten. Constant time regardless of size.

A dataization's result is a payload word. If it points into byte blocks,
those blocks belong to a `bytes` instance the dataization reached, which
lives in the region of the object dataized or older — never in the
dataization's own frame — so handing it up needs no copy.

## 14. Costs, honestly

- **The copy at frame end**, proportional to the result's young part.
  This is the price of not knowing where an object will end up when it is
  born. It is the whole price.
- **Garbage carried by a relink.** When a frame's blocks are spliced
  into the destination (§9), its garbage goes along and lives until that
  region ends. Only frames with big results relink, and a frame that
  builds something big is mostly result, so the ratio is small — but it
  is memory the copy would have freed.
- **Dead thunks in old regions.** Forcing a thunk overwrites the slot;
  the two-word thunk stays in its region until that region dies. Once per
  slot, so bounded by live data, not by running time.
- **Computed constants.** A value cached into a program-region object
  (`Φ.foo.bar` computed once) takes its garbage-free copy into frame 0
  forever. Java holds it forever too.
- **Memoized φ chains.** `D(x)` caches `x.φ`, `x.φ.φ`, … in `x`'s region,
  so a loop written as a φ chain holds its whole history as long as `x`
  lives. That is the memo law's cost, identical in Java; the model does
  not add to it.
- **Block tails.** When the next object does not fit, the tail of the
  block stays empty — at most one object per block, a fraction of a
  percent. A region that receives one small copy holds a whole block for
  it; if that is common, side blocks get a smaller size class.
- **Thunk chains.** A lazy argument closes over the object it was written
  in; until forced it keeps that object alive, and the copy at frame end
  drags the chain along. Laziness, not memory; every lazy runtime has it.

## 15. What to measure first

Four numbers decide whether this design is nearly free or merely better:

1. **Escape rate** — of all frames, how many end with a pointer into their
   own region (rather than an immediate or an older pointer).
2. **Young size** — when they do, how many words the young part has,
   by shape of the result.
3. **Copy depth** — how many times the same object gets copied across
   chain links (§11) before it settles. This is the quadratic detector:
   a depth that grows with input size says the relink budget in §9 is
   wrong or missing.
4. **Share count** — inside one young part, how many objects are reached
   more than once. This is the forwarding detector: zero would mean the
   Cheney word in §9 never fires; anything else is where a naive copy
   would have doubled a thunk.

eoc's machine has the slot-write site — `written()` at
`eo-rt/src/reduction.rs:896`, the `Put::Slot` arm at line 912 — and runs
the 2,180-test home. All four counters fit there; a day of work. Count
them over the suite, `bench/rivals/trees` and the corpus. If the young
parts are a handful of objects and copy depth stays flat, §9's copy is
noise. If they are large and frequent, we learn where and why before
building anything.

Fifth, once §12 exists: how many bodies are constructor-shaped, and what
share of stores they cover.

Sixth, split the first two by whether the result's site carries a
singleton type in the inference tables (§18). That is the share of the copy
a type import removes, known before either is built. It is counted in eoc,
where the tables are; this project has no types to split by.

## 16. What this replaces in eoc

- P7 (bump, then mark-sweep with conservative roots) — gone. No collector,
  no root scan, no 56-byte uniform cell. Objects are variable-size words
  in blocks.
- P9's copy at dispatch stays as is; its planned fusion is §12.
- `--bump` and `--regions` as experiments — this is the model they were
  probing.
- `heap.rs` becomes the pool and the region descriptor; the frame stack
  entry grows by six words.
- The direct lane's values in Rust locals are unaffected: an immediate
  needs no region, and a pointer held by a frame always points to a
  region at least as old as that frame.

## 17. Open questions

- **⊥ as a value.** eoc aborts today; `recovered` needs a representable
  bottom (decisions 44/45). It is an immediate; nothing here changes.
- **Threads.** One frame stack per thread, one pool shared under a lock
  or per thread. Sharing an object across threads means its region must
  outlive both — out of scope until threads exist.
- **Frame numbers.** A `u32` on a machine stack that already bounds depth
  by memory. Overflow is not a practical concern.
- **The direct lane and §12.** A constructor body compiled to plain Rust
  values may skip the region entirely; where it hands an object to the
  machine, that object is born by §8 in the current frame.

## 18. What a type is worth here

eo-inference works out, for every object of a program, which formation it
is a copy of, and what it answers as once the decorators in front of it are
folded. It is a whole-program analysis: every filling of every void is in
the tables, so what a void holds is a fact about the program, and an answer
rooted at a void is a union of formations, never an open question. The
tables are keyed on the locator the XMIR already carries, and section 7 of
eoc's `COMPILER.md` says how they come in. This section says what they are
worth once they have.

**The model needs none of it.** Every rule above holds with nothing known
about any object, and the objects the tables leave at ⊤ run the same paths
as the rest. A type removes run-time steps at the sites it covers and never
changes what is correct. Whatever a type licenses, eoc's differential suite
must still pass with the licence withheld.

**A shape is a type made physical.** One shape per formation, one locator
per formation, one table. The formation an object is a copy of is its shape
and fixes its layout; the name `Reduced` writes beside it is what the object
answers as and fixes where a dispatch lands. A site with a singleton type
knows the shape number at compile time, so the header load and the table
index of §4 go, the slot offset is a constant, and the allocation is a
constant bump.

**Two kinds of result are immediates by type alone.** A literal, and the
result of an atom whose annotation names a datum, `[] > plus /Q.number`.
Those frames have no region and no copy, and the escape rate of §15 is zero
for them before anything is measured. A decorator in front of a number is
still an object: `[] > five` with `5 > @` answers as a number and is born
as a `five`, so a folded name licenses nothing about the frame's result.
Only the dataization that walks behind it ends in the immediate.

**§12 widens.** The constructor rule reads a body's syntax and stops at any
dispatch it cannot see through. Only an atom dataizes, so a body every
dispatch of which resolves without passing an atom, on its own type or on
every member of its union, only constructs, and is born at the destination.
This is where a type buys the model the most: it turns the copy of §9 into
no copy for bodies the syntax alone would send through it.

**A union is a switch.** The tree of `fork` and `leaf` is one site with two
shapes; a small union compiles to a compare per member, and a large one, or
⊤, keeps the dynamic path of §4. ⊥ is in every set, so every compiled
shortcut keeps the branch that hands control back to the machine.

**What a type says nothing about.** How long anything lives: that is the
frame's business and nobody else's. Whether a thunk may be skipped: a
datum-typed void still takes a thunk until the eraser (P6) proves the
demand, and the type says only that one word will be left behind when it is
forced. And the chain of §11 and the copy of §9 for everything the types do
not reach.
