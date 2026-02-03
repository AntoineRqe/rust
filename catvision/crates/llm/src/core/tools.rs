/// Writes the given list of garbage domains to a file named "garbage_domains_{id}.txt".
/// Each domain is written on a new line.
/// # Arguments
/// * `domains` - A slice of domain strings to write to the garbage file.
/// * `id` - An identifier to distinguish the garbage file.
/// 
pub fn write_domain_in_garbage_file(domains: &Vec<String>, id: usize) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let garbage_file = format!("garbage_domains_{}.txt", id);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&garbage_file)
        .unwrap_or_else(|_| panic!("Unable to open {}", garbage_file));

    for domain in domains {
        writeln!(file, "{}", domain)
            .unwrap_or_else(|_| panic!("Unable to write to {}", garbage_file));
        println!("Written garbage domain '{}' to {}", domain, garbage_file);
    }
}


