use std::fs::OpenOptions;
use memmap2::MmapMut;
use spsc::spsc_lock_free::{RingBuffer};

pub struct SharedQueue<'a, const N: usize, T> {
    _mmap: MmapMut,  // keeps the mapping alive
    pub queue: &'a mut RingBuffer<T, N>,
}

pub fn open_shared_queue<'a, const N: usize, T>(name: &str, create: bool) -> SharedQueue<'a, N, T> {
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
    let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };

    let queue_ptr = mmap.as_mut_ptr() as *mut RingBuffer<T, N>;

    if create {
        unsafe { RingBuffer::init(queue_ptr) };
    }

    let queue = unsafe { &mut *queue_ptr };

    SharedQueue {
        _mmap: mmap,
        queue,
    }
}