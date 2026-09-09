use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Counting;

thread_local! {
    static IN_WINDOW: Cell<bool> = const { Cell::new(false) };
}

static COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) static WINDOW_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_window_for_test() -> std::sync::MutexGuard<'static, ()> {
    match WINDOW_TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn counting() -> bool {
    IN_WINDOW.try_with(Cell::get).unwrap_or(false)
}


fn usize_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

// SAFETY: delegates every call to `System`; only adds relaxed counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
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
        if counting() {
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
        let _ = IN_WINDOW.try_with(|flag| flag.set(true));
        window
    }

    #[must_use]
    pub fn end(self) -> AllocCounts {
        let _ = IN_WINDOW.try_with(|flag| flag.set(false));
        AllocCounts {
            count: COUNT.load(Ordering::Relaxed).saturating_sub(self.count0),
            bytes: BYTES.load(Ordering::Relaxed).saturating_sub(self.bytes0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{lock_window_for_test, AllocWindow};

    // COUNT/BYTES are process-global. Production opens one window on the
    // worker main thread; these tests must not overlap each other or the
    // worker measure tests, or the snapshots double-count. Other tests
    // allocate freely — IN_WINDOW keeps them out.

    #[test]
    fn window_counts_only_allocations_inside_it() {
        let _guard = lock_window_for_test();
        let _outside = vec![0u8; 4096];

        let window = AllocWindow::begin();
        let inside = vec![0u8; 8192];
        let counts = window.end();
        drop(inside);
        assert!(counts.count >= 1);
        assert!(counts.bytes >= 8192, "bytes={}", counts.bytes);
        assert!(counts.bytes < 8192 + 1024, "bytes={}", counts.bytes);
    }

    #[test]
    fn window_ignores_allocations_on_other_threads() {
        let _guard = lock_window_for_test();

        let _outside = vec![0u8; 4096];
        let window = AllocWindow::begin();
        let inside = vec![0u8; 8192];
        let spawned = std::thread::spawn(|| {
            let _other = vec![0u8; 1 << 20];
        });
        let joined = spawned.join();
        assert!(joined.is_ok(), "helper thread panicked");
        let counts = window.end();
        drop(inside);
        assert!(counts.count >= 1);
        assert!(counts.bytes >= 8192, "bytes={}", counts.bytes);
        assert!(counts.bytes < 8192 + 1024, "bytes={}", counts.bytes);
    }
}
