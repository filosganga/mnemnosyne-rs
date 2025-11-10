use crate::model::Process;
use crate::Error;
use async_trait::async_trait;
use std::time::{Duration, SystemTime};

/// Abstraction for persisting process records
#[async_trait]
pub trait Persistence<Id, ProcessorId, A>: Send + Sync {
    /// Attempt to start processing a signal
    /// Returns the previous process record if one existed
    async fn start_processing_update(
        &self,
        id: Id,
        processor_id: ProcessorId,
        now: SystemTime,
    ) -> Result<Option<Process<Id, ProcessorId, A>>, Error>;

    /// Mark a process as completed with a memoized result
    async fn complete_process(
        &self,
        id: Id,
        processor_id: ProcessorId,
        now: SystemTime,
        ttl: Option<Duration>,
        value: A,
    ) -> Result<(), Error>;

    /// Delete a process record
    async fn invalidate_process(&self, id: Id, processor_id: ProcessorId) -> Result<(), Error>;
}
