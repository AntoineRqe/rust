use std::sync::Mutex;
use std::collections::VecDeque;

pub struct SpscLock<T, const N: usize> {
    buffer: Mutex<VecDeque<T>>,
}

impl<T, const N: usize> SpscLock<T, N> {
    pub fn new() -> Self {
        SpscLock {
            buffer: Mutex::new(VecDeque::with_capacity(N)),
        }
    }

    pub fn push(&self, item: T) -> Result<(), T> {
        let mut buf = self.buffer.lock().unwrap();
        if buf.len() < N {
            buf.push_back(item);
            Ok(())
        } else {
            Err(item)
        }
    }

    pub fn pop(&self) -> Option<T> {
        let mut buf = self.buffer.lock().unwrap();
        buf.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

        #[test]
    fn spsc_threads() {
        use std::thread;
        use std::sync::Arc;

        let rb = Arc::new(SpscLock::<usize, 1024>::new());
        let rb_producer = Arc::clone(&rb);
        let rb_consumer = Arc::clone(&rb);

        thread::scope(|s| {

            let prod = s.spawn(move || {
                for i in 0..1_000_000 {
                    loop {
                        if rb_producer.push(i).is_ok() {
                            break;
                        }
                    }
                }
            });

            let mut expected = 0;
            while expected < 1_000_000 {
                if let Some(v) = rb_consumer.pop() {
                    assert_eq!(v, expected);
                    expected += 1;
                }
            }

            prod.join().unwrap();
        });
    }
}