# Nx86 automation. Run `just` to list recipes.
# Mirrors the CI checks in .github/workflows/ci.yml.

# Show available recipes.
default:
    @just --list

# Build the whole workspace.
build:
    cargo build --workspace

# Launch the egui desktop shell (Linux only).
run:
    cargo run -p nx86-app

# Run a worker IPC smoke mode: `just worker compiler-smoke` or `just worker runtime-smoke`.
worker mode='compiler-smoke':
    cargo run -p nx86-app -- --worker {{mode}}

# Format all crates in place.
fmt:
    cargo fmt --all

# Check formatting without modifying files (CI gate).
fmt-check:
    cargo fmt --all -- --check

# Lint with warnings treated as errors (CI gate).
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run tests. Optional filter: `just test run_synthetic` or `just test -p nx86-runtime`.
test *filter:
    cargo test --workspace --all-targets {{filter}}

# Full CI parity: format check, clippy, tests, build.
check: fmt-check clippy test build

# Alias for `check`.
ci: check
