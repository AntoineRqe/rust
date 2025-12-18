use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::cell::UnsafeCell;
use crossbeam_utils::CachePadded;

pub struct RingBuffer<T, const N: usize> {
    buffer: UnsafeCell<[MaybeUninit<T>; N]>,    // Circular buffer storage, Make the whole buffer UnsafeCell to allow interior mutability
    pub head: CachePadded<AtomicUsize>,
    pub tail: CachePadded<AtomicUsize>,
}

// Safety: The RingBuffer can be safely sent between threads as long as T is Send
unsafe impl<T: Send, const N: usize> Send for RingBuffer<T, N> {}
// Safety: The RingBuffer can be safely shared between threads as long as T is Send
unsafe impl<T: Send, const N: usize> Sync for RingBuffer<T, N> {}

// Need to properly drop any remaining items in the buffer when RingBuffer is dropped
impl<T, const N: usize> Drop for RingBuffer<T, N> {
    fn drop(&mut self) {
        let mut head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        while head != tail {
            unsafe {
                self.buffer.get()
                    .as_mut()
                    .unwrap()
                    .get_unchecked_mut(head)
                    .assume_init_drop();
            }
            head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2
        }
    }
}

impl<T, const N: usize> RingBuffer<T, N> {
    /// Creates a new RingBuffer with the specified capacity N.
    /// N must be a power of 2.
    pub fn new() -> Self {
        assert!(N.is_power_of_two(), "N must be a power of 2");

        let buffer : UnsafeCell<[MaybeUninit<T>; N]> = UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() });
    
        Self {
            buffer,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        }
    }

    /// Pushes an item into the ring buffer.
    /// Returns Err(item) if the buffer is full.
    pub fn push(&self, item: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2

        if next_tail == head {
            // Buffer is full
            return Err(item);
        }

        unsafe {
             *self.buffer
                .get()
                .as_mut()
                .unwrap()
                .get_unchecked_mut(tail) = MaybeUninit::new(item);
        }

        self.tail.store(next_tail, Ordering::Release);
        Ok(())
    }

    /// Pops an item from the ring buffer.
    /// Returns None if the buffer is empty.
    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        if head == tail {
            // Buffer is empty
            return None;
        }

        let item = unsafe {
            self.buffer.get()
                .as_mut()
                .unwrap()
                .get_unchecked_mut(head)
                .as_ptr()
                .read()
        };

        let next_head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2
        self.head.store(next_head, Ordering::Release);
        Some(item)
    }

    /// Checks if the ring buffer is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        head == tail
    }

    /// Checks if the ring buffer is full.
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2
        next_tail == head
    }

    /// Returns the current number of items in the ring buffer.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        (tail + N - head) & (N - 1) // Bitwise mask because N is power of 2
    }
}


#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn it_works() {
        let _rb: RingBuffer<u8, 1024> = RingBuffer::new();
    }

    #[test]
    fn push_and_pop() {
        let rb: RingBuffer<u8, 4> = RingBuffer::new();
        assert_eq!(rb.push(1), Ok(()));
        assert_eq!(rb.push(2), Ok(()));
        assert_eq!(rb.push(3), Ok(()));
        assert_eq!(rb.push(4), Err(4)); // Buffer should be full
        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert_eq!(rb.push(4), Ok(()));
        assert_eq!(rb.pop(), Some(3));
        assert_eq!(rb.pop(), Some(4));
        assert_eq!(rb.pop(), None); // Buffer should be empty
    }

    #[test]
    fn is_empty_and_full() {
        let rb: RingBuffer<u8, 2> = RingBuffer::new();
        assert!(rb.is_empty());
        assert!(!rb.is_full());
        rb.push(1).unwrap();
        assert!(rb.len() == 1);
        assert!(!rb.is_empty());
        assert!(rb.is_full());
        rb.pop().unwrap();
        assert!(rb.is_empty());
    }

    #[test]
    fn spsc_threads() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(RingBuffer::<usize, 1024>::new());

        let prod = {
            let rb = rb.clone();
            thread::spawn(move || {
                for i in 0..1_000_000 {
                    loop {
                        if rb.push(i).is_ok() {
                            break;
                        }
                    }
                }
            })
        };

        let cons = {
            let rb = rb.clone();
            thread::spawn(move || {
                let mut expected = 0;
                while expected < 1_000_000 {
                    if let Some(v) = rb.pop() {
                        assert_eq!(v, expected);
                        expected += 1;
                    }
                }
            })
        };

        prod.join().unwrap();
        cons.join().unwrap();
    }

}