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
pub trait Pool {
    /// The size of every block in bytes, which is also its alignment.
    const SIZE: usize;
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
/// all back to `System`. A free block holds, in its first word, the
/// address of the next free block, so the free list is threaded through
/// the free blocks themselves:
///
/// ```text
///   free ──► ┌──────┬───┐   ┌──────┬───┐   ┌──────┬───┐
///            │ next │   │──►│ next │   │──►│ None │   │
///            └──────┴───┘   └──────┴───┘   └──────┴───┘
/// ```
///
/// Neither list needs memory of its own, so the pool never touches the
/// global allocator, and the chunk layout is an exact multiple of 4 KB,
/// so no byte of it spills into a page the pool cannot use. Every
/// `unsafe` block here is sound for one reason: each address it reads
/// or writes is the first word of a block inside a live chunk, which is
/// aligned for a link word, and that block is either free, so the pool
/// owns its memory until `pop` hands it out, or a header, which nobody
/// but the pool ever sees. Every block is 4 KB aligned because the
/// chunk is and blocks sit at multiples of 4 KB inside it; a chunk has
/// at least two blocks, so block 1, the one `grow` hands out, lies
/// inside it. Chunks are freed with the same layout they were taken
/// with, once each, when the pool itself dies.
pub struct Blocks {
    count: usize,
    free: Link,
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
    /// Takes one more chunk from `System`, pushes all of its blocks but
    /// the header and the first one, and hands out the first one.
    fn grow(&mut self) -> NonNull<u8> {
        let chunk = NonNull::new(unsafe { System.alloc(self.layout()) })
            .unwrap_or_else(|| handle_alloc_error(self.layout()));
        unsafe { chunk.cast::<Link>().write(self.chunks) };
        self.chunks = Some(chunk);
        for index in 2..self.count {
            unsafe { self.push(chunk.add(index * Self::SIZE)) };
        }
        unsafe { chunk.add(Self::SIZE) }
    }
}

impl Default for Blocks {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl Pool for Blocks {
    const SIZE: usize = 4096;
    fn pop(&mut self) -> NonNull<u8> {
        match self.free {
            Some(block) => {
                self.free = unsafe { block.cast::<Link>().read() };
                block
            }
            None => self.grow(),
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
}
