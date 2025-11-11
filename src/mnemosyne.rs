use crate::error::Error;
use crate::model::{Config, Outcome, PollStrategy, ProcessStatus};
use crate::persistence::Persistence;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

#[cfg(feature = "tracing")]
use tracing::{debug, info, instrument, warn};

// No-op macros when tracing is disabled
#[cfg(not(feature = "tracing"))]
macro_rules! debug {
    ($($tt:tt)*) => {{}};
}
#[cfg(not(feature = "tracing"))]
macro_rules! info {
    ($($tt:tt)*) => {{}};
}
#[cfg(not(feature = "tracing"))]
macro_rules! warn {
    ($($tt:tt)*) => {{}};
}

/// Main Mnemosyne API for deduplicating process execution
pub struct Mnemosyne<Id, ProcessorId, A> {
    persistence: Arc<dyn Persistence<Id, ProcessorId, A>>,
    config: Config<ProcessorId>,
}

impl<Id, ProcessorId, A> Mnemosyne<Id, ProcessorId, A>
where
    Id: Clone + Send + Sync + std::fmt::Debug + 'static,
    ProcessorId: Clone + Send + Sync + std::fmt::Debug + 'static,
    A: Clone + Send + Sync + 'static,
{
    /// Create a new Mnemosyne instance
    pub fn new(
        persistence: Arc<dyn Persistence<Id, ProcessorId, A>>,
        config: Config<ProcessorId>,
    ) -> Self {
        Self {
            persistence,
            config,
        }
    }

    /// Try to start processing a signal.
    ///
    /// Atomically attempts to claim processing of a signal. Returns either:
    /// - `Outcome::New` with a completion callback if this is the first processor
    /// - `Outcome::Duplicate` with memoized value if already processed
    ///
    /// This provides the low-level API for manual control. Most users should use `once()` instead.
    #[cfg_attr(feature = "tracing", instrument(skip(self), fields(signal_id = ?id)))]
    pub async fn try_start_process(&self, id: Id) -> Result<Outcome<A>, Error> {
        let now = SystemTime::now();
        let processor_id = self.config.processor_id.clone();
        let max_processing_time = self.config.max_processing_time;

        debug!("Attempting to start process");

        // Phase 1: Try to claim the process
        let previous_process = self
            .persistence
            .start_processing_update(id.clone(), processor_id.clone(), now)
            .await?;

        match previous_process {
            None => {
                // This is a new process
                info!("New process - no previous record found");
                Ok(self.create_new_outcome(id, processor_id))
            }
            Some(process) => {
                // A record exists - determine its status
                let status = process.status(max_processing_time);

                match status {
                    ProcessStatus::Completed(memoized) => {
                        info!("Process already completed - returning memoized value");
                        Ok(Outcome::Duplicate {
                            value: memoized.clone(),
                        })
                    }
                    ProcessStatus::Expired => {
                        info!("Previous process expired - allowing retry");
                        Ok(self.create_new_outcome(id, processor_id))
                    }
                    ProcessStatus::Timeout => {
                        info!("Previous process timed out - allowing retry");
                        Ok(self.create_new_outcome(id, processor_id))
                    }
                    ProcessStatus::Running => {
                        // Process is still running - poll and wait
                        warn!("Process is currently running - will poll");
                        self.poll_for_completion(id, processor_id, max_processing_time)
                            .await
                    }
                    ProcessStatus::NotStarted => {
                        // Shouldn't happen since we have a process record
                        info!("Unexpected NotStarted status - treating as new");
                        Ok(self.create_new_outcome(id, processor_id))
                    }
                }
            }
        }
    }

    /// Run an effect once across distributed systems.
    ///
    /// Provides at-least-once semantics with best-effort exactly-once through
    /// distributed deduplication. Returns the result whether from fresh execution
    /// or memoized from a previous run.
    #[cfg_attr(feature = "tracing", instrument(skip(self, f), fields(signal_id = ?id)))]
    pub async fn once<F, Fut>(&self, id: Id, f: F) -> Result<A, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<A, Error>>,
    {
        let outcome = self.try_start_process(id).await?;

        match outcome {
            Outcome::New { complete_process } => {
                let result = f().await?;
                complete_process(result.clone()).await?;
                Ok(result)
            }
            Outcome::Duplicate { value } => Ok(value),
        }
    }

    /// Invalidate a previously processed signal
    #[cfg_attr(feature = "tracing", instrument(skip(self), fields(signal_id = ?id)))]
    pub async fn invalidate(&self, id: Id) -> Result<(), Error> {
        let processor_id = self.config.processor_id.clone();
        self.persistence.invalidate_process(id, processor_id).await
    }

    /// Create a New outcome with the completion callback
    fn create_new_outcome(&self, id: Id, processor_id: ProcessorId) -> Outcome<A> {
        let persistence = Arc::clone(&self.persistence);
        let ttl = self.config.ttl;

        Outcome::New {
            complete_process: Box::new(move |value: A| {
                Box::pin(async move {
                    let now = SystemTime::now();
                    persistence
                        .complete_process(id, processor_id, now, ttl, value)
                        .await
                })
            }),
        }
    }

    /// Poll for process completion using the configured strategy
    #[cfg_attr(feature = "tracing", instrument(skip(self), fields(signal_id = ?id, processor_id = ?processor_id)))]
    async fn poll_for_completion(
        &self,
        id: Id,
        processor_id: ProcessorId,
        max_processing_time: Duration,
    ) -> Result<Outcome<A>, Error> {
        let poll_strategy = self.config.poll_strategy;
        let max_poll_duration = poll_strategy.max_duration();
        let start_time = SystemTime::now();

        let mut attempt = 0;

        loop {
            // Calculate delay based on strategy
            let delay = match poll_strategy {
                PollStrategy::Linear { delay, .. } => delay,
                PollStrategy::Backoff {
                    base_delay,
                    multiplier,
                    ..
                } => {
                    let factor = multiplier.powi(attempt);
                    Duration::from_secs_f64(base_delay.as_secs_f64() * factor)
                }
            };

            sleep(delay).await;
            attempt += 1;

            // Check if we've exceeded max poll duration
            let elapsed = SystemTime::now()
                .duration_since(start_time)
                .map_err(|e| Error::Internal(e.to_string()))?;

            if elapsed >= max_poll_duration {
                warn!("Polling exceeded max duration - treating as timeout");
                return Ok(self.create_new_outcome(id, processor_id));
            }

            // Try to start again - will check current status
            debug!("Polling attempt {}", attempt);
            let previous_process = self
                .persistence
                .start_processing_update(id.clone(), processor_id.clone(), SystemTime::now())
                .await?;

            if let Some(process) = previous_process {
                let status = process.status(max_processing_time);

                match status {
                    ProcessStatus::Completed(memoized) => {
                        info!("Process completed during polling");
                        return Ok(Outcome::Duplicate {
                            value: memoized.clone(),
                        });
                    }
                    ProcessStatus::Expired | ProcessStatus::Timeout => {
                        info!("Process expired/timed out during polling");
                        return Ok(self.create_new_outcome(id, processor_id));
                    }
                    ProcessStatus::Running => {
                        // Still running, continue polling
                        debug!("Process still running, continuing to poll");
                        continue;
                    }
                    ProcessStatus::NotStarted => {
                        // Process disappeared or never existed
                        return Ok(self.create_new_outcome(id, processor_id));
                    }
                }
            } else {
                // Process record disappeared
                return Ok(self.create_new_outcome(id, processor_id));
            }
        }
    }
}
