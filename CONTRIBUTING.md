# Contributing to Mnemosyne-rs

Thank you for your interest in contributing to Mnemosyne-rs! This document provides guidelines and information for contributors.

## Development Setup

### Prerequisites

This project uses [Nix](https://nixos.org/) for reproducible development environments. If you don't have Nix installed:

```bash
# Install Nix (multi-user installation recommended)
sh <(curl -L https://nixos.org/nix/install) --daemon

# Enable flakes (if not already enabled)
mkdir -p ~/.config/nix
echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

### Getting Started

1. **Clone the repository**

   ```bash
   git clone https://github.com/filosganga/mnemosyne-rs.git
   cd mnemosyne-rs
   ```

2. **Enter the development environment**

   ```bash
   # Using nix directly
   nix develop

   # Or with direnv (recommended)
   echo "use flake" > .envrc
   direnv allow
   ```

   The development shell includes:
   - Rust toolchain (stable)
   - AWS CLI
   - Docker Compose
   - GitHub CLI
   - `just` task runner
   - All necessary build tools

3. **Verify setup**

   ```bash
   cargo build
   cargo test
   ```

## Development Workflow

### Available Commands

The project uses `just` as a task runner. View all available commands:

```bash
just --list
```

Common commands:

```bash
just build              # Build the project
just test               # Run unit tests
just test-integration   # Run integration tests (requires DynamoDB Local)
just test-all           # Run all tests with DynamoDB Local
just clippy             # Run linter
just check-publish      # Verify package is ready for publishing
just clean              # Clean build artifacts
```

### Running Tests

#### Unit Tests

```bash
cargo test
```

#### Integration Tests

Integration tests require DynamoDB Local:

```bash
# Start DynamoDB Local
docker-compose up -d

# Run integration tests
cargo test -- --ignored

# Or use the just command that handles everything
just test-all

# Stop DynamoDB Local
docker-compose down
```

You can also configure a custom DynamoDB endpoint:

```bash
export MNEMOSYNE_DYNAMODB_ENDPOINT=http://localhost:8000
cargo test -- --ignored
```

### Code Quality

Before submitting a PR, ensure your code passes all checks:

```bash
# Format code
cargo fmt

# Run clippy
just clippy

# Run all tests
just test-all

# Verify package builds
just check-publish
```

## Versioning Strategy

This project uses git tags for versioning, similar to [sbt-dynver](https://github.com/sbt/sbt-dynver).

### How It Works

The `just set-version` command automatically derives the version from git tags:

- **On a tag** (e.g., `v0.1.0`): Uses exact version `0.1.0`
- **Commits after a tag** (e.g., `v0.1.0-5-g1a2b3c4`): Uses pre-release version `0.1.0-dev.5+1a2b3c4`
- **Dirty working directory**: Appends `-dirty` to the version
- **No tags**: Uses default version `0.0.0-dev`

### Version Commands

```bash
# Show current git-based version
just show-version

# Update Cargo.toml with git-based version
just set-version
```

## Pull Request Process

1. **Fork and create a branch**

   ```bash
   git checkout -b feature/your-feature-name
   # or
   git checkout -b fix/issue-description
   ```

2. **Make your changes**
   - Write clear, concise commit messages
   - Add tests for new features
   - Update documentation as needed
   - Ensure all tests pass

3. **Run quality checks**

   ```bash
   cargo fmt
   just clippy
   just test-all
   ```

4. **Commit and push**

   ```bash
   git add .
   git commit -m "feat: add new feature"
   git push origin feature/your-feature-name
   ```

5. **Create a pull request**
   - Provide a clear description of changes
   - Reference any related issues
   - Ensure CI passes

## Commit Message Convention

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <description>

[optional body]

[optional footer]
```

Types:

- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation changes
- `test:` - Adding or updating tests
- `refactor:` - Code refactoring
- `perf:` - Performance improvements
- `chore:` - Maintenance tasks

Examples:

```bash
feat: add support for custom TTL per signal
fix: prevent race condition in polling logic
docs: update README with new examples
test: add integration test for timeout recovery
```

## Release Process

Releases are managed by maintainers. The process is:

1. **Create and push a version tag**

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

2. **Update version in Cargo.toml**

   ```bash
   just set-version
   git add Cargo.toml Cargo.lock
   git commit -m "chore: release v0.2.0"
   git push
   ```

3. **Publish to crates.io**

   ```bash
   cargo publish
   ```

   Or use the combined command:

   ```bash
   just publish
   ```

## Code Style

- Follow Rust standard style (`rustfmt`)
- Use meaningful variable and function names
- Add doc comments for public APIs
- Keep functions focused and concise
- Write self-documenting code

### Documentation

All public APIs must have documentation:

```rust
/// Protects an effect from being executed multiple times.
///
/// This method wraps your processing logic and ensures it runs at most once
/// for the given signal ID.
///
/// # Arguments
///
/// * `id` - Unique identifier for the signal to process
/// * `f` - The effect to protect (async closure)
///
/// # Returns
///
/// The result of the effect, either from fresh execution or memoized value
///
/// # Example
///
/// ```no_run
/// let result = mnemosyne.protect(signal_id, || async {
///     process_payment().await
/// }).await?;
/// ```
pub async fn protect<F, Fut>(&self, id: Id, f: F) -> Result<A, Error>
```

## Testing Guidelines

### Test Organization

- **Unit tests**: In `src/` files using `#[cfg(test)]` modules
- **Integration tests**: In `tests/` directory
- **Correctness tests**: In `tests/correctness_test.rs` for concurrency guarantees

### Writing Tests

```rust
#[tokio::test]
async fn test_descriptive_name() {
    // Arrange
    let mnemosyne = create_test_mnemosyne().await;
    let signal_id = Uuid::new_v4();

    // Act
    let result = mnemosyne.protect(signal_id, || async {
        Ok("test".to_string())
    }).await;

    // Assert
    assert!(result.is_ok());
}
```

### Testing Best Practices

- Test one thing per test
- Use descriptive test names
- Clean up resources (DynamoDB tables, etc.)
- Use atomic counters to verify exactly-once semantics
- Test concurrent scenarios for race conditions

## Architecture Overview

### Two-Phase Commit Protocol

1. **Phase 1 - Start Processing**
   - Atomically create or update DynamoDB record
   - Use conditional expressions (`if_not_exists`)
   - Record `startedAt` timestamp

2. **Phase 2 - Complete Processing**
   - Update record with `completedAt` timestamp
   - Store memoized result
   - Set TTL if configured

### Process States

- **New**: First time seeing this signal
- **Running**: Currently being processed
- **Completed**: Already processed, return memoized value
- **Timeout**: Exceeded `maxProcessingTime`, allow retry
- **Expired**: Exceeded TTL, allow retry

## Troubleshooting

### DynamoDB Local Issues

```bash
# Check if running
docker ps | grep dynamodb

# View logs
docker-compose logs dynamodb-local

# Restart
docker-compose restart
```

### Build Issues

```bash
# Clean and rebuild
just clean
cargo build

# Update dependencies
cargo update
```

## Getting Help

- **Issues**: [GitHub Issues](https://github.com/filosganga/mnemosyne-rs/issues)
- **Discussions**: [GitHub Discussions](https://github.com/filosganga/mnemosyne-rs/discussions)
- **Original Scala version**: [filosganga/mnemosyne](https://github.com/filosganga/mnemosyne)

## License

By contributing to Mnemosyne-rs, you agree that your contributions will be licensed under the Apache-2.0 license.
