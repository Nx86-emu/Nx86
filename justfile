# Nx86 developer automation. Run `just` to list recipes.

linux_target := "x86_64-unknown-linux-gnu"
release_binary := "target/" + linux_target + "/release/nx86-app"

# Show available recipes.
default:
    @just --list

# Show the toolchain and optional validation tools available on this host.
doctor:
    @rustc --version
    @cargo --version
    @just --version
    @printf 'host: %s %s\n' "$(uname -s)" "$(uname -m)"
    @printf 'installed Rust targets:\n'
    @rustup target list --installed | sed 's/^/  /'
    @for tool in cargo-audit actionlint shellcheck; do \
        if command -v "$tool" >/dev/null 2>&1; then \
            printf '%-12s %s\n' "$tool:" "$(command -v "$tool")"; \
        else \
            printf '%-12s %s\n' "$tool:" 'not installed'; \
        fi; \
    done

# Build the whole workspace.
build:
    cargo build --workspace

# Build the whole workspace without changing Cargo.lock.
build-locked:
    cargo build --workspace --locked

# Build an optimized application binary for the current host.
build-release:
    cargo build -p nx86-app --release --locked

# Launch the egui desktop shell (supported product target: Linux).
run *args:
    cargo run -p nx86-app -- {{ args }}

# Run a worker IPC mode: `just worker compiler-smoke` or `just worker runtime-smoke`.
worker mode='compiler-smoke':
    cargo run -p nx86-app -- --worker {{ mode }}

# Run both worker IPC smoke modes.
smoke:
    cargo run -p nx86-app --locked -- --worker compiler-smoke
    cargo run -p nx86-app --locked -- --worker runtime-smoke

# Format all Rust crates in place.
fmt:
    cargo fmt --all

# Check Rust formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Check that this Justfile itself is formatted.
just-check:
    just --fmt --check

# Lint every workspace target with warnings treated as errors.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Lint every workspace target without changing Cargo.lock.
clippy-locked:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run workspace tests; pass optional Cargo arguments after the recipe name.
test *args:
    cargo test --workspace --all-targets {{ args }}

# Run workspace tests without changing Cargo.lock.
test-locked *args:
    cargo test --workspace --all-targets --locked {{ args }}

# Test one crate: `just test-crate nx86-profile`.
test-crate crate *args:
    cargo test -p {{ crate }} --all-targets {{ args }}

# Type-check every workspace target without producing final binaries.
cargo-check:
    cargo check --workspace --all-targets --locked

# Build strict API documentation for every workspace crate.
docs:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --document-private-items --locked

# Build and open public API documentation in a browser.
docs-open:
    cargo doc --workspace --no-deps --locked --open

# Scan locked dependencies against the RustSec advisory database.
audit:
    cargo audit --deny warnings

# Validate GitHub Actions workflows (requires actionlint).
workflow-check:
    actionlint .github/workflows/ci.yml .github/workflows/linux-x86_64-v4.yml

# Validate repository shell scripts (requires shellcheck).
shell-check:
    shellcheck .github/scripts/*.sh

# Validate workflow and shell automation.
automation-check: workflow-check shell-check

# Reject whitespace errors in the working-tree patch.
diff-check:
    git diff --check

# Compile the Linux backend tests from any host with the target installed.
linux-check:
    cargo check -p nx86-backend --tests --target {{ linux_target }} --locked

# Report whether this host can execute x86_64-v4 artifacts.
cpu-check:
    bash .github/scripts/verify-linux-x86_64-v4.sh --cpu-only

# Run the Linux CI test strategy, including x86_64-v4 native tests when supported.
linux-test:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
        echo "linux-test requires a Linux x86_64 host" >&2
        exit 1
    fi
    if bash .github/scripts/verify-linux-x86_64-v4.sh --cpu-only; then
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C target-cpu=x86-64-v4' \
            cargo test --workspace --all-targets --target {{ linux_target }} --locked
    else
        cargo test --workspace --all-targets --locked
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C target-cpu=x86-64-v4' \
            cargo test --workspace --all-targets --target {{ linux_target }} --no-run --locked
    fi

# Build the CI-style static-CRT Linux x86_64-v4 release artifact.
release-build-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
        echo "release-build-linux requires a Linux x86_64 host" >&2
        exit 1
    fi
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C target-cpu=x86-64-v4 -C target-feature=+crt-static -C strip=symbols' \
        cargo build -p nx86-app --release --target {{ linux_target }} --locked

# Inspect and smoke-test a Linux x86_64-v4 release artifact.
release-verify-linux binary=release_binary:
    bash .github/scripts/verify-linux-x86_64-v4.sh {{ binary }}

# Build and verify the Linux x86_64-v4 release artifact.
release-linux: release-build-linux release-verify-linux

# Exact Rust gate used by the main CI workflow.
check: fmt-check clippy test build

# Reproducible Rust gate that also checks Cargo.lock stability.
check-locked: fmt-check clippy-locked test-locked build-locked

# Comprehensive pre-push gate, including smoke, docs, security, automation, and Linux compilation.
verify: just-check diff-check check-locked smoke docs audit automation-check linux-check

# Alias retained for CI-oriented local use.
ci: check

# Remove Cargo build artifacts.
clean:
    cargo clean
