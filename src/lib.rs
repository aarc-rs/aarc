pub mod example1 {
    /// The quick brown fox jumps over the lazy dog.
    /// # Examples:
    /// ```
    /// assert!(true);
    /// ```
    pub fn say_hello() {
        println!("hello world!");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test1() {
        assert_eq!(1, 1);
    }
}
