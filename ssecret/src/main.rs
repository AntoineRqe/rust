use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::{env, process, thread};
use std::fs;
use std::path::PathBuf;
use clap::Parser;


use crate::{analyser::TextAnalysis, config::{Config, Cli}};
use inotify::{Events, Inotify, WatchMask};
use std::io::ErrorKind;

pub mod analyser;
pub mod config;

fn process_event(events: Events<'_>, handles: &mut Vec<JoinHandle<()>>, folder_to_scan: &str) {
    for event in events {
        // Handle event
        let Some(filename) = event.name else {
            continue;
        };

        let full_path = folder_to_scan.to_string() + filename.to_str().unwrap();
        let full_path = fs::canonicalize(full_path).unwrap();

        let handle = thread::spawn(move || {
            let mut my_analyser = TextAnalysis::new(full_path.clone().to_str().unwrap()).unwrap();
            my_analyser.analyse_file().unwrap();
            println!("{0}", my_analyser.build_json());

        });

        handles.push(handle);
    }
}

fn run (config: &Config, stop: &Arc<AtomicBool>) {

    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    rayon::ThreadPoolBuilder::new().num_threads(config.max_thread).build_global().unwrap();
    let pool = rayon_core::ThreadPoolBuilder::default().build().unwrap();


     let mut inotify = Inotify::init()
        .expect("Error while initializing inotify instance");

    // Watch for modify and close events.
    inotify
        .watches()
        .add(
            config.folder_to_scan.clone(),
            WatchMask::CREATE,
        )
        .expect("Failed to add file watch");

    let mut buffer = [0; 1024];

    while !stop.load(Ordering::Relaxed) {

        let events=  {
            match inotify.read_events(&mut buffer) {
                Ok(events) => events,
                Err(error) if error.kind() == (ErrorKind::WouldBlock) => continue,
                _ => panic!("Error while reading events"),
            }
        };

        process_event(events, &mut handles, &config.folder_to_scan);
    };

    for handle in handles {
        handle.join().expect("Failed to stop some threads");
    }
}
 
fn main() {

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    let _ = ctrlc::set_handler( move|| {
        
        println!("Received end of program");
        stop.store(true, Ordering::Relaxed);
    });

    let cli = Cli::parse();
    let config : Config = Config::build_config(cli);

    println!("{config:?}");

    run(&config, &stop_clone);
}
