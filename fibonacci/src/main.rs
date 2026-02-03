fn basic_fibonacci(iteration: i32) -> i32 {
    let mut counter = 0;
    let mut fibonacci_number = 1;
    let mut fibonacci_number_n1 = 1;
    let mut fibonacci_number_n2 = 0;

    loop {
        if counter > iteration {
            break fibonacci_number;
        } else if counter == 0 {
            fibonacci_number = 1;
            fibonacci_number_n1 = 1;
            fibonacci_number_n2 = 1;
        } else if counter == 1 {
            let old_fibonacci_number = fibonacci_number;
            fibonacci_number = fibonacci_number_n1 + fibonacci_number_n2;
            fibonacci_number_n1 = old_fibonacci_number;
            fibonacci_number_n2 = 1;
        } else {
            let old_fibonacci_number = fibonacci_number;
            fibonacci_number_n2 = fibonacci_number_n1;
            fibonacci_number_n1 = old_fibonacci_number;
            fibonacci_number = fibonacci_number_n1 + fibonacci_number_n2;
        }

        println!("[{counter}] {fibonacci_number} = {fibonacci_number_n1} + {fibonacci_number_n2}");
        counter += 1;
    }
}

fn naive_recursive_fibonacci(iteration: i32) -> i32 {
    if iteration == 0 {
        1
    } else if iteration ==1 {
        2
    } else {
        naive_recursive_fibonacci(iteration -1) + naive_recursive_fibonacci(iteration - 2)
    }
}

fn polynomial_fibonacci(iteration: i32, a:i32, b: i32) -> i32 {
    if iteration == 0 {
        a
    } else if iteration == 1 {
        b
    } else {
        polynomial_fibonacci(iteration -1, b, a + b)
    }
}

fn main() {
    let iteration = 10;

    println!("{iteration}th : {:?}", basic_fibonacci(iteration));
    println!("{iteration}th : {:?}", naive_recursive_fibonacci(iteration));
    println!("{iteration}th : {:?}", polynomial_fibonacci(iteration+2, 0, 1));
}
