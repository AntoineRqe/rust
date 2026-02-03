use std::thread;


fn test_custom_thread() {
    let handle = thread::Builder::new()
        .name("custom_thread".to_string())
        .stack_size(2 * 1024 * 1024) // 2 MB stack size
        .spawn(|| {
            for i in 1..=3 {
                println!("Custom thread iteration: {}", i);
                thread::sleep(std::time::Duration::from_millis(300));
            }
        })
        .expect("Failed to spawn custom thread");

    handle.join().expect("Custom thread panicked");
}

fn test_scoped_thread() {
    let data = vec![1, 2, 3, 4, 5];

    thread::scope(|s| {
        for &item in &data {
            s.spawn(move || {
                println!("Scoped thread processing item: {}", item);
            });
        }
    });
}


fn test_leaking_thread() {
    let x: &'static [i32; 3] = Box::leak(Box::new([1, 2, 3]));

    thread::spawn(move || println!("{:?}", x)).join().expect("Thread panicked");
    thread::spawn(move || println!("{:?}", x)).join().expect("Thread panicked");
}
fn main() {

    //test_custom_thread();
    //test_scoped_thread();
    test_leaking_thread();
    println!("Hello, world!");
}
