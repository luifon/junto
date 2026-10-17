//! A global allocator for test programs that counts the bytes that the Rust allocations of
//! the process hold now (ADR 0902). It passes every call to the allocator of the system
//! unchanged and adds or subtracts the size of the block.
//!
//! The footprint of a process also changes when the allocator of the system keeps or
//! returns free pages and when the system compresses pages, by tens of MiB in a long
//! test. The count changes only when the program allocates or frees, so a slow leak of
//! the Rust code shows in it. Memory that native libraries allocate without the Rust
//! allocator is not in it.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: junto_alloc_count::Counting = junto_alloc_count::Counting::new();
//! let held = ALLOCATOR.held();
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The allocator of the system with a count of the bytes of the blocks that are held.
pub struct Counting {
    held: AtomicUsize,
}

impl Counting {
    pub const fn new() -> Self {
        Self {
            held: AtomicUsize::new(0),
        }
    }

    /// The bytes of the blocks that were allocated through this allocator and are not
    /// freed yet.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }
}

impl Default for Counting {
    fn default() -> Self {
        Self::new()
    }
}

// The allocation of a block happens before its release, and all changes of the count are
// on one atomic value, so the count does not fall below zero.
#[allow(unsafe_code)]
// SAFETY: every method passes its arguments to `System`, which implements the contract of
// `GlobalAlloc`, and returns its result unchanged; the count is not used by the allocation.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`, which is the
        // contract of `System::alloc`.
        let block = unsafe { System.alloc(layout) };
        if !block.is_null() {
            self.held.fetch_add(layout.size(), Ordering::Relaxed);
        }
        block
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as in `alloc`.
        let block = unsafe { System.alloc_zeroed(layout) };
        if !block.is_null() {
            self.held.fetch_add(layout.size(), Ordering::Relaxed);
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // SAFETY: the caller passes a block of this allocator with its layout, as
        // `System::dealloc` requires: this allocator got every block from `System`.
        unsafe { System.dealloc(block, layout) };
        self.held.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as in `dealloc`; the caller keeps the contract on `new_size`.
        let moved = unsafe { System.realloc(block, layout, new_size) };
        if !moved.is_null() {
            self.held.fetch_add(new_size, Ordering::Relaxed);
            self.held.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        moved
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn the_count_follows_allocations_reallocations_and_releases() {
        let counting = Counting::new();
        let layout = Layout::from_size_align(1000, 8).unwrap();
        // SAFETY: the layout has a size above zero; the block is released with it below.
        let block = unsafe { counting.alloc(layout) };
        assert!(!block.is_null());
        assert_eq!(counting.held(), 1000);
        // SAFETY: the block came from this allocator with `layout`.
        let block = unsafe { counting.realloc(block, layout, 3000) };
        assert!(!block.is_null());
        assert_eq!(counting.held(), 3000);
        let grown = Layout::from_size_align(3000, 8).unwrap();
        // SAFETY: the block came from this allocator and now has the layout `grown`.
        unsafe { counting.dealloc(block, grown) };
        assert_eq!(counting.held(), 0);
        // SAFETY: as above.
        let zeroed = unsafe { counting.alloc_zeroed(layout) };
        assert!(!zeroed.is_null());
        assert_eq!(counting.held(), 1000);
        // SAFETY: as above.
        unsafe { counting.dealloc(zeroed, layout) };
        assert_eq!(counting.held(), 0);
    }
}
