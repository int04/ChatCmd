pub fn checked_double(value: i32) -> Option<i32> {
    value.checked_mul(2)
}

#[cfg(test)]
mod tests {
    use super::checked_double;

    #[test]
    fn doubles_small_values() {
        assert_eq!(checked_double(21), Some(42));
    }
}

