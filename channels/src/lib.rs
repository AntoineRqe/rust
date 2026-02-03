use std::sync::atomic::{AtomicBool, Ordering::{Acquire, Relaxed, Release}};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
pub use std::sync::Arc;

pub struct Sender<'a, T> {
    channel: &'a Channel<T>,
}

pub struct Receiver<'a, T> {
    channel: &'a Channel<T>,
}
pub struct Channel<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

unsafe impl<T> Sync for Channel<T> where T: Send {}


impl<'a, T> Sender<'a, T> {
    pub fn send(self, value: T) {
        unsafe {
            (*self.channel.message.get()).write(value);
        }
        self.channel.ready.store(true, Release);
    }
}

impl<'a, T> Receiver<'a, T> {
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


impl<T> Channel<T> {
    pub fn new() -> Self {
        Self {
            message: UnsafeCell::new(MaybeUninit::uninit()),
            ready: AtomicBool::new(false),
        }
    }

    pub fn split<'a>(&'a mut self) -> (Sender<'a, T>, Receiver<'a, T>) {
        *self = Self::new();
        (Sender { channel: self }, Receiver { channel: self })
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use super::Channel;


    #[test]
    fn test_channel() {
        let t = thread::current();

        let mut channel = Channel::new();

        thread::scope(|s| {
            let (sender, receiver) = channel.split();

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
