//! The memory of one frame: two lanes of blocks, bumped word by word.

use crate::pool::Pool;
use std::ptr::NonNull;

/// The size of one word in bytes.
const WORD: usize = size_of::<u64>();

/// The memory of one frame.
///
/// A frame allocates everything it creates here, objects in one lane
/// and payload bytes in the other, so that all of it can later go back
/// to the pool at once, block by block, when the frame ends.
pub trait Region {
    /// Hands out room for `words` words of objects, word aligned.
    fn alloc(&mut self, pool: &mut impl Pool, words: usize) -> NonNull<u8>;
    /// Hands out room for `words` words of bytes, word aligned.
    fn bytes(&mut self, pool: &mut impl Pool, words: usize) -> NonNull<u8>;
}

/// The first two words of every block a lane takes.
///
/// ```text
///   block ──► ┌───────┬──────┬────────────────────────────────────┐
///             │ owner │ next │ words of the lane ...              │
///             └───────┴──┬───┴────────────────────────────────────┘
///             0       8  │   16                                4096
///                        └──► the lane's previous block
/// ```
///
/// The owner is the number of the frame whose region took the block,
/// so the age of anything inside it is one read away; `next` chains the
/// blocks of one lane, so the whole lane can be walked and given back.
///
/// @todo #9:60min An oversized payload needs a run of blocks and a kind
///  word in the header telling it apart, so that it can be relinked
///  instead of copied (`MEMORY.md` §9 and §13). Until then an oversized
///  request fails fast in `Lane::grow`.
#[repr(C)]
struct Header {
    owner: usize,
    next: Option<NonNull<u8>>,
}

/// A run of blocks that one kind of memory is bumped out of.
///
/// `block` is the current block, `bump` is where the next allocation
/// starts, and `limit` is the end of the current block; each block's
/// header links to the one the lane filled before it:
///
/// ```text
///   block ──► ┌────────┬────────────┬────────┐
///             │ header │ used words │  free  │
///             └───┬────┴────────────┴────────┘
///                 │                 ▲        ▲
///                 │               bump     limit
///                 ▼
///             ┌────────┬─────────────────────┐
///             │ header │ used words          │ ──► ... ──► None
///             └────────┴─────────────────────┘
/// ```
///
/// An allocation of `n` words that fits costs one compare and one add.
/// A fresh lane has no block, and its `bump` and `limit` are the same
/// dangling address, so the first allocation finds no room and takes
/// the slow path, with no special case on the fast one.
///
/// The `unsafe` here is sound because `bump` only moves inside the
/// current block: the fast path adds `n` words only after checking that
/// `n` words fit between `bump` and `limit`, and the slow path points
/// `bump` just past the header and `limit` at the end of a block the
/// pool has just handed out, which the `Pool` contract makes `SIZE`
/// bytes long. The header write lands on the first two words of that
/// fresh block, which is word aligned because blocks are aligned to
/// their size, and which nobody else holds, since the pool gave it to
/// this lane alone.
struct Lane {
    block: Option<NonNull<u8>>,
    bump: NonNull<u8>,
    limit: NonNull<u8>,
}

impl Lane {
    /// A lane that has taken no block yet.
    fn new() -> Self {
        Self {
            block: None,
            bump: NonNull::dangling(),
            limit: NonNull::dangling(),
        }
    }
    /// Hands out `words` words, stamping any new block with the owner.
    fn alloc(&mut self, pool: &mut impl Pool, words: usize, owner: usize) -> NonNull<u8> {
        if words > (self.limit.addr().get() - self.bump.addr().get()) / WORD {
            self.grow(pool, words, owner);
        }
        let object = self.bump;
        self.bump = unsafe { object.add(words * WORD) };
        object
    }
    /// Takes a fresh block from the pool, stamps its header and makes it
    /// the current block of the lane.
    fn grow<P: Pool>(&mut self, pool: &mut P, words: usize, owner: usize) {
        assert!(
            words <= (P::SIZE - size_of::<Header>()) / WORD,
            "a request of {words} words does not fit in an empty block of {} bytes",
            P::SIZE
        );
        let block = pool.pop();
        unsafe {
            block.cast::<Header>().write(Header {
                owner,
                next: self.block,
            });
        };
        self.block = Some(block);
        self.bump = unsafe { block.add(size_of::<Header>()) };
        self.limit = unsafe { block.add(P::SIZE) };
    }
}

/// A region made of an objects lane and a bytes lane.
///
/// Both lanes stamp every block they take with the number of the frame
/// the region belongs to. The region does not own the pool: whoever
/// owns the frames owns the pool and lends it to each allocation, so a
/// region can never outlive the memory it bumps through.
pub struct Lanes {
    number: usize,
    objects: Lane,
    bytes: Lane,
}

impl Lanes {
    /// The region of the frame with the given number.
    #[must_use]
    pub fn new(number: usize) -> Self {
        Self {
            number,
            objects: Lane::new(),
            bytes: Lane::new(),
        }
    }
}

impl Region for Lanes {
    fn alloc(&mut self, pool: &mut impl Pool, words: usize) -> NonNull<u8> {
        self.objects.alloc(pool, words, self.number)
    }
    fn bytes(&mut self, pool: &mut impl Pool, words: usize) -> NonNull<u8> {
        self.bytes.alloc(pool, words, self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::{Header, Lanes, Region};
    use crate::pool::{Blocks, Pool};
    use std::ptr::NonNull;

    /// A pool that remembers every block it hands out, so a test can
    /// look at the header a region wrote there.
    struct Traced {
        pool: Blocks,
        blocks: Vec<NonNull<u8>>,
    }

    unsafe impl Pool for Traced {
        const SIZE: usize = Blocks::SIZE;
        fn pop(&mut self) -> NonNull<u8> {
            let block = self.pool.pop();
            self.blocks.push(block);
            block
        }
        unsafe fn push(&mut self, block: NonNull<u8>) {
            unsafe { self.pool.push(block) };
        }
    }

    #[test]
    fn allocation_that_fits_only_moves_the_bump_pointer() {
        let mut pool = Blocks::new(5);
        let mut region = Lanes::new(7);
        let first = region.alloc(&mut pool, 3).addr().get();
        assert_eq!(
            region.alloc(&mut pool, 11).addr().get(),
            first + 3 * 8,
            "the next allocation does not start right after the previous one"
        );
    }

    #[test]
    fn allocation_that_fills_a_block_exactly_stays_in_it() {
        let mut pool = Traced {
            pool: Blocks::new(6),
            blocks: Vec::new(),
        };
        let mut region = Lanes::new(4);
        region.alloc(&mut pool, 300);
        region.alloc(&mut pool, 210);
        assert_eq!(
            pool.blocks.len(),
            1,
            "an allocation that fits the rest of a block takes a new block"
        );
    }

    #[test]
    fn allocation_that_does_not_fit_takes_a_new_block() {
        let mut pool = Traced {
            pool: Blocks::new(6),
            blocks: Vec::new(),
        };
        let mut region = Lanes::new(4);
        region.alloc(&mut pool, 300);
        region.alloc(&mut pool, 211);
        assert_eq!(
            pool.blocks.len(),
            2,
            "an allocation that does not fit does not take a new block"
        );
    }

    #[test]
    fn owner_word_of_a_taken_block_is_the_frame_number() {
        let mut pool = Traced {
            pool: Blocks::new(3),
            blocks: Vec::new(),
        };
        Lanes::new(23).alloc(&mut pool, 5);
        assert_eq!(
            unsafe { pool.blocks[0].cast::<Header>().read() }.owner,
            23,
            "the owner word of a taken block is not the frame number"
        );
    }

    #[test]
    fn two_lanes_never_share_a_block() {
        let mut pool = Traced {
            pool: Blocks::new(11),
            blocks: Vec::new(),
        };
        let mut region = Lanes::new(2);
        region.alloc(&mut pool, 3);
        region.bytes(&mut pool, 5);
        assert_eq!(
            pool.blocks.len(),
            2,
            "the bytes lane allocates in the block of the objects lane"
        );
    }

    #[test]
    fn new_block_links_to_the_lanes_previous_block() {
        let mut pool = Traced {
            pool: Blocks::new(4),
            blocks: Vec::new(),
        };
        let mut region = Lanes::new(13);
        region.bytes(&mut pool, 400);
        region.bytes(&mut pool, 401);
        assert_eq!(
            unsafe { pool.blocks[1].cast::<Header>().read() }.next,
            Some(pool.blocks[0]),
            "a new block does not link to the lane's previous block"
        );
    }
}
