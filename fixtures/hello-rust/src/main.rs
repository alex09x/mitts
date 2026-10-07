fn main() {
    let mut buffer = itoa::Buffer::new();
    let formatted = buffer.format(42);
    println!("Hello from Mitts fixture: {}", formatted);
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_answer() {
        assert_eq!(42, 42);
    }
}
