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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expiration_is_expired_past() {
        let past_time = SystemTime::now() - Duration::from_secs(10);
        let expiration = Expiration::new(past_time);
        assert!(expiration.is_expired());
    }

    #[test]
    fn test_expiration_is_expired_future() {
        let future_time = SystemTime::now() + Duration::from_secs(10);
        let expiration = Expiration::new(future_time);
        assert!(!expiration.is_expired());
    }

    #[test]
    fn test_expiration_from_duration() {
        let expiration = Expiration::from_duration(Duration::from_secs(1));
        // Should not be expired immediately
        assert!(!expiration.is_expired());
    }

    #[test]
    fn test_process_is_completed() {
        let process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        assert!(!process.is_completed());

        let mut completed_process = process.clone();
        completed_process.completed_at = Some(SystemTime::now());
        assert!(completed_process.is_completed());
    }

    #[test]
    fn test_process_is_expired_no_expiration() {
        let process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        assert!(!process.is_expired());
    }

    #[test]
    fn test_process_is_expired_past() {
        let mut process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        let past_time = SystemTime::now() - Duration::from_secs(10);
        process.expires_on = Some(Expiration::new(past_time));
        assert!(process.is_expired());
    }

    #[test]
    fn test_process_is_expired_future() {
        let mut process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        let future_time = SystemTime::now() + Duration::from_secs(10);
        process.expires_on = Some(Expiration::new(future_time));
        assert!(!process.is_expired());
    }

    #[test]
    fn test_process_is_timeout_completed() {
        let mut process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        process.completed_at = Some(SystemTime::now());
        // Completed processes should never timeout
        assert!(!process.is_timeout(Duration::from_secs(0)));
    }

    #[test]
    fn test_process_is_timeout_not_exceeded() {
        let process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        let max_processing_time = Duration::from_secs(10);
        assert!(!process.is_timeout(max_processing_time));
    }

    #[test]
    fn test_process_is_timeout_exceeded() {
        let past_time = SystemTime::now() - Duration::from_secs(20);
        let process: Process<&str, &str, String> = Process::new("id", "processor", past_time);
        let max_processing_time = Duration::from_secs(10);
        assert!(process.is_timeout(max_processing_time));
    }

    #[test]
    fn test_process_status_running() {
        let process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        let status = process.status(Duration::from_secs(60));
        assert_eq!(status, ProcessStatus::Running);
    }

    #[test]
    fn test_process_status_completed() {
        let mut process = Process::new("id", "processor", SystemTime::now());
        process.completed_at = Some(SystemTime::now());
        process.memoized = Some("result".to_string());

        let status = process.status(Duration::from_secs(60));
        match status {
            ProcessStatus::Completed(value) => assert_eq!(*value, "result"),
            _ => panic!("Expected Completed status"),
        }
    }

    #[test]
    fn test_process_status_expired() {
        let mut process: Process<&str, &str, String> =
            Process::new("id", "processor", SystemTime::now());
        let past_time = SystemTime::now() - Duration::from_secs(10);
        process.expires_on = Some(Expiration::new(past_time));

        let status = process.status(Duration::from_secs(60));
        assert_eq!(status, ProcessStatus::Expired);
    }

    #[test]
    fn test_process_status_timeout() {
        let past_time = SystemTime::now() - Duration::from_secs(20);
        let process: Process<&str, &str, String> = Process::new("id", "processor", past_time);

        let status = process.status(Duration::from_secs(10));
        assert_eq!(status, ProcessStatus::Timeout);
    }

    #[test]
    fn test_process_status_priority_order() {
        // Test that status checks happen in the right order:
        // 1. Completed (if memoized value exists)
        // 2. Expired
        // 3. Timeout
        // 4. Running

        // Create a process that is both expired and timed out
        let past_time = SystemTime::now() - Duration::from_secs(20);
        let mut process: Process<&str, &str, String> = Process::new("id", "processor", past_time);

        let past_expiration = SystemTime::now() - Duration::from_secs(10);
        process.expires_on = Some(Expiration::new(past_expiration));

        // Should return Expired (higher priority than Timeout)
        let status = process.status(Duration::from_secs(10));
        assert_eq!(status, ProcessStatus::Expired);
    }

    #[test]
    fn test_process_status_completed_overrides_expired() {
        // A completed process with memoized value should return Completed
        // even if it's expired
        let mut process = Process::new("id", "processor", SystemTime::now());
        process.completed_at = Some(SystemTime::now());
        process.memoized = Some("result".to_string());

        let past_time = SystemTime::now() - Duration::from_secs(10);
        process.expires_on = Some(Expiration::new(past_time));

        let status = process.status(Duration::from_secs(60));
        match status {
            ProcessStatus::Completed(value) => assert_eq!(*value, "result"),
            _ => panic!("Expected Completed status to override Expired"),
        }
    }

    #[test]
    fn test_poll_strategy_linear_max_duration() {
        let strategy = PollStrategy::linear(Duration::from_secs(1), Duration::from_secs(10));
        assert_eq!(strategy.max_duration(), Duration::from_secs(10));
    }

    #[test]
    fn test_poll_strategy_backoff_max_duration() {
        let strategy = PollStrategy::backoff(Duration::from_secs(1), 2.0, Duration::from_secs(30));
        assert_eq!(strategy.max_duration(), Duration::from_secs(30));
    }
}
