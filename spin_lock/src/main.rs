use std::{sync::atomic::{
    AtomicBool,
    Ordering::{Acquire, Relaxed, Release}
}};

use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut, Drop};
use std::thread;

struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>
}

// As guard is a reference to SpinLock, its lifetime is tied to SpinLock
pub struct Guard<'a, T> {
    lock: &'a SpinLock<T> // Need a lifetime parameter to tie Guard's lifetime to SpinLock
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: This Guard means the value is locked so we can get &T
        unsafe { & *self.lock.data.get() }
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: This Guard means the value is locked so we can get &mut T
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Release);
    }
}

// Safety: As data can be accessed by only one thread at a time (thanks to the spin lock), SpinLock<T> is Sync if T is Send
unsafe impl<T> Sync for SpinLock<T> where T: Send {}

unsafe impl<T> Send for Guard<'_, T> where T: Send {}
unsafe impl<T> Sync for Guard<'_, T> where T: Sync {}

impl<T> SpinLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value)
        }
    }

    // Guard is alive as long as &self is alive
    pub fn lock<'a>(&'a self) -> Guard<'a, T> {

        while self.locked.compare_exchange(false, true, Acquire, Relaxed).is_err() {
            std::hint::spin_loop(); // specom CPU pause instruction
        }

        Guard { lock: self }
    }

    /// Safety: Erase the &mut T from lock by yourself
    pub fn unlock(&mut self) {
        self.locked.store(false, Release);
    }
    
}
fn main() {
    let x = SpinLock::new(Vec::new());
    thread::scope(|s| {
        s.spawn(|| x.lock().push(1));
        s.spawn(|| {
            let mut g = x.lock();
            g.push(2);
            g.push(2);
        });
    });

    let g = x.lock();
    assert!(g.as_slice() == [1, 2, 2] || g.as_slice() == [2, 2, 1]);
}


