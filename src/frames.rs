//! The stack of open frames, each with its own region.

use crate::pool::Pool;
use crate::region::{Header, Lanes, Region};
use std::ptr::NonNull;

/// The open frames of a running program.
///
/// Every frame is one evaluation of an attribute body (`MEMORY.md` §6)
/// and owns one region; its number is its depth, so a deeper frame is
/// a younger one. Frame 0 is the program region: open from the start
/// and never given back.
pub trait Frames {
    /// Opens a frame one deeper than the youngest one.
    fn enter(&mut self);
    /// Closes the youngest frame and gives its whole region back.
    fn leave(&mut self);
    /// The number of the youngest open frame.
    fn depth(&self) -> usize;
    /// Hands out `words` words of objects in the region of `frame`.
    fn alloc(&mut self, frame: usize, words: usize) -> NonNull<u8>;
    /// Hands out `words` words of bytes in the region of `frame`.
    fn bytes(&mut self, frame: usize, words: usize) -> NonNull<u8>;
    /// The number of the frame whose region holds `address`.
    ///
    /// # Safety
    ///
    /// The address must lie inside memory that `alloc` or `bytes` of
    /// this very stack handed out, in a frame that is still open.
    unsafe fn region_of(&self, address: NonNull<u8>) -> usize;
}

/// A stack of regions that owns the pool they take blocks from.
///
/// Region `n` of the stack is the region of frame `n`:
///
/// ```text
///   regions ─► ┌─────────┐
///              │ frame 0 │ program region, never left
///              ├─────────┤
///              │ frame 1 │ ─► objects lane: block ─► block ─► None
///              ├─────────┤    bytes lane:   block ─► None
///              │ frame 2 │ ◄─ youngest: `leave` closes this one
///              └─────────┘
/// ```
///
/// Any open region may allocate, not only the youngest, so an older
/// region grows by taking a side block while younger frames sit above
/// it (`MEMORY.md` §7). `leave` pops the youngest region and walks both
/// of its lanes, pushing each block back:
///
/// ```text
///   before   frame 2: objects A ─► B ─► None   bytes C ─► None
///   read     A.next = B, push A;  B.next = None, push B
///   read     C.next = None, push C
///   after    free ─► C ─► B ─► A ─► ...        frame 1 is the youngest
/// ```
///
/// `region_of` rounds an address down to the start of its block, by
/// stepping back over its offset inside the block, `address & (4096 -
/// 1)`, and reads the owner word the region stamped there:
///
/// ```text
///   0x7f3a_1000 ─► ┌───────┬──────┬─────────────┬──────────────┐
///                  │ owner │ next │ ...         │ object ...   │
///                  └───────┴──────┴─────────────┴──────▲───────┘
///                  ◄──────── 0xc58 = address & 4095 ───┤
///                                              address 0x7f3a_1c58
/// ```
///
/// Since the stack owns the pool and lends it to nobody else, every
/// region gets its blocks from this pool and gives them back to it,
/// once, when `leave` consumes the region. That is why the `unsafe`
/// release in `leave` is sound. The `unsafe` read in `region_of` is
/// sound under its contract: an address handed out by an open region
/// lies inside a live block of the pool, the block starts at a multiple
/// of the block size, and its first word is the owner the region wrote.
pub struct Stack<P: Pool> {
    pool: P,
    regions: Vec<Lanes>,
}

impl<P: Pool> Stack<P> {
    /// A stack with only frame 0, the program region, open.
    #[must_use]
    pub fn new(pool: P) -> Self {
        Self {
            pool,
            regions: vec![Lanes::new(0)],
        }
    }
}

impl<P: Pool> Frames for Stack<P> {
    fn enter(&mut self) {
        self.regions.push(Lanes::new(self.regions.len()));
    }
    fn leave(&mut self) {
        assert!(
            self.regions.len() > 1,
            "frame 0 is the program region and is never left"
        );
        if let Some(region) = self.regions.pop() {
            unsafe { region.release(&mut self.pool) };
        }
    }
    fn depth(&self) -> usize {
        self.regions.len() - 1
    }
    fn alloc(&mut self, frame: usize, words: usize) -> NonNull<u8> {
        self.regions
            .get_mut(frame)
            .unwrap_or_else(|| panic!("frame {frame} is not open, so it cannot allocate"))
            .alloc(&mut self.pool, words)
    }
    fn bytes(&mut self, frame: usize, words: usize) -> NonNull<u8> {
        self.regions
            .get_mut(frame)
            .unwrap_or_else(|| panic!("frame {frame} is not open, so it cannot allocate"))
            .bytes(&mut self.pool, words)
    }
    unsafe fn region_of(&self, address: NonNull<u8>) -> usize {
        unsafe {
            address
                .sub(address.addr().get() & (P::SIZE - 1))
                .cast::<Header>()
                .read()
        }
        .owner
    }
}

#[cfg(test)]
mod tests {
    use super::{Frames, Stack};
    use crate::pool::{Blocks, Pool};
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::rc::Rc;

    /// The blocks a pool handed out and took back, in order.
    #[derive(Default)]
    struct Log {
        pops: Vec<NonNull<u8>>,
        pushes: Vec<NonNull<u8>>,
    }

    /// A pool that writes every pop and push into a log it shares with
    /// the test, since the stack owns the pool itself.
    struct Logged {
        pool: Blocks,
        log: Rc<RefCell<Log>>,
    }

    unsafe impl Pool for Logged {
        const SIZE: usize = Blocks::SIZE;
        fn pop(&mut self) -> NonNull<u8> {
            let block = self.pool.pop();
            self.log.borrow_mut().pops.push(block);
            block
        }
        unsafe fn push(&mut self, block: NonNull<u8>) {
            self.log.borrow_mut().pushes.push(block);
            unsafe { self.pool.push(block) };
        }
    }

    #[test]
    fn leave_returns_every_block_of_both_lanes() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut stack = Stack::new(Logged {
            pool: Blocks::new(7),
            log: Rc::clone(&log),
        });
        stack.enter();
        for words in [400, 333, 480] {
            stack.alloc(1, words);
        }
        for words in [290, 290, 505, 17] {
            stack.bytes(1, words);
        }
        stack.leave();
        let mut pops = log.borrow().pops.clone();
        let mut pushes = log.borrow().pushes.clone();
        pops.sort_unstable();
        pushes.sort_unstable();
        assert_eq!(
            pushes, pops,
            "leave does not give back exactly the blocks its frame took"
        );
    }

    #[test]
    fn older_region_grows_while_a_younger_frame_is_open() {
        let mut stack = Stack::new(Blocks::new(5));
        (0..3).for_each(|_| stack.enter());
        stack.alloc(1, 470);
        stack.alloc(3, 12);
        let side = stack.alloc(1, 470);
        assert_eq!(
            unsafe { stack.region_of(side) },
            1,
            "a side block of an older region does not belong to the older frame"
        );
    }

    #[test]
    fn region_of_names_the_frame_for_an_address_inside_an_object() {
        let mut stack = Stack::new(Blocks::new(3));
        (0..4).for_each(|_| stack.enter());
        let object = stack.alloc(4, 37);
        assert_eq!(
            unsafe { stack.region_of(object.add(8 * 19 + 3)) },
            4,
            "an address inside an object does not name the frame that allocated it"
        );
    }

    #[test]
    fn frame_numbers_go_up_per_enter_and_down_per_leave() {
        let mut stack = Stack::new(Blocks::new(2));
        (0..6).for_each(|_| stack.enter());
        (0..2).for_each(|_| stack.leave());
        assert_eq!(
            stack.depth(),
            4,
            "six enters and two leaves do not leave frame 4 the youngest"
        );
    }

    #[test]
    fn youngest_frame_allocates_under_its_own_number() {
        let mut stack = Stack::new(Blocks::new(4));
        (0..5).for_each(|_| stack.enter());
        stack.leave();
        let object = stack.bytes(stack.depth(), 9);
        assert_eq!(
            unsafe { stack.region_of(object) },
            4,
            "the youngest frame does not allocate under its own number"
        );
    }

    #[test]
    fn frame_0_keeps_its_blocks_when_every_other_frame_has_left() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut stack = Stack::new(Logged {
            pool: Blocks::new(6),
            log: Rc::clone(&log),
        });
        stack.alloc(0, 450);
        stack.bytes(0, 450);
        (1..4).for_each(|frame| {
            stack.enter();
            stack.alloc(frame, 300);
            stack.bytes(frame, 300);
        });
        (1..4).for_each(|_| stack.leave());
        let program = log.borrow().pops[..2].to_vec();
        assert!(
            !log.borrow()
                .pushes
                .iter()
                .any(|block| program.contains(block)),
            "a block of frame 0 goes back to the pool"
        );
    }
}
