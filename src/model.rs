use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

/// Type alias for the completion callback function
pub type CompletionCallback<A> =
    Box<dyn FnOnce(A) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send>> + Send>;

/// The outcome of attempting to start processing a signal
pub enum Outcome<A> {
    /// This is a new process - use the provided callback to mark it complete
    New {
        complete_process: CompletionCallback<A>,
    },
    /// This process was already completed - here's the memoized result
    Duplicate { value: A },
}

impl<A: fmt::Debug> fmt::Debug for Outcome<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::New { .. } => f.debug_struct("New").finish_non_exhaustive(),
            Self::Duplicate { value } => f.debug_struct("Duplicate").field("value", value).finish(),
        }
    }
}

/// Current state of a process
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessStatus<A> {
    /// Process has never been started
    NotStarted,
    /// Process is currently running
    Running,
    /// Process completed successfully with this memoized value
    Completed(A),
    /// Process exceeded maxProcessingTime
    Timeout,
    /// Process record expired (TTL exceeded)
    Expired,
}

/// TTL expiration timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expiration {
    pub instant: SystemTime,
}

impl Expiration {
    pub fn new(instant: SystemTime) -> Self {
        Self { instant }
    }

    pub fn from_duration(duration: Duration) -> Self {
        Self {
            instant: SystemTime::now() + duration,
        }
    }

    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.instant
    }
}

/// Core process record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process<Id, ProcessorId, A> {
    /// Unique signal identifier
    pub id: Id,
    /// Processor instance identifier
    pub processor_id: ProcessorId,
    /// When processing started
    pub started_at: SystemTime,
    /// When processing completed (None = still running)
    pub completed_at: Option<SystemTime>,
    /// When record expires (TTL)
    pub expires_on: Option<Expiration>,
    /// Memoized result value
    pub memoized: Option<A>,
}

impl<Id, ProcessorId, A> Process<Id, ProcessorId, A> {
    pub fn new(id: Id, processor_id: ProcessorId, started_at: SystemTime) -> Self {
        Self {
            id,
            processor_id,
            started_at,
            completed_at: None,
            expires_on: None,
            memoized: None,
        }
    }

    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn is_expired(&self) -> bool {
        self.expires_on.is_some_and(|e| e.is_expired())
    }

    pub fn is_timeout(&self, max_processing_time: Duration) -> bool {
        if self.is_completed() {
            return false;
        }

        SystemTime::now()
            .duration_since(self.started_at)
            .is_ok_and(|elapsed| elapsed >= max_processing_time)
    }

    pub fn status(&self, max_processing_time: Duration) -> ProcessStatus<&A>
    where
        A: Clone,
    {
        if let Some(ref memoized) = self.memoized {
            if self.is_completed() {
                return ProcessStatus::Completed(memoized);
            }
        }

        if self.is_expired() {
            return ProcessStatus::Expired;
        }

        if self.is_timeout(max_processing_time) {
            return ProcessStatus::Timeout;
        }

        ProcessStatus::Running
    }
}

/// Poll strategy for checking in-progress processes
#[derive(Debug, Clone, Copy)]
pub enum PollStrategy {
    /// Poll with linear delay between attempts
    Linear {
        delay: Duration,
        max_duration: Duration,
    },
    /// Poll with exponential backoff
    Backoff {
        base_delay: Duration,
        multiplier: f64,
        max_duration: Duration,
    },
}

impl PollStrategy {
    pub fn linear(delay: Duration, max_duration: Duration) -> Self {
        Self::Linear {
            delay,
            max_duration,
        }
    }

    pub fn backoff(base_delay: Duration, multiplier: f64, max_duration: Duration) -> Self {
        Self::Backoff {
            base_delay,
            multiplier,
            max_duration,
        }
    }

    pub fn max_duration(&self) -> Duration {
        match self {
            Self::Linear { max_duration, .. } => *max_duration,
            Self::Backoff { max_duration, .. } => *max_duration,
        }
    }
}

/// Configuration for Mnemosyne
#[derive(Debug, Clone)]
pub struct Config<ProcessorId> {
    /// Unique identifier for this processor instance
    pub processor_id: ProcessorId,
    /// Maximum time allowed for processing
    pub max_processing_time: Duration,
    /// Time-to-live for records
    pub ttl: Option<Duration>,
    /// How to poll for in-progress processes
    pub poll_strategy: PollStrategy,
}

impl<ProcessorId> Config<ProcessorId> {
    pub fn new(
        processor_id: ProcessorId,
        max_processing_time: Duration,
        ttl: Option<Duration>,
        poll_strategy: PollStrategy,
    ) -> Self {
        Self {
            processor_id,
            max_processing_time,
            ttl,
            poll_strategy,
        }
    }
}
