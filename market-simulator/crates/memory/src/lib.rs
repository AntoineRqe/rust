use std::fs::OpenOptions;
use memmap2::MmapMut;
use spsc::spsc_lock_free::{RingBuffer};
use order_book::types::OrderEvent;

pub struct SharedQueue<const N: usize> {
    _mmap: MmapMut,  // keeps the mapping alive
    pub queue: &'static mut RingBuffer<OrderEvent, N>,
}

pub fn open_shared_queue<const N: usize>(name: &str, create: bool) -> SharedQueue<N> {
    let size = std::mem::size_of::<RingBuffer<OrderEvent, N>>();

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
    let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };

    let queue_ptr = mmap.as_mut_ptr() as *mut RingBuffer<OrderEvent, N>;

    if create {
        unsafe { RingBuffer::init(queue_ptr) };
    }

    let queue = unsafe { &mut *queue_ptr };

    SharedQueue {
        _mmap: mmap,
        queue,
    }
}