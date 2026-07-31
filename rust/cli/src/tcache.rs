//! Thread-caching global allocator.
//!
//! The portable dist binary links musl statically, and musl's malloc
//! takes ONE global lock: on MAIN09 every thread count from 1 to 24
//! finished in the same ~190 s (per-tile time inflated linearly with
//! the worker count - total throughput pinned to a single core),
//! while the same build against glibc ran 60 s. Tiling's hot
//! allocations are small vectors that are born and die within one
//! tile on one thread, so a per-thread free-list cache in front of
//! the system allocator removes the lock from the hot path entirely,
//! on every libc.
//!
//! Design: size classes 16..4096 (power of two), align <= 16.
//! alloc: pop the thread-local list, else System with the CLASS
//! layout. dealloc: push to the thread-local list up to a per-class
//! byte cap, else System. Every small block is created and released
//! with its class layout, so the pair always matches. Larger or
//! over-aligned requests pass straight through to System. Caches
//! drain to System on thread exit; teardown-phase frees fall back to
//! System via try_with.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const NCLASS: usize = 12; // 16, 32, ... 32768
const MAX_SMALL: usize = 32_768;
const MAX_ALIGN: usize = 16;
/// retained-bytes bound per class per thread (steady state reuse
/// needs far less; this only caps the high-water mark). 12 classes
/// x 4 MB = <=48 MB per thread worst case - vector growth up to
/// 32 KB stays off the libc lock, which musl holds globally.
const CAP_BYTES: usize = 4 << 20;

#[inline]
fn class_of(size: usize) -> usize {
    (size.max(16).next_power_of_two().trailing_zeros() - 4) as usize
}

#[inline]
const fn class_size(c: usize) -> usize {
    16 << c
}

#[inline]
fn class_layout(c: usize) -> Layout {
    // size is a power of two >= 16, align 16: always valid
    unsafe { Layout::from_size_align_unchecked(class_size(c), MAX_ALIGN) }
}

struct Cache {
    heads: [Cell<*mut u8>; NCLASS],
    counts: [Cell<usize>; NCLASS],
}

impl Drop for Cache {
    fn drop(&mut self) {
        for c in 0..NCLASS {
            let mut p = self.heads[c].get();
            while !p.is_null() {
                let next = unsafe { *(p as *mut *mut u8) };
                unsafe { System.dealloc(p, class_layout(c)) };
                p = next;
            }
            self.heads[c].set(std::ptr::null_mut());
        }
    }
}

thread_local! {
    static CACHE: Cache = const {
        Cache {
            heads: [const { Cell::new(std::ptr::null_mut()) }; NCLASS],
            counts: [const { Cell::new(0) }; NCLASS],
        }
    };
}

pub struct TCache;

unsafe impl GlobalAlloc for TCache {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() <= MAX_SMALL && l.align() <= MAX_ALIGN {
            let c = class_of(l.size());
            let hit = CACHE
                .try_with(|t| {
                    let head = t.heads[c].get();
                    if head.is_null() {
                        std::ptr::null_mut()
                    } else {
                        t.heads[c].set(*(head as *mut *mut u8));
                        t.counts[c].set(t.counts[c].get() - 1);
                        head
                    }
                })
                .unwrap_or(std::ptr::null_mut());
            if !hit.is_null() {
                return hit;
            }
            return System.alloc(class_layout(c));
        }
        System.alloc(l)
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() <= MAX_SMALL && l.align() <= MAX_ALIGN {
            let c = class_of(l.size());
            let cached = CACHE
                .try_with(|t| {
                    if t.counts[c].get() >= CAP_BYTES / class_size(c) {
                        return false;
                    }
                    *(p as *mut *mut u8) = t.heads[c].get();
                    t.heads[c].set(p);
                    t.counts[c].set(t.counts[c].get() + 1);
                    true
                })
                .unwrap_or(false);
            if !cached {
                System.dealloc(p, class_layout(c));
            }
            return;
        }
        System.dealloc(p, l)
    }
}
