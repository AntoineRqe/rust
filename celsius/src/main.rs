fn celsius_to_fahrenheit(temp: f32) -> f32 {
    temp * (9.0/5.0) + 32.0
}

fn fahrenheit_to_celsius(temp: f32) -> f32 {
    (temp - 32.0) * (5.0/9.0)
}

fn main() {
    let c_temp = 30.0;
    let f_temp = 86.0;
    println!("{c_temp}°C = {:?}°F", celsius_to_fahrenheit(c_temp));
    println!("{f_temp}°F = {:?}°F", fahrenheit_to_celsius(f_temp));
}
