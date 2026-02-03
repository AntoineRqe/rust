use std::{cell::UnsafeCell, sync::atomic::{AtomicUsize,
Ordering::{
    Acquire, Relaxed, Release
}, fence}, usize};
use std::ptr::NonNull;
use std::ops::Deref;
use std::mem::ManuallyDrop;

struct ArcData<T> {
    /// Number of Arc's
    data_ref_count: AtomicUsize,
    /// Number of Arc's and Weak's combined
    alloc_ref_count: AtomicUsize,
    /// The data, use None if there is only weak pointer left
    data: UnsafeCell<ManuallyDrop<T>>,
}

pub struct Arc<T> {
    ptr: NonNull<ArcData<T>>
}

unsafe impl<T: Send + Sync> Send for Arc<T> {}
unsafe impl<T: Send + Sync> Sync for Arc<T> {}

pub struct Weak<T> {
    ptr: NonNull<ArcData<T>>
}

unsafe impl<T: Send + Sync> Send for Weak<T> {}
unsafe impl<T: Send + Sync> Sync for Weak<T> {}

impl<T> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &*self.data().data.get()
        }
    }
}

impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        let ref_count = self.data().data_ref_count.load(Relaxed); // Make sure we do not overflow here

        if ref_count < usize::MAX - 1 {
            self.data().alloc_ref_count.fetch_add(1, Relaxed);
            Weak { 
                ptr: self.ptr
            }
        } else {
            panic!("Excedded maximu number of shared references!")
        }
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {

        if self.data().data_ref_count.fetch_add(1, Relaxed) > usize::MAX / 2 {
            panic!("Excedded maximu number of shared references");
        }

        Arc { 
                ptr: self.ptr
        }
    }
}

impl<T> Drop for Weak<T> {
    fn drop(&mut self) {
        if self.data().alloc_ref_count.fetch_sub(1, Release) == 1 {
            fence(Acquire); // Acquire fence allow to synchronize with ref_count
            unsafe {
                drop(Box::from_raw(self.ptr.as_ptr()))
            }
        }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        if self.data().data_ref_count.fetch_sub(1, Release) == 1 {
            fence(Acquire); // Acquire fence allow to synchronize with ref_count
            //Safety : there is nor more Arc becaus data reference is 0
            // It is safe to remove the data
            unsafe {
                ManuallyDrop::drop(&mut *self.data().data.get());
            }
            // Drop the weak pointer representing all Arc
            drop(Weak {ptr: self.ptr})
        }
    }
}

impl<T> Weak<T> {
    fn data(&self) -> &ArcData<T> {
        unsafe { self.ptr.as_ref()}
    }

    pub fn upgrade(&self) -> Option<Arc<T>> {
        let mut n = self.data().data_ref_count.load(Relaxed);
        loop {
            if n == 0 {
                return None;
            }
            assert!(n <= usize::MAX / 2);
            if let Err(e) = 
                self
                .data()
                .data_ref_count
                .compare_exchange_weak(n, n + 1, Relaxed, Relaxed)
            {
                n = e;
                continue;
            }

            return Some(Arc { ptr: self.ptr });
        }

    }
}

impl<T> Arc<T> {
    pub fn new(data: T) -> Arc<T> {
        Arc {
                ptr: NonNull::from(
                    Box::leak(
                        Box::new(
                            ArcData { 
                                data_ref_count: AtomicUsize::new(1),
                                alloc_ref_count: AtomicUsize::new(1),
                                data: UnsafeCell::new(ManuallyDrop::new(data))
                            }
                        )
                    )
                )
            }
    }

    pub fn get_mut(arc: &mut Self) -> Option<&mut T> {
        if arc.data()
        .alloc_ref_count
        .compare_exchange(1, usize::MAX, Acquire, Relaxed)
        .is_err() {
            return None;
        }

        let is_unique = arc.data().data_ref_count.load(Relaxed) == 1;

        arc.data().alloc_ref_count.store(1, Release);

        if !is_unique {
            return None;
        }
    
        fence(Acquire);

        unsafe {
            Some(&mut *arc
                .data()
                .data
                .get()
            )
        } 
    }

    pub fn downgrade(arc: &Self) -> Weak<T> {
        let mut n = arc.data().alloc_ref_count.load(Relaxed);
        loop {
            if n == usize::MAX {
                std::hint::spin_loop();
                n = arc.data().alloc_ref_count.load(Relaxed);
                continue;
            }
            assert!(n <= usize::MAX / 2);
            // Acquire synchronises with get_mut's release-store.
            if let Err(e) =
                arc.data()
                    .alloc_ref_count
                    .compare_exchange_weak(n, n + 1, Acquire, Relaxed)
            {
                n = e;
                continue;
            }
            return Weak { ptr: arc.ptr };
        }
    }

    fn data(&self) -> &ArcData<T> {
        unsafe { self.ptr.as_ref()}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
#[test]
fn test() {
    static NUM_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct DetectDrop;

    impl Drop for DetectDrop {
        fn drop(&mut self) {
            NUM_DROPS.fetch_add(1, Relaxed);
        }
    }

    // Create an Arc with two weak pointers.
    let x = Arc::new(("hello", DetectDrop));
    let y = Arc::downgrade(&x);
    let z = Arc::downgrade(&x);

    let t = std::thread::spawn(move || {
        // Weak pointer should be upgradable at this point.
        let y = y.upgrade().unwrap();
        assert_eq!(y.0, "hello");
    });
    assert_eq!(x.0, "hello");
    t.join().unwrap();

    // The data shouldn't be dropped yet,
    // and the weak pointer should be upgradable.
    assert_eq!(NUM_DROPS.load(Relaxed), 0);
    assert!(z.upgrade().is_some());

    drop(x);

    // Now, the data should be dropped, and the
    // weak pointer should no longer be upgradable.
    assert_eq!(NUM_DROPS.load(Relaxed), 1);
    assert!(z.upgrade().is_none());
}
}