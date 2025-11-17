{
  description = "Mnemosyne.rs - Rust Development Environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Define stable Rust version
        rustVersion = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Rust toolchain
            rustVersion
            cargo-watch
            cargo-edit

            # Version control
            git

            # Task runner
            just

            # Docker Compose CLI (Docker itself provided by Colima)
            docker-compose

            # AWS CLI
            awscli2

            # GitHub CLI
            gh

            # Markdown linting
            markdownlint-cli2

            # Development tools
            pkg-config
            openssl

            # Optional: useful utilities
            jq
            curl
          ];

          shellHook = ''
            # To remove warning: unhandled Platform key FamilyDisplayName   
            unset DEVELOPER_DIR
            
            # Rust environment
            export RUST_BACKTRACE=1

            echo "🦀 Mnemosyne.rs Development Environment"
            echo ""
            echo "Available tools:"
            echo "  - Rust $(rustc --version | cut -d' ' -f2)"
            echo "  - Cargo $(cargo --version | cut -d' ' -f2)"
            echo "  - Docker $(docker --version | cut -d' ' -f3 | tr -d ',')"
            echo "  - AWS CLI $(aws --version | cut -d' ' -f1 | cut -d'/' -f2)"
            echo "  - GitHub CLI $(gh --version | head -n1 | cut -d' ' -f3)"
            echo "  - markdownlint $(markdownlint-cli2 --version)"
            echo ""
            echo "📝 Environment configuration:"
            echo "  AWS_PROFILE           - $AWS_PROFILE"
            echo ""
            echo "🔧 Useful commands:"
            echo "  just set-version      - Set version from git tags"
            echo "  just show-version     - Show current git-based version"
            echo "  just build            - Build the project"
            echo "  just test             - Run tests"
            echo "  just test-integration - Run integration tests"
            echo "  just clippy           - Run linter"
            echo "  just publish          - Publish to crates.io"
            echo "  cargo fmt             - Format code"
            echo "  markdownlint-cli2 \"**/*.md\" - Lint markdown files"
            echo ""

            # Check if LocalStack is running
            if docker ps | grep -q localstack; then
              echo "✅ LocalStack is running"
            else
              echo "⚠️  LocalStack is not running (optional for DynamoDB testing)"
            fi
            echo ""
          '';

          # Set environment variables for building
          RUST_SRC_PATH = "${rustVersion}/lib/rustlib/src/rust/library";
        };
      }
    );
}
