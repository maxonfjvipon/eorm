//! The pool asks the global allocator for nothing, seen from outside.
//!
//! Cargo builds this file as a program of its own, so the counting
//! allocator below is the global allocator of this program only.
#![deny(clippy::pedantic, missing_docs)]

use eorm::pool::{Blocks, Pool};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr::NonNull;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

/// The system allocator, counting every allocation it makes.
///
/// The count is kept per thread, so that what the test harness does on
/// its own threads while a test runs does not show up in the test's
/// count. A `const` thread local holding a `Cell` needs no memory of its
/// own and no destructor, so counting never allocates and works at any
/// point in a thread's life. Every method hands the request straight to
/// `System` with the caller's own arguments, so it is exactly as sound
/// as `System` is.
struct Counted;

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT.set(COUNT.get() + 1);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        COUNT.set(COUNT.get() + 1);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT.set(COUNT.get() + 1);
        unsafe { System.realloc(block, layout, size) }
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        unsafe { System.dealloc(block, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

#[test]
fn popping_and_pushing_across_chunks_allocates_nothing() {
    let mut pool = Blocks::new(7);
    let mut kept = [NonNull::<u8>::dangling(); 23];
    let before = COUNT.get();
    for round in 0..5 {
        for slot in &mut kept {
            *slot = pool.pop();
        }
        for block in kept.iter().step_by(round + 2) {
            unsafe { pool.push(*block) };
        }
    }
    assert_eq!(
        COUNT.get(),
        before,
        "the pool asks the global allocator for memory of its own"
    );
}
