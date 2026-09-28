//! The pool of equal 4 KB blocks that all memory comes from.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::ptr::NonNull;

/// The word that links one free block or one chunk to the next.
type Link = Option<NonNull<u8>>;

/// A source of equal blocks of memory.
///
/// A region takes a block when its current one is full and gives all
/// of its blocks back when its frame ends; the pool is where they come
/// from and where they return, so both moves cost one pointer swap.
///
/// # Safety
///
/// Regions write into the blocks they pop without asking, so a pool
/// must keep its promises: `SIZE` is a power of two, a whole number of
/// words, and larger than a block header of two words; every block
/// `pop` hands out is `SIZE` bytes of live memory aligned to `SIZE`,
/// and nobody else holds it until it is pushed back.
pub unsafe trait Pool {
    /// The size of every block in bytes, which is also its alignment.
    const SIZE: usize;
    /// Hands out a free block of `SIZE` bytes aligned to `SIZE`.
    fn pop(&mut self) -> NonNull<u8>;
    /// Takes a block back, so a later `pop` may hand it out again.
    ///
    /// # Safety
    ///
    /// The block must come from `pop` of this very pool, must not be in
    /// the pool already, and nobody may touch its memory afterwards.
    unsafe fn push(&mut self, block: NonNull<u8>);
}

/// Blocks cut from chunks of system memory.
///
/// A chunk is `count` blocks in a row, taken straight from `System`,
/// exactly `count * 4096` bytes and aligned to 4 KB. Block 0 is the
/// chunk's header and is never handed out; its first word links to the
/// chunk taken before it:
///
/// ```text
///   chunks ──► ┌────────┬─────────┬─────────┬─────────┐
///              │ header │ block 1 │ block 2 │ block 3 │
///              └───┬────┴─────────┴─────────┴─────────┘
///                  └──► older chunk's header ──► ... ──► None
/// ```
///
/// The headers chain every chunk ever taken, so `drop` can give them
/// all back to `System`. The blocks of the newest chunk that nobody has
/// used yet are a fresh run, handed out in address order by bumping the
/// `fresh` cursor, which is `None` once the run is used up:
///
/// ```text
///   chunks ──► ┌────────┬─────────┬─────────┬─────────┬─────────┐
///              │ header │ in use  │ in use  │  fresh  │  fresh  │
///              └────────┴─────────┴─────────┴─────────┴─────────┘
///                                             ▲                   ▲
///                                           fresh      chunk + count * 4096
/// ```
///
/// Nothing writes into a fresh block before a caller has it, so the
/// operating system backs a page of a chunk with real memory only when
/// its block is first used. A block that comes back through `push` is
/// linked into the free list, which is threaded through the free blocks
/// themselves, the first word of each holding the address of the next:
///
/// ```text
///   free ──► ┌──────┬───┐   ┌──────┬───┐   ┌──────┬───┐
///            │ next │   │──►│ next │   │──►│ None │   │
///            └──────┴───┘   └──────┴───┘   └──────┴───┘
/// ```
///
/// `pop` takes from the free list first, then from the fresh run, and
/// takes a new chunk only when both are empty.
///
/// None of this needs memory of its own, so the pool never touches the
/// global allocator, and the chunk layout is an exact multiple of 4 KB,
/// so no byte of it spills into a page the pool cannot use. Every
/// `unsafe` block here is sound for one reason: each address it reads
/// or writes is the first word of a block inside a live chunk, which is
/// aligned for a link word, and that block is either free, so the pool
/// owns its memory until `pop` hands it out, or a header, which nobody
/// but the pool ever sees. Every block is 4 KB aligned because the
/// chunk is and blocks sit at multiples of 4 KB inside it. The fresh
/// cursor never leaves its chunk: it moves one block forward only when
/// the next block still starts before the chunk's end, and becomes
/// `None` otherwise, and since a chunk has at least two blocks, block 1,
/// the one `grow` hands out, lies inside it. Chunks are freed with the
/// same layout they were taken with, once each, when the pool itself
/// dies.
pub struct Blocks {
    count: usize,
    free: Link,
    fresh: Link,
    chunks: Link,
}

impl Blocks {
    /// A pool that takes `count` blocks at a time from the system, one
    /// of them being the chunk's header.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            count,
            free: None,
            fresh: None,
            chunks: None,
        }
    }
    /// The layout of one chunk: `count` blocks, 4 KB aligned.
    fn layout(&self) -> Layout {
        assert!(
            self.count > 1,
            "a chunk of {} blocks has no block besides its header",
            self.count
        );
        self.count
            .checked_mul(Self::SIZE)
            .and_then(|size| Layout::from_size_align(size, Self::SIZE).ok())
            .unwrap_or_else(|| panic!("a chunk of {} blocks does not fit in memory", self.count))
    }
    /// The block right after `block` in the newest chunk, or `None` when
    /// `block` is the chunk's last one.
    fn next(&self, block: NonNull<u8>) -> Link {
        self.chunks
            .filter(|chunk| {
                block.addr().get() + Self::SIZE < chunk.addr().get() + self.count * Self::SIZE
            })
            .map(|_| unsafe { block.add(Self::SIZE) })
    }
    /// Takes one more chunk from `System`, starts its fresh run right
    /// after block 1, and hands out block 1.
    fn grow(&mut self) -> NonNull<u8> {
        let chunk = NonNull::new(unsafe { System.alloc(self.layout()) })
            .unwrap_or_else(|| handle_alloc_error(self.layout()));
        unsafe { chunk.cast::<Link>().write(self.chunks) };
        self.chunks = Some(chunk);
        let block = unsafe { chunk.add(Self::SIZE) };
        self.fresh = self.next(block);
        block
    }
}

impl Default for Blocks {
    fn default() -> Self {
        Self::new(1024)
    }
}

unsafe impl Pool for Blocks {
    const SIZE: usize = 4096;
    fn pop(&mut self) -> NonNull<u8> {
        match (self.free, self.fresh) {
            (Some(block), _) => {
                self.free = unsafe { block.cast::<Link>().read() };
                block
            }
            (None, Some(block)) => {
                self.fresh = self.next(block);
                block
            }
            (None, None) => self.grow(),
        }
    }
    unsafe fn push(&mut self, block: NonNull<u8>) {
        unsafe { block.cast::<Link>().write(self.free) };
        self.free = Some(block);
    }
}

impl Drop for Blocks {
    fn drop(&mut self) {
        while let Some(chunk) = self.chunks {
            self.chunks = unsafe { chunk.cast::<Link>().read() };
            unsafe { System.dealloc(chunk.as_ptr(), self.layout()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Blocks, Pool};
    use std::collections::HashSet;

    #[test]
    fn pushed_block_is_handed_out_before_a_fresh_one() {
        let mut pool = Blocks::new(7);
        let block = pool.pop();
        pool.pop();
        unsafe { pool.push(block) };
        assert_eq!(
            pool.pop(),
            block,
            "a fresh block is handed out while a pushed one waits"
        );
    }

    #[test]
    fn popped_blocks_start_on_4kb_boundaries() {
        let mut pool = Blocks::new(3);
        assert!(
            (0..29).all(|_| pool.pop().addr().get().is_multiple_of(4096)),
            "a popped block does not start on a 4 KB boundary"
        );
    }

    #[test]
    fn popping_past_one_chunk_takes_another_chunk() {
        let mut pool = Blocks::new(3);
        let start = (0..2)
            .map(|_| pool.pop().addr().get())
            .min()
            .unwrap_or_default();
        assert!(
            !(start - 4096..start + 2 * 4096).contains(&pool.pop().addr().get()),
            "the block popped past the first chunk does not come from another chunk"
        );
    }

    #[test]
    fn blocks_from_several_chunks_dont_overlap() {
        let mut pool = Blocks::new(13);
        let mut address: Vec<usize> = (0..41).map(|_| pool.pop().addr().get()).collect();
        address.sort_unstable();
        assert!(
            address.windows(2).all(|pair| pair[1] - pair[0] >= 4096),
            "two popped blocks overlap"
        );
    }

    #[test]
    fn header_block_of_a_chunk_is_never_handed_out() {
        let mut pool = Blocks::new(2);
        let mut address: Vec<usize> = (0..19).map(|_| pool.pop().addr().get()).collect();
        address.sort_unstable();
        assert!(
            address.windows(2).all(|pair| pair[1] - pair[0] >= 2 * 4096),
            "the header block of a chunk is handed out by a pop"
        );
    }

    #[test]
    fn many_pops_give_distinct_blocks() {
        let mut pool = Blocks::new(5);
        assert_eq!(
            (0..37).map(|_| pool.pop()).collect::<HashSet<_>>().len(),
            37,
            "two pops give the same block"
        );
    }

    #[test]
    fn fresh_chunk_hands_out_its_blocks_in_address_order() {
        let mut pool = Blocks::new(9);
        let address: Vec<usize> = (0..8).map(|_| pool.pop().addr().get()).collect();
        assert!(
            address.windows(2).all(|pair| pair[1] == pair[0] + 4096),
            "a fresh chunk does not hand out its blocks one after another"
        );
    }

    #[test]
    fn pop_after_a_whole_chunk_is_the_block_right_after_the_one_past_it() {
        let mut pool = Blocks::new(6);
        for _ in 0..5 {
            pool.pop();
        }
        let next = pool.pop().addr().get();
        assert_eq!(
            pool.pop().addr().get(),
            next + 4096,
            "the pop after the one past a whole chunk does not come one block later"
        );
    }
}
