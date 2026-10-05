fn main() {
    let mut buffer = itoa::Buffer::new();
    let formatted = buffer.format(42);
    println!("Hello from Mitts fixture: {}", formatted);
}
