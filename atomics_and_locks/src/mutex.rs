use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering::{Acquire, Release, Relaxed}};
use atomic_wait::{wait, wake_one};

pub struct Mutex<T> {
    /// 0: unlocked
    /// 1: locked, with no waiters
    /// 2: locked, with waiters
    state: AtomicU32,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for Mutex<T> where T: Send {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0), // unlocked state
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        // Try to acquire the lock by setting state from 0 to 1.
        if self.state.compare_exchange(0, 1, Acquire, Relaxed).is_err() {
            // If we failed to acquire the lock, it means it's already locked. We need to handle contention.
            Self::lock_contended(&self.state);
        }
        // At this point, we have acquired the lock (state is 1).
        MutexGuard { mutex: self }
    }

    #[cold]
    fn lock_contended(state: &AtomicU32) {
        let mut spin_count = 0;

        while state.load(Relaxed) == 1 && spin_count < 10 {
            spin_count += 1;
            std::hint::spin_loop();
        }

        if state.compare_exchange(0, 1, Acquire, Relaxed).is_ok() {
            return;
        }

        while state.swap(2, Acquire) != 0 {
            wait(state, 2);
    }
}
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

unsafe impl<T> Send for MutexGuard<'_, T> where T: Send {}
unsafe impl<T> Sync for MutexGuard<'_, T> where T: Sync {}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        // Set the state back to 0: unlocked.
        if self.mutex.state.swap(0, Release) == 2 {
            // If the previous state was 2, it means there are waiters, so we need to wake one of them.
            wake_one(&self.mutex.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_mutex() {
        let mutex = Mutex::new(0);
        let mutex = Arc::new(mutex);

        thread::scope(|s| {
            for _ in 0..10 {
                let mutex = Arc::clone(&mutex);
                s.spawn(move || {
                    for _ in 0..1000 {
                        let mut guard = mutex.lock();
                        *guard += 1;
                    }
                });
            }
        });
        assert_eq!(*mutex.lock(), 10 * 1000);
    }
}