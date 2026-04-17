use std::fs::OpenOptions;
use memmap2::MmapMut;
use spsc::spsc_lock_free::{RingBuffer};

pub struct SharedQueue<const N: usize, T: 'static> {
    pub queue: &'static mut RingBuffer<T, N>,
}

pub fn open_shared_queue<const N: usize, T: 'static>(name: &str, create: bool) -> SharedQueue<N, T> {
    let size = std::mem::size_of::<RingBuffer<T, N>>();
    let path = format!("/dev/shm/{name}");

    let file = if create {
        // Recreate queue storage on startup so we never reopen+truncate an
        // already-mmapped file from another process instance.
        // Truncating a live mapping can trigger SIGBUS/SIGSEGV in readers.
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                panic!("Failed to remove shared queue file '{path}': {e}");
            }
        }

        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) => panic!("Failed to create shared queue file '{path}': {e}"),
        }
    } else {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) => panic!("Failed to open shared queue file '{path}': {e}"),
        }
    };

    if create {
        file.set_len(size as u64).unwrap_or_else(|e| {
            panic!("Failed to set shared queue size for '{path}' to {size} bytes: {e}")
        });
    }
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