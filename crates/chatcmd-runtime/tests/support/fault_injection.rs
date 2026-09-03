use std::{
    io,
    sync::mpsc::{Receiver, SyncSender},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FaultPoint {
    BeforeTempCreate,
    AfterTempCreate,
    AfterBytesWritten(u64),
    BeforeFileSync,
    AfterFileSync,
    BeforeVersionRecheck,
    BeforeAtomicReplace,
    AfterAtomicReplace,
    BeforeDirectorySync,
    AfterNFilesCopied(u64),
    BeforeDestinationPublish,
    AfterDestinationPublish,
    BeforeSourceDelete,
    DuringRollback,
    BeforeJournalCommit,
    AfterJournalCommit,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum FaultAction {
    Continue,
    Error(io::ErrorKind),
    Cancel,
}

/// A deterministic test-only gate. Production code must receive it only through
/// an internal test seam; callers cannot select fault points through tool input.
pub struct FaultGate {
    point: FaultPoint,
    action: FaultAction,
    reached: SyncSender<FaultPoint>,
    release: Receiver<()>,
}

impl FaultGate {
    pub fn new(
        point: FaultPoint,
        action: FaultAction,
        reached: SyncSender<FaultPoint>,
        release: Receiver<()>,
    ) -> Self {
        Self {
            point,
            action,
            reached,
            release,
        }
    }

    pub fn trigger(&self, point: FaultPoint) -> io::Result<()> {
        if point != self.point {
            return Ok(());
        }
        self.reached.send(point).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "fault observer disconnected")
        })?;
        self.release
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "fault release disconnected"))?;
        match self.action {
            FaultAction::Continue => Ok(()),
            FaultAction::Error(kind) => Err(io::Error::new(kind, "injected test fault")),
            FaultAction::Cancel => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "injected test cancellation",
            )),
        }
    }
}
