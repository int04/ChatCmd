use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, Default)]
pub struct ResourceProbe {
    bytes: Arc<AtomicU64>,
    entries: Arc<AtomicU64>,
    maximum_buffered: Arc<AtomicU64>,
}

impl ResourceProbe {
    pub fn add_bytes(&self, count: u64) {
        self.bytes.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_entry(&self) {
        self.entries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_buffered(&self, count: u64) {
        self.maximum_buffered.fetch_max(count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            bytes: self.bytes.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
            maximum_buffered: self.maximum_buffered.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub bytes: u64,
    pub entries: u64,
    pub maximum_buffered: u64,
}
