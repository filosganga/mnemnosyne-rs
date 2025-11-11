# Justfile for mnemosyne-rs

# Show current version from git
show-version:
    @git describe --tags --always --dirty 2>/dev/null || echo "0.0.0-unknown"

# Set version in Cargo.toml to a specific version
set-version-to VERSION:
    #!/usr/bin/env bash
    set -euo pipefail

    CURRENT_VERSION=$(grep -E '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

    echo "Current version: $CURRENT_VERSION"
    echo "New version: {{VERSION}}"

    # Update Cargo.toml
    sed -i.bak 's/^version = ".*"/version = "{{VERSION}}"/' Cargo.toml && rm Cargo.toml.bak

    echo "✓ Version updated to: {{VERSION}}"

# Set version in Cargo.toml from git tags (similar to sbt-dynver)
set-version:
    #!/usr/bin/env bash
    set -euo pipefail

    # Get git describe output
    GIT_DESCRIBE=$(git describe --tags --always --dirty 2>/dev/null || echo "0.0.0-unknown")

    # Extract version components
    if [[ $GIT_DESCRIBE =~ ^v?([0-9]+\.[0-9]+\.[0-9]+)(-([0-9]+)-g([0-9a-f]+))?(-dirty)?$ ]]; then
        BASE_VERSION="${BASH_REMATCH[1]}"
        COMMITS_AHEAD="${BASH_REMATCH[3]}"
        COMMIT_HASH="${BASH_REMATCH[4]}"
        DIRTY="${BASH_REMATCH[5]}"

        if [ -n "$COMMITS_AHEAD" ] && [ "$COMMITS_AHEAD" != "0" ]; then
            VERSION="${BASE_VERSION}-dev.${COMMITS_AHEAD}+${COMMIT_HASH}"
        elif [ -n "$DIRTY" ]; then
            VERSION="${BASE_VERSION}-dirty"
        else
            VERSION="$BASE_VERSION"
        fi
    else
        VERSION="0.0.0-dev"
    fi

    CURRENT_VERSION=$(grep -E '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

    echo "Current version: $CURRENT_VERSION"
    echo "New version: $VERSION"

    # Update Cargo.toml
    sed -i.bak 's/^version = ".*"/version = "'"$VERSION"'"/' Cargo.toml && rm Cargo.toml.bak

    echo "✓ Version updated to: $VERSION"

# Build the project
build:
    cargo build

# Build release
build-release:
    cargo build --release

# Run tests
test:
    cargo test

# Run integration tests (requires DynamoDB Local)
test-integration:
    cargo test -- --ignored

# Run all tests including integration
test-all:
    docker-compose up -d
    cargo test -- --ignored
    docker-compose down

# Run clippy
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Check if package is ready for publishing
check-publish:
    cargo package --allow-dirty

# Publish to crates.io (requires version to be set and committed)
publish: set-version
    cargo publish

# Clean build artifacts
clean:
    cargo clean
