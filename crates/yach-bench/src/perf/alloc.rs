use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct Counting;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

fn usize_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

// SAFETY: delegates every call to `System`; only adds relaxed counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(usize_u64(layout.size()), Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(
                usize_u64(new_size.saturating_sub(layout.size())),
                Ordering::Relaxed,
            );
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocCounts {
    pub count: u64,
    pub bytes: u64,
}

pub struct AllocWindow {
    count0: u64,
    bytes0: u64,
}

impl AllocWindow {
    #[must_use]
    pub fn begin() -> Self {
        let window = Self {
            count0: COUNT.load(Ordering::Relaxed),
            bytes0: BYTES.load(Ordering::Relaxed),
        };
        ACTIVE.store(true, Ordering::SeqCst);
        window
    }

    #[must_use]
    pub fn end(self) -> AllocCounts {
        ACTIVE.store(false, Ordering::SeqCst);
        AllocCounts {
            count: COUNT.load(Ordering::Relaxed).saturating_sub(self.count0),
            bytes: BYTES.load(Ordering::Relaxed).saturating_sub(self.bytes0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AllocWindow;

    #[test]
    fn window_counts_only_allocations_inside_it() {
        let _outside = vec![0u8; 4096];
        let window = AllocWindow::begin();
        let inside = vec![0u8; 8192];
        let counts = window.end();
        drop(inside);
        assert!(counts.count >= 1);
        assert!(counts.bytes >= 8192, "bytes={}", counts.bytes);
        assert!(counts.bytes < 8192 + 1024, "bytes={}", counts.bytes);
    }
}
