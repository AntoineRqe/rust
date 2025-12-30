use std::sync::atomic::{AtomicBool, Ordering::{Acquire, Relaxed, Release}, fence};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
pub use std::sync::Arc;

pub struct Sender<T> {
    channel: Arc<Channel<T>>,
}

pub struct Receiver<T> {
    channel: Arc<Channel<T>>,
}
struct Channel<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

unsafe impl<T> Sync for Channel<T> where T: Send {}


impl<T> Sender<T> {
    pub fn send(self, value: T) {
        unsafe {
            (*self.channel.message.get()).write(value);
        }
        self.channel.ready.store(true, Release);
    }
}

impl<T> Receiver<T> {
    pub fn is_ready(&self) -> bool {
        self.channel.ready.load(Relaxed)
    }

    pub fn receive(self) -> T {
        if !self.channel.ready.swap(false, Acquire) {
            panic!("Attempted to receive a message before it was ready");
        }
        unsafe { (*self.channel.message.get()).assume_init_read() }
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        if *self.ready.get_mut() {
            unsafe {
                (*self.message.get()).assume_init_drop();
            }
        }
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Arc::new(Channel::new());
    (
        Sender {
            channel: channel.clone(),
        },
        Receiver { channel },
    )
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        Self {
            message: UnsafeCell::new(MaybeUninit::uninit()),
            ready: AtomicBool::new(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn test_channel() {
        let t = thread::current();

        thread::scope(|s| {
            let (sender, receiver) = channel();

            s.spawn(|| {
                sender.send("Hello, world!");
                t.unpark();
            });
        
            while receiver.is_ready() == false {
                thread::park();
            }
            let received = receiver.receive();
            assert_eq!(received, "Hello, world!");
        });
    }
}
