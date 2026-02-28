use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use fix::engine::{FixRawMsg, RequestQueue, ResponseQueue};
use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FixServer<const N: usize> {
    fifo_in: RequestQueue<N>,
    running: Arc<AtomicBool>,
}

impl <'a, const N: usize> FixServer<N> {
    pub fn new(fifo_in: RequestQueue<N>) -> Self {
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
        queue:   RequestQueue<N>,
        running: Arc<AtomicBool>,
    ) {
        let response_queue: ResponseQueue<N> = Arc::new(ArrayQueue::new(1024));
        let mut buf = [0u8; 4096];

        while running.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => break, // no message, client closed connection
                Ok(n) => {
                    println!("Received {} bytes from client", n);
                    let mut msg = FixRawMsg::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.queue = Some(Arc::clone(&response_queue));

                    while let Err(backup_msg) = queue.push(msg) {
                        msg = backup_msg;
                        std::thread::yield_now(); // backpressure: yield to allow the FIX engine to catch up
                    }

                    loop {
                        if let Some(response) = response_queue.pop() {
                            println!("Sending response of {} bytes to client", response.len);
                            if let Err(e) = stream.write_all(&response.data[..response.len as usize]) {
                                eprintln!("Failed to send response to client: {}", e);
                                break;
                            }
                            break; // response sent, go back to reading from the client
                        } else {
                            continue; // no more responses, go back to reading from the client
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }
}

