# NOTE: This shell script should always have unix(LF) line endings
#!/usr/bin/env bash
set -euo pipefail

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Run cargo using the Cargo.toml in that directory
cargo run --quiet --manifest-path "$SCRIPT_DIR/Cargo.toml"
