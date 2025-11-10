//! Mnemosyne - A library for deduplicating process execution
//!
//! Named after the Greek goddess of memory, Mnemosyne provides best-effort exactly-once
//! processing semantics by preventing duplicate execution of signals/messages across
//! distributed systems. It tracks which messages have been processed using DynamoDB's
//! strong consistency guarantees to achieve at-least-once delivery with deduplication.
//!
//! # Example
//!
//! ```no_run
//! use mnemosyne_rs::{Mnemosyne, Config, PollStrategy, DynamoDbPersistence};
//! use std::time::Duration;
//! use std::sync::Arc;
//! use uuid::Uuid;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Initialize AWS DynamoDB client
//! let aws_config = aws_config::load_from_env().await;
//! let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
//!
//! // Create persistence layer
//! let persistence = Arc::new(DynamoDbPersistence::new(
//!     dynamodb_client,
//!     "mnemosyne-processes".to_string(),
//! ));
//!
//! // Configure Mnemosyne
//! let config = Config::new(
//!     Uuid::new_v4(), // processor ID
//!     Duration::from_secs(300), // max processing time
//!     Some(Duration::from_secs(86400 * 30)), // 30 day TTL
//!     PollStrategy::backoff(
//!         Duration::from_millis(100),
//!         2.0,
//!         Duration::from_secs(15)
//!     ),
//! );
//!
//! // Create Mnemosyne instance
//! let mnemosyne = Mnemosyne::new(persistence, config);
//!
//! // Use it to deduplicate processing
//! let signal_id = Uuid::new_v4();
//! let result = mnemosyne.protect(signal_id, || async {
//!     // Your processing logic here
//!     Ok("processed".to_string())
//! }).await?;
//!
//! # Ok(())
//! # }
//! ```

pub mod dynamodb;
pub mod error;
pub mod mnemosyne;
pub mod model;
pub mod persistence;

// Re-export commonly used types
pub use dynamodb::DynamoDbPersistence;
pub use error::Error;
pub use mnemosyne::Mnemosyne;
pub use model::{Config, Expiration, Outcome, PollStrategy, Process, ProcessStatus};
pub use persistence::Persistence;
