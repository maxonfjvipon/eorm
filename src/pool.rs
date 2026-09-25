//! The pool of equal 4 KB blocks that all memory comes from.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::ptr::NonNull;

/// The size of a block in bytes, which is also its alignment.
const BLOCK: usize = 4096;

/// The word that links one free block or one chunk to the next.
type Link = Option<NonNull<u8>>;

/// A source of equal blocks of memory.
///
/// A region takes a block when its current one is full and gives all
/// of its blocks back when its frame ends; the pool is where they come
/// from and where they return, so both moves cost one pointer swap.
pub trait Pool {
    /// Hands out a free block of 4 KB that starts on a 4 KB boundary.
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
/// A chunk is `count` blocks in a row, taken straight from `System` and
/// aligned to 4 KB, followed by one link word:
///
/// ```text
///   chunk ──► ┌─────────┬─────────┬─────────┬──────┐
///             │ block 0 │ block 1 │ block 2 │ link │──► older chunk
///             └─────────┴─────────┴─────────┴──────┘
/// ```
///
/// The links chain every chunk ever taken, so `drop` can give them all
/// back to `System`. A free block holds, in its first word, the address
/// of the next free block, so the free list is threaded through the
/// free blocks themselves:
///
/// ```text
///   free ──► ┌──────┬───┐   ┌──────┬───┐   ┌──────┬───┐
///            │ next │   │──►│ next │   │──►│ None │   │
///            └──────┴───┘   └──────┴───┘   └──────┴───┘
/// ```
///
/// Neither list needs memory of its own, so the pool never touches the
/// global allocator. Every `unsafe` block here is sound for one reason:
/// each address it reads or writes lies inside a live chunk, is aligned
/// for a link word, and is either a free block, whose memory the pool
/// owns until `pop` hands it out, or a chunk's link word, which nobody
/// but the pool ever sees. A block is 4 KB aligned because the chunk is
/// and blocks sit at multiples of 4 KB inside it; the link word sits at
/// a multiple of 4 KB too, past the last block and inside the chunk's
/// layout. Chunks are freed with the same layout they were taken with,
/// once each, when the pool itself dies.
pub struct Blocks {
    count: usize,
    free: Link,
    chunks: Link,
}

impl Blocks {
    /// A pool that takes `count` blocks at a time from the system.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            count,
            free: None,
            chunks: None,
        }
    }
    /// The layout of one chunk: `count` blocks and a link word, 4 KB aligned.
    fn layout(&self) -> Layout {
        self.count
            .checked_mul(BLOCK)
            .and_then(|size| size.checked_add(size_of::<Link>()))
            .and_then(|size| Layout::from_size_align(size, BLOCK).ok())
            .unwrap_or_else(|| panic!("a chunk of {} blocks does not fit in memory", self.count))
    }
    /// The link word of a chunk, right past its last block.
    ///
    /// The chunk must be one this pool took from `System` and has not
    /// freed yet, so the offset stays inside its allocation.
    unsafe fn tail(&self, chunk: NonNull<u8>) -> NonNull<Link> {
        unsafe { chunk.add(self.count * BLOCK) }.cast()
    }
    /// Takes one more chunk from `System` and pushes all of its blocks.
    fn grow(&mut self) {
        let chunk = NonNull::new(unsafe { System.alloc(self.layout()) })
            .unwrap_or_else(|| handle_alloc_error(self.layout()));
        unsafe { self.tail(chunk).write(self.chunks) };
        self.chunks = Some(chunk);
        for index in 0..self.count {
            unsafe { self.push(chunk.add(index * BLOCK)) };
        }
    }
}

impl Default for Blocks {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl Pool for Blocks {
    fn pop(&mut self) -> NonNull<u8> {
        if self.free.is_none() {
            self.grow();
        }
        let block = self
            .free
            .unwrap_or_else(|| panic!("a chunk of {} blocks has no block to pop", self.count));
        self.free = unsafe { block.cast::<Link>().read() };
        block
    }
    unsafe fn push(&mut self, block: NonNull<u8>) {
        unsafe { block.cast::<Link>().write(self.free) };
        self.free = Some(block);
    }
}

impl Drop for Blocks {
    fn drop(&mut self) {
        while let Some(chunk) = self.chunks {
            self.chunks = unsafe { self.tail(chunk).read() };
            unsafe { System.dealloc(chunk.as_ptr(), self.layout()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Blocks, Pool};
    use std::collections::HashSet;

    #[test]
    fn pushed_block_comes_back_on_next_pop() {
        let mut pool = Blocks::new(7);
        let block = pool.pop();
        pool.pop();
        unsafe { pool.push(block) };
        assert_eq!(
            pool.pop(),
            block,
            "the pushed block does not come back on the next pop"
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
        let start = (0..3)
            .map(|_| pool.pop().addr().get())
            .min()
            .unwrap_or_default();
        assert!(
            !(start..start + 3 * 4096).contains(&pool.pop().addr().get()),
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
    fn many_pops_give_distinct_blocks() {
        let mut pool = Blocks::new(5);
        assert_eq!(
            (0..37).map(|_| pool.pop()).collect::<HashSet<_>>().len(),
            37,
            "two pops give the same block"
        );
    }
}
