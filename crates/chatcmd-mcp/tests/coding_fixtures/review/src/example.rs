pub fn ratio(total: u64, count: u64) -> Option<u64> {
    (count != 0).then(|| total / count)
}

