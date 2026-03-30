use std::fs::OpenOptions;
use memmap2::MmapMut;
use spsc::spsc_lock_free::{RingBuffer};

pub struct SharedQueue<const N: usize, T: 'static> {
    pub queue: &'static mut RingBuffer<T, N>,
}

pub fn open_shared_queue<const N: usize, T: 'static>(name: &str, create: bool) -> SharedQueue<N, T> {
    let size = std::mem::size_of::<RingBuffer<T, N>>();

    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .open(format!("/dev/shm/{}", name))  // /dev/shm is RAM-backed on Linux
    {
        Ok(file) => file,
        Err(e) => panic!("Failed to open shared queue file: {}", e),
    };

    file.set_len(size as u64).unwrap();
    let mmap = unsafe { MmapMut::map_mut(&file).unwrap() };

    // Leak the mapping so the backing memory lives for the entire process.
    // These queues are created once at startup and are never freed, so the
    // leak is intentional and the OS will reclaim the memory on exit.
    let mmap: &'static mut MmapMut = Box::leak(Box::new(mmap));

    let queue_ptr = mmap.as_mut_ptr() as *mut RingBuffer<T, N>;

    if create {
        unsafe { RingBuffer::init(queue_ptr) };
    }

    let queue: &'static mut RingBuffer<T, N> = unsafe { &mut *queue_ptr };

    SharedQueue { queue }
}