use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::cell::UnsafeCell;

#[repr(align(64))]
pub struct CachePadded<T> (T);

#[repr(align(64))]
struct Slot<T>(MaybeUninit<T>);


pub struct RingBuffer<T, const N: usize> {
    buffer: UnsafeCell<[Slot<T>; N]>,    // Circular buffer storage, Make the whole buffer UnsafeCell to allow interior mutability
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
        let mut head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Relaxed);

        while head != tail {
            unsafe {
                self.buffer.get()
                    .as_mut()
                    .unwrap()
                    .get_unchecked_mut(head)
                    .0
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

        let buffer : UnsafeCell<[Slot<T>; N]> = UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() });
    
        Self {
            buffer,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    /// Pushes an item into the ring buffer.
    /// Returns Err(item) if the buffer is full.
    pub fn push(&self, item: T) -> Result<(), T> {
        let head = self.head.0.load(Ordering::Acquire); // Relaxed is safe here because only the consumer modifies head
        let tail = self.tail.0.load(Ordering::Relaxed); // Acquire to synchronize with consumer
        let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2

        if next_tail == head {
            // Buffer is full
            return Err(item);
        }

        // Synchronize with consumer to ensure we see the latest data, improve atomic load performance when buffer is not full
        //fence(Ordering::Acquire);

        unsafe {
             *self.buffer
                .get()
                .as_mut()
                .unwrap()
                .get_unchecked_mut(tail) = Slot(MaybeUninit::new(item));
        }

        self.tail.0.store(next_tail, Ordering::Release);
        Ok(())
    }

    /// Pushes a batch of items into the ring buffer.
    /// Returns the number of items successfully pushed.
    pub fn push_batch(&self, items: &[T]) -> usize 
    where T: Copy
    {
        let mut pushed = 0;
        
        let head = self.head.0.load(Ordering::Acquire); // Relaxed is safe here because only the consumer modifies head
        let mut tail = self.tail.0.load(Ordering::Relaxed); // Acquire to synchronize with consumer
    
        for &item in items {
            let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2

            if next_tail == head {
                // Buffer is full
                break;
            }

            // Synchronize with consumer to ensure we see the latest data, improve atomic load performance when buffer is not full
            //fence(Ordering::Acquire);

            unsafe {
                 *self.buffer
                    .get()
                    .as_mut()
                    .unwrap()
                    .get_unchecked_mut(tail) = Slot(MaybeUninit::new(item));
            }

            tail = next_tail;
            pushed += 1;
        }

        self.tail.0.store(tail, Ordering::Release);
    
        pushed
    }

    /// Pops an item from the ring buffer.
    /// Returns None if the buffer is empty.
    pub fn pop(&self) -> Option<T> {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire); // 
    
        if head == tail {
            // Buffer is empty
            return None;
        }

        // Synchronize with producer to ensure we see the latest data, improve atomic load performance when buffer is not empty
        //fence(Ordering::Acquire);

        let item = unsafe {
            self.buffer.get()
                .as_mut()
                .unwrap()
                .get_unchecked_mut(head)
                .0
                .as_ptr()
                .read()
        };

        let next_head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2
        self.head.0.store(next_head, Ordering::Release);
        Some(item)
    }

    pub fn pop_batch(&self, items: &mut [T]) -> usize 
    where T: Copy
    {
        let mut popped = 0;

        let mut head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);

        while head != tail && popped < items.len() {
            // Synchronize with producer to ensure we see the latest data, improve atomic load performance when buffer is not empty
            //fence(Ordering::Acquire);

            items[popped] = unsafe {
                self.buffer.get()
                    .as_mut()
                    .unwrap()
                    .get_unchecked_mut(head)
                    .0
                    .as_ptr()
                    .read()
            };

            head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2
            popped += 1;
        }

        self.head.0.store(head, Ordering::Release);
        popped
    }

    /// Checks if the ring buffer is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.0.load(Ordering::Acquire);
        let tail = self.tail.0.load(Ordering::Relaxed);
        head == tail
    }

    /// Checks if the ring buffer is full.
    pub fn is_full(&self) -> bool {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2
        next_tail == head
    }

    /// Returns the current number of items in the ring buffer.
    pub fn len(&self) -> usize {
        let head = self.head.0.load(Ordering::Acquire);
        let tail = self.tail.0.load(Ordering::Relaxed);
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