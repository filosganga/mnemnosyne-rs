# Mnemosyne-rs

A Rust library for deduplicating process execution across distributed systems using DynamoDB.

Named after the Greek goddess of memory, Mnemosyne prevents duplicate processing of signals/messages across distributed systems by tracking which messages have been processed by each processor instance.

## Features

- **Exactly-once processing semantics** - Guarantees that a signal is processed at most once per processor instance
- **Two-phase commit** - Uses DynamoDB's strong consistency to implement reliable deduplication
- **Memoization** - Stores and returns the result of previous processing
- **Timeout handling** - Automatically retries stuck processes after a configurable timeout
- **TTL support** - Automatic cleanup of old records using DynamoDB's native TTL feature
- **Configurable polling** - Linear or exponential backoff strategies for waiting on in-progress processes
- **Generic and type-safe** - Works with any serializable types for IDs and results

## Use Cases

- Preventing duplicate processing of queue messages (SQS, Kafka, etc.)
- Ensuring idempotent API operations
- Deduplicating event handling in distributed systems
- Coordinating work across multiple service instances

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
mnemosyne-rs = "0.1"
aws-config = "1.5"
aws-sdk-dynamodb = "1.52"
tokio = { version = "1", features = ["full"] }
uuid = { version = "1.0", features = ["v4", "serde"] }
```

## Quick Start

```rust
use mnemosyne_rs::{Mnemosyne, Config, PollStrategy, DynamoDbPersistence};
use std::time::Duration;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize AWS DynamoDB client
    let aws_config = aws_config::load_from_env().await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    // Create persistence layer
    let persistence = Arc::new(DynamoDbPersistence::new(
        dynamodb_client,
        "mnemosyne-processes".to_string(),
    ));

    // Configure Mnemosyne
    let config = Config::new(
        Uuid::new_v4(), // processor ID
        Duration::from_secs(300), // max processing time
        Some(Duration::from_secs(86400 * 30)), // 30 day TTL
        PollStrategy::backoff(
            Duration::from_millis(100),
            2.0,
            Duration::from_secs(15)
        ),
    );

    // Create Mnemosyne instance
    let mnemosyne = Mnemosyne::new(persistence, config);

    // Use it to deduplicate processing
    let signal_id = Uuid::new_v4();
    let result = mnemosyne.protect(signal_id, || async {
        // Your processing logic here - will only run once
        println!("Processing signal...");
        Ok("processed".to_string())
    }).await?;

    println!("Result: {}", result);
    Ok(())
}
```

## DynamoDB Setup

Create a DynamoDB table with the following schema:

```bash
aws dynamodb create-table \
    --table-name mnemosyne-processes \
    --attribute-definitions \
        AttributeName=id,AttributeType=S \
        AttributeName=processorId,AttributeType=S \
    --key-schema \
        AttributeName=id,KeyType=HASH \
        AttributeName=processorId,KeyType=RANGE \
    --billing-mode PAY_PER_REQUEST
```

Enable TTL (optional but recommended):

```bash
aws dynamodb update-time-to-live \
    --table-name mnemosyne-processes \
    --time-to-live-specification \
        Enabled=true,AttributeName=expiresOn
```

## Usage Examples

### Basic Usage with `protect`

The `protect` method wraps your processing logic and ensures it runs at most once:

```rust
let result = mnemosyne.protect(signal_id, || async {
    // Your expensive operation here
    process_payment().await
}).await?;
```

### Manual Control with `try_start_process`

For more control, use `try_start_process` directly:

```rust
use mnemosyne_rs::Outcome;

match mnemosyne.try_start_process(signal_id).await? {
    Outcome::New { complete_process } => {
        // This is the first time processing this signal
        let result = do_work().await?;
        complete_process(result.clone()).await?;
        println!("Processed: {:?}", result);
    }
    Outcome::Duplicate { value } => {
        // Already processed - use memoized result
        println!("Already processed: {:?}", value);
    }
}
```

### Invalidation

Remove a processed signal to allow reprocessing:

```rust
mnemosyne.invalidate(signal_id).await?;
```

## Configuration

### Poll Strategies

**Linear backoff:**
```rust
PollStrategy::linear(
    Duration::from_millis(100), // delay between polls
    Duration::from_secs(10),    // max total poll duration
)
```

**Exponential backoff:**
```rust
PollStrategy::backoff(
    Duration::from_millis(50),  // base delay
    2.0,                         // multiplier
    Duration::from_secs(15),     // max total poll duration
)
```

### TTL Configuration

Set a TTL to automatically clean up old records:

```rust
let config = Config::new(
    processor_id,
    Duration::from_secs(300),
    Some(Duration::from_secs(86400 * 7)), // 7 day TTL
    poll_strategy,
);
```

## Architecture

Mnemosyne uses a two-phase commit protocol:

1. **Phase 1 - Start Processing**: Atomically creates or updates a record with `startedAt` timestamp
2. **Phase 2 - Complete Processing**: Updates the record with `completedAt` and memoized result

Process states:
- **New**: Never seen before, proceed with processing
- **Running**: Currently being processed by another instance, poll and wait
- **Completed**: Already processed, return memoized result
- **Timeout**: Exceeded `maxProcessingTime`, allow retry
- **Expired**: Exceeded TTL, allow retry

## Testing

Run unit tests:

```bash
cargo test
```

Run integration tests (requires DynamoDB Local):

```bash
# Start DynamoDB Local using docker-compose
docker-compose up -d

# Run integration tests
cargo test --ignored

# Stop DynamoDB Local
docker-compose down
```

You can also use the environment variable `MNEMOSYNE_DYNAMODB_ENDPOINT` to configure the DynamoDB endpoint:

```bash
export MNEMOSYNE_DYNAMODB_ENDPOINT=http://localhost:8000
cargo test --ignored
```

## Observability

Mnemosyne includes optional tracing support via the `tracing` crate (enabled by default).

### Enabling Tracing

To see trace output, initialize a tracing subscriber in your application:

```rust
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize tracing subscriber
    tracing_subscriber::fmt::init();

    // Your code using Mnemosyne...
}
```

### Tracing Levels

The library emits traces at different levels:
- `DEBUG` - Low-level operations (polling attempts, state checks)
- `INFO` - Significant events (new process, completed, duplicate detected)
- `WARN` - Recoverable issues (timeouts, polling exceeded)

### Filtering Traces

Filter traces by target and level:

```rust
use tracing_subscriber::{EnvFilter, fmt};

fmt()
    .with_env_filter(
        EnvFilter::new("mnemosyne_rs=debug")
    )
    .init();
```

Or via environment variable:

```bash
RUST_LOG=mnemosyne_rs=debug cargo run
```

### Trace Spans

Key operations are instrumented with spans that include signal_id and processor_id fields:
- `try_start_process` - Attempting to start processing a signal
- `protect` - Protecting an effect from duplicate execution
- `invalidate` - Invalidating a previously processed signal
- `poll_for_completion` - Polling for an in-progress process

### Disabling Tracing

To compile without tracing support (zero-cost):

```toml
[dependencies]
mnemosyne-rs = { version = "0.1", default-features = false }
```

## Development

This project uses Nix for development environment management:

```bash
nix develop
```

Or with direnv:

```bash
echo "use flake" > .envrc
direnv allow
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed development guidelines, testing procedures, and release process.

## License

Apache-2.0

## Credits

This is a Rust port of the original Scala implementation at [filosganga/mnemosyne](https://github.com/filosganga/mnemosyne).