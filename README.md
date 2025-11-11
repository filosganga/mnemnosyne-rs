# Mnemosyne-rs

A Rust library for deduplicating process execution across distributed systems using DynamoDB.

Named after the Greek goddess of memory, Mnemosyne prevents duplicate processing of signals/messages across distributed systems by tracking which messages have been processed by each processor instance.

## Features

- **Best-effort exactly-once processing** - Provides at-least-once semantics with distributed deduplication to achieve effectively-once execution
- **Two-phase commit** - Uses DynamoDB's strong consistency to implement reliable deduplication
- **Memoization** - Stores and returns the result of previous processing
- **Timeout handling** - Automatically retries stuck processes after a configurable timeout
- **TTL support** - Automatic cleanup of old records using DynamoDB's native TTL feature
- **Configurable polling** - Linear or exponential backoff strategies for waiting on in-progress processes
- **Generic and type-safe** - Works with any serializable types for IDs and results

## Processing Guarantees

Mnemosyne provides **exactly-once processing in normal operations**, with **at-least-once as the failure mode**.

### Normal Operation (Exactly-Once)

Under normal conditions, when processes complete successfully:

- A signal is processed exactly once across all processor instances
- Results are memoized and returned for subsequent requests
- No duplicate execution occurs, even with concurrent requests

### Failure Mode (At-Least-Once)

In case of process crashes or failures before completion:

- The signal may be retried by the same or different processor
- The timeout mechanism allows recovery from stuck processes
- Your processing logic must be idempotent to handle potential retries

This design ensures **reliability** (signals are never lost) while providing **best-effort exactly-once** semantics. The two-phase commit protocol with DynamoDB's strong consistency guarantees that in the absence of failures, each signal executes exactly once.

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
    let result = mnemosyne.once(signal_id, || async {
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

### Basic Usage with `once`

The `once` method wraps your processing logic and deduplicates execution:

```rust
let result = mnemosyne.once(signal_id, || async {
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

### Configuration Parameters

When creating a `Config`, you need to specify four parameters:

```rust
let config = Config::new(
    processor_id,           // Unique ID for this processor/process type
    max_processing_time,    // Maximum time allowed for processing
    ttl,                    // Optional time-to-live for records
    poll_strategy,          // How to poll for in-progress processes
);
```

### Understanding `processor_id`

The `processor_id` identifies the **type of process** you're protecting, not the individual instance.

**Key points:**

- Use the same `processor_id` for all instances handling the same type of work
- Different process types should use different `processor_id` values
- Allows multiple Mnemosyne instances to share the same DynamoDB table

**Examples:**

```rust
// Different process types can share the same table
let email_processor_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")?;
let webhook_processor_id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")?;
let payment_processor_id = Uuid::parse_str("7c9e6679-7425-40de-944b-e07fc1f90ae7")?;

// Each Mnemosyne instance handles a different process type
let email_mnemosyne = Mnemosyne::new(persistence.clone(), Config::new(
    email_processor_id, // Deduplicates email sending
    Duration::from_secs(60),
    Some(Duration::from_secs(86400)),
    poll_strategy,
));

let webhook_mnemosyne = Mnemosyne::new(persistence.clone(), Config::new(
    webhook_processor_id, // Deduplicates webhook calls
    Duration::from_secs(30),
    Some(Duration::from_secs(86400)),
    poll_strategy,
));
```

The composite key is `(signal_id, processor_id)`, so:

- Same signal across different processors = independent processes
- Same signal for same processor = deduplicated

### Choosing `max_processing_time`

This determines how long a process can run before being considered timed out and allowing retry.

**Guidelines:**

- Set this to your **worst-case processing time** + buffer
- If your operation typically takes 30s, set to 60-120s
- Too short: premature timeouts cause duplicate execution
- Too long: stuck processes delay retry

**Examples:**

```rust
// Fast API calls (< 5s typical)
Duration::from_secs(30)

// Database operations (5-30s typical)
Duration::from_secs(120)

// Heavy computations or external API calls (30-300s typical)
Duration::from_secs(600)
```

### Choosing TTL

TTL automatically cleans up old process records from DynamoDB.

**Guidelines:**

- Set based on how long you need to maintain deduplication
- Use `None` for permanent deduplication if needed
- Consider your use case:
  - **Event processing**: 1-7 days (prevent replay attacks)
  - **API idempotency**: 24 hours (prevent duplicate requests)
  - **Long-running workflows**: 30+ days (maintain state across restarts)
  - **Audit trail**: `None` (keep records forever)

**Examples:**

```rust
// Short-term deduplication (API requests)
Some(Duration::from_secs(86400))  // 24 hours

// Medium-term deduplication (event processing)
Some(Duration::from_secs(86400 * 7))  // 7 days

// Long-term deduplication (workflow state)
Some(Duration::from_secs(86400 * 30))  // 30 days

// Permanent deduplication (audit trail)
None
```

### Poll Strategies

When a process is already running, other instances will poll and wait for completion.

#### Linear Backoff

Fixed delay between each poll attempt:

```rust
PollStrategy::linear(
    Duration::from_millis(100), // delay between polls
    Duration::from_secs(10),    // max total poll duration
)
```

**Use when:**

- Processing time is consistent and predictable
- You want simple, predictable behavior
- Low latency is critical

#### Exponential Backoff

Delay increases exponentially with each attempt:

```rust
PollStrategy::backoff(
    Duration::from_millis(50),  // base delay
    2.0,                         // multiplier (delay doubles each attempt)
    Duration::from_secs(15),     // max total poll duration
)
```

**Use when:**

- Processing time is variable
- You want to reduce DynamoDB read load
- You expect most processes to complete quickly

**Choosing poll parameters:**

- **Base delay/delay**: Start with 50-100ms for responsive systems
- **Max poll duration**: Should be ≤ `max_processing_time`
- If max poll duration is exceeded, the waiting process will retry as a new process
- Trade-off: Shorter delays = more responsive but higher DynamoDB costs

### Complete Example

```rust
let config = Config::new(
    Uuid::new_v4(),                    // processor_id: unique per process type
    Duration::from_secs(300),          // max_processing_time: 5 minutes
    Some(Duration::from_secs(86400 * 7)), // ttl: 7 days
    PollStrategy::backoff(
        Duration::from_millis(100),    // start with 100ms delay
        2.0,                            // double delay each attempt
        Duration::from_secs(60),       // poll for up to 1 minute
    ),
);
```

## Type Requirements

Mnemosyne requires that `Id`, `ProcessorId`, and `A` (the result type) all have a `'static` lifetime bound. This means they must be owned types (like `String`, `Uuid`, custom structs) rather than borrowed references.

### Why `'static` is required

The `'static` requirement comes from two fundamental constraints:

1. **Trait object with generics**: Mnemosyne uses `Arc<dyn Persistence<Id, ProcessorId, A>>` for runtime polymorphism, allowing different persistence implementations. When trait objects have generic parameters, Rust requires those parameters to be `'static` to ensure type safety across async boundaries.

2. **Persistence and deserialization**: All three types must be serialized to and deserialized from DynamoDB:
   - When storing a process, `Id`, `ProcessorId`, and `A` are serialized to DynamoDB
   - When fetching, new instances are deserialized from bytes
   - The library returns owned instances to the caller, not borrowed data

### What this means for users

You must use owned types for IDs and results:

```rust
// ✅ Works - owned types
let mnemosyne: Mnemosyne<Uuid, Uuid, String> = ...;
let mnemosyne: Mnemosyne<String, String, MyStruct> = ...;

// ❌ Won't compile - borrowed types
let mnemosyne: Mnemosyne<&str, &str, String> = ...;
struct Borrowed<'a> { data: &'a str }
let mnemosyne: Mnemosyne<Uuid, Uuid, Borrowed> = ...;
```

In practice, this is not a limitation because:

- IDs are typically `Uuid` (which is `Copy`) or `String`
- Results must be serializable anyway for DynamoDB storage
- Memoized values need to outlive the original computation

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
- `once` - Running an effect once across distributed systems
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
