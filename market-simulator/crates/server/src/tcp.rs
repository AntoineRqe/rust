use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use fix::engine::{FixRawMsg, RequestQueue};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crossbeam_channel::{unbounded};
use types::StopHandle;

pub struct FixServer<const N: usize> {
    fifo_in: RequestQueue<N>,
    shutdown: Arc<AtomicBool>,
}

impl <'a, const N: usize> FixServer<N> {
    pub fn new(fifo_in: RequestQueue<N>) -> Self {
        Self { fifo_in, shutdown: Arc::new(AtomicBool::new(false)) }
    }

    pub fn accept_loop(&self, listener: TcpListener) {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            // Disable Nagle's algorithm for lower latency
            stream.set_nodelay(true).unwrap();

            let queue = Arc::clone(&self.fifo_in);
            let shutdown = Arc::clone(&self.shutdown);

            // Spawn a thread to handle this client connection
            std::thread::spawn(move || {
                Self::handle_client(stream, queue, shutdown);
            });
        }
    }

    pub fn handle_shutdown(&self) -> StopHandle {
        StopHandle { 
            shutdown: Arc::clone(&self.shutdown),
            thread: None,
        }
    }

    fn handle_client(
        mut stream: TcpStream,
        queue:   RequestQueue<N>,
        shutdown: Arc<AtomicBool>,
    ) {

        let (response_tx, response_rx) = unbounded();
        let mut buf = [0u8; 4096];

        while !shutdown.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => {
                    eprintln!("Client disconnected");
                    break
                }, // no message, client closed connection
                Ok(n) => {
                    let mut msg = FixRawMsg::default();
                    msg.len = n as u16;
                    msg.data[..n].copy_from_slice(&buf[..n]);
                    msg.resp_queue = Some(response_tx.clone());

                    while let Err(backup_msg) = queue.push(msg) {
                        msg = backup_msg;
                        std::thread::yield_now(); // backpressure: yield to allow the FIX engine to catch up
                    }

                    loop {
                        if let Ok(response) = response_rx.recv() {
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

