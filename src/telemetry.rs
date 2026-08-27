// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    WrapperStarted,
    JvmLaunching {
        id: u32,
    },
    JvmStarted {
        id: u32,
        pid: u32,
    },
    JvmStopped {
        id: u32,
        exit_code: Option<i32>,
    },
    RestartRequested {
        reason: String,
    },
    FilterMatched {
        index: u32,
    },
    ProtocolAuthenticated,
    ProtocolDisconnected,
    ServicePaused,
    ServiceResumed,
    ThreadDumpStarted {
        jvm_id: u32,
        pid: u32,
        method: String,
    },
    ThreadDumpCompleted {
        jvm_id: u32,
        method: String,
    },
    HeapDumpStarted {
        jvm_id: u32,
        pid: u32,
        path: String,
    },
    HeapDumpCompleted {
        jvm_id: u32,
        path: String,
        bytes: u64,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct Event {
    pub observed_at: SystemTime,
    pub kind: EventKind,
}

impl Event {
    #[must_use]
    pub fn new(kind: EventKind) -> Self {
        Self {
            observed_at: SystemTime::now(),
            kind,
        }
    }
}

/// Event publication is intentionally lossy under backpressure. Telemetry must
/// never be able to block the JVM lifecycle state machine.
#[derive(Clone)]
pub struct EventPublisher {
    sender: crossbeam_channel::Sender<Event>,
}

impl EventPublisher {
    #[must_use]
    pub fn bounded(capacity: usize) -> (Self, crossbeam_channel::Receiver<Event>) {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        (Self { sender }, receiver)
    }

    pub fn publish(&self, kind: EventKind) {
        let _ = self.sender.try_send(Event::new(kind));
    }
}
