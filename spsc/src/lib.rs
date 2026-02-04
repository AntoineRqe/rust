use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::cell::UnsafeCell;

#[repr(align(64))]
pub struct CachePadded<T> (T);

#[repr(align(64))]
pub struct AlignedBuffer<T, const N: usize> ([MaybeUninit<T>; N]);

pub struct RingBuffer<T, const N: usize> {
    pub head: CachePadded<AtomicUsize>,
    pub tail: CachePadded<AtomicUsize>,
    buffer: UnsafeCell<AlignedBuffer<T, N>>,    // Circular buffer storage, Make the whole buffer UnsafeCell to allow interior mutability
}

/// Split the RingBuffer into a Producer and Consumer. The Producer can only push items, and the Consumer can only pop items.
/// This allows for safe concurrent access from separate threads without needing to use Arc or other synchronization primitives.
pub struct Producer<'a, T, const N: usize> {
    rb: &'a RingBuffer<T, N>,
}

pub struct Consumer<'a, T, const N: usize> {
    rb: &'a RingBuffer<T, N>,
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
                    .0
                    .get_unchecked_mut(head)
                    .assume_init_drop();
            }
            head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2
        }
    }
}

impl <'a, T, const N: usize> Producer<'a, T, N> {
    pub fn push(&self, item: T) -> Result<(), T> {
        self.rb.push(item)
    }

    pub fn push_batch(&self, items: &[T]) -> usize 
    where T: Copy
    {
        self.rb.push_batch(items)
    }    
}

impl <'a, T, const N: usize> Consumer<'a, T, N> {
    pub fn pop(&self) -> Option<T> {
        self.rb.pop()
    }

    pub fn pop_batch(&self, items: &mut [T]) -> usize 
    where T: Copy
    {
        self.rb.pop_batch(items)
    }
}

const SPIN_THRESHOLD: usize = 256;

impl<T, const N: usize> RingBuffer<T, N> {
    /// Creates a new RingBuffer with the specified capacity N.
    /// N must be a power of 2.
    pub fn new() -> Self {
        assert!(N.is_power_of_two(), "N must be a power of 2");

        let buffer : UnsafeCell<AlignedBuffer<T, N>> = UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() });
    
        Self {
            buffer,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn split<'a>(&'a mut self) -> (Producer<'a, T, N>, Consumer<'a, T, N>) {
        *self = Self::new(); // Reset head and tail to 0, ensure buffer is empty
        (Producer { rb: self }, Consumer { rb: self })
    }

    /// Pushes an item into the ring buffer.
    /// Returns Err(item) if the buffer is full.
    pub fn push(&self, item: T) -> Result<(), T> {
        let head = self.head.0.load(Ordering::Relaxed); // Relaxed is safe here because only the consumer modifies head
        let next_head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2

        let tail_relaxed = self.tail.0.load(Ordering::Relaxed); // Acquire to synchronize with consumer

        if next_head != tail_relaxed {
            fence(Ordering::Acquire);
            // Space available, fast path
            unsafe {
                 *self.buffer
                    .get()
                    .as_mut()
                    .unwrap()
                    .0
                    .get_unchecked_mut(head) = MaybeUninit::new(item);
            }

            self.head.0.store(next_head, Ordering::Release);
            return Ok(());
        }
    
        let mut tail = self.tail.0.load(Ordering::Acquire); // Acquire to synchronize with consumer
    
        if next_head == tail {
            // Buffer is full
            let mut spin = 1;

            loop {
                for _ in 0..spin { std::hint::spin_loop(); }

                tail = self.tail.0.load(Ordering::Acquire); // Acquire to synchronize with consumer

                if next_head != tail {
                    // Space available
                    break;
                }

                if spin < SPIN_THRESHOLD {
                    spin *= 2; // backoff
                } else {
                    return Err(item);
                }
            }
            
        }

        // Synchronize with consumer to ensure we see the latest data, improve atomic load performance when buffer is not full
        //fence(Ordering::Acquire);

        unsafe {
             *self.buffer
                .get()
                .as_mut()
                .unwrap()
                .0
                .get_unchecked_mut(head) = MaybeUninit::new(item);
        }

        self.head.0.store(next_head, Ordering::Release);
        Ok(())
    }

    /// Pops an item from the ring buffer.
    /// Returns None if the buffer is empty.
    pub fn pop(&self) -> Option<T> {
        let relaxed_head = self.head.0.load(Ordering::Relaxed); // Acquire to synchronize with producer
        let tail = self.tail.0.load(Ordering::Relaxed); // Relaxed is safe here because only the consumer modifies tail 
    
        if relaxed_head != tail {
            fence(Ordering::Acquire);
            // Data available, fast path
            let item = unsafe {
                self.buffer.get()
                    .as_mut()
                    .unwrap()
                    .0
                    .get_unchecked_mut(tail)
                    .as_ptr()
                    .read()
            };

            let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2
            self.tail.0.store(next_tail, Ordering::Release);
            
            return Some(item);
        }
        
        let mut head = self.head.0.load(Ordering::Acquire); // Acquire to synchronize with producer
        if head == tail {
            let mut spin = 1;

            loop {
                for _ in 0..spin { std::hint::spin_loop(); }

                head = self.head.0.load(Ordering::Acquire); // Acquire to synchronize with producer

                if head != tail {
                    // Data available
                    break;
                }

                if spin < SPIN_THRESHOLD {
                    spin *= 2; // backoff
                } else {
                    return None;
                }
            }
        }


        // Synchronize with producer to ensure we see the latest data, improve atomic load performance when buffer is not empty
        //fence(Ordering::Acquire);

        let item = unsafe {
            self.buffer.get()
                .as_mut()
                .unwrap()
                .0
                .get_unchecked_mut(tail)
                .as_ptr()
                .read()
        };

        let next_tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2
        self.tail.0.store(next_tail, Ordering::Release);
        Some(item)
    }

    /// Pushes a batch of items into the ring buffer.
    /// Returns the number of items successfully pushed.
    pub fn push_batch(&self, items: &[T]) -> usize 
    where T: Copy
    {
        let mut pushed = 0;
        
        let mut head = self.head.0.load(Ordering::Relaxed); // Relaxed is safe here because only the consumer modifies head
        let tail = self.tail.0.load(Ordering::Acquire); // Acquire to synchronize with consumer
    
        for &item in items {
            let next_head = (head + 1) & (N - 1); // Bitwise mask because N is power of 2

            if next_head == tail {
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
                    .0
                    .get_unchecked_mut(head) = MaybeUninit::new(item);
            }

            head = next_head;
            pushed += 1;
        }

        self.head.0.store(head, Ordering::Release);
    
        pushed
    }

    pub fn pop_batch(&self, items: &mut [T]) -> usize 
    where T: Copy
    {
        let mut popped = 0;

        let head = self.head.0.load(Ordering::Relaxed);
        let mut tail = self.tail.0.load(Ordering::Acquire);

        while head != tail && popped < items.len() {
            // Synchronize with producer to ensure we see the latest data, improve atomic load performance when buffer is not empty
            //fence(Ordering::Acquire);

            items[popped] = unsafe {
                self.buffer.get()
                    .as_mut()
                    .unwrap()
                    .0
                    .get_unchecked_mut(tail)
                    .as_ptr()
                    .read()
            };

            tail = (tail + 1) & (N - 1); // Bitwise mask because N is power of 2
            popped += 1;
        }

        self.tail.0.store(tail, Ordering::Release);
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
        let mut rb: RingBuffer<u8, 4> = RingBuffer::new();
        let (producer, consumer) = rb.split();
        assert_eq!(producer.push(1), Ok(()));
        assert_eq!(producer.push(2), Ok(()));
        assert_eq!(producer.push(3), Ok(()));
        assert_eq!(producer.push(4), Err(4)); // Buffer should be full
        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(producer.push(4), Ok(()));
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), Some(4));
        assert_eq!(consumer.pop(), None); // Buffer should be empty
    }

    #[test]
    fn spsc_threads() {
        use std::thread;

        let mut rb = RingBuffer::<usize, 1024>::new();

        thread::scope(|s| {
            let (producer, consumer) = rb.split();

            let prod = s.spawn(move || {
                for i in 0..1_000_000 {
                    loop {
                        if producer.push(i).is_ok() {
                            break;
                        }
                    }
                }
            });

            let cons = s.spawn(move || {
                let mut expected = 0;
                while expected < 1_000_000 {
                    if let Some(v) = consumer.pop() {
                        assert_eq!(v, expected);
                        expected += 1;
                    }
                }
            });

            prod.join().unwrap();
            cons.join().unwrap();
        });
    }

}