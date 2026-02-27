use std::net::{TcpListener, TcpStream};
use std::io::Read;
use protocol::fix_engine::{FixRawMsg};
use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FixServer<const N: usize> {
    fifo_in: Arc<ArrayQueue<FixRawMsg<N>>>,
    running: Arc<AtomicBool>,
}

impl <'a, const N: usize> FixServer<N> {
    pub fn new(fifo_in: Arc<ArrayQueue<FixRawMsg<N>>>) -> Self {
        Self { fifo_in, running: Arc::new(AtomicBool::new(true)) }
    }

    pub fn accept_loop(&self, listener: TcpListener) {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            // Disable Nagle's algorithm for lower latency
            stream.set_nodelay(true).unwrap();

            let queue = Arc::clone(&self.fifo_in);
            let running = Arc::clone(&self.running);

            // Spawn a thread to handle this client connection
            std::thread::spawn(move || {
                Self::handle_client(stream, queue, running);
            });
        }
    }

    fn handle_client(
        mut stream: TcpStream,
        queue:   Arc<ArrayQueue<FixRawMsg<N>>>,
        running: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];

        while running.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => break, // no message, client closed connection
                Ok(n) => {
                    println!("Received {} bytes from client", n);
                    let mut msg = FixRawMsg::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);

                    while queue.push(msg).is_err() {
                        std::hint::spin_loop(); // engine is behind, spin
                    }
                }
                Err(_) => break,
            }
        }
    }
}

