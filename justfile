default: verify

# Run full workspace verification (checks, clippy, tests)
[group('tauqe')]
[doc('Run full workspace verification (checks, clippy, tests)')]
verify: check clippy test

# Run cargo check across all workspace targets
[group('tauqe')]
[doc('Run cargo check across all workspace targets')]
check:
    cargo check --workspace --all-targets

# Run cargo clippy with -D warnings across all workspace targets
[group('tauqe')]
[doc('Run cargo clippy with -D warnings across all workspace targets')]
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run all workspace tests
[group('tauqe')]
[doc('Run cargo test across the workspace')]
test:
    cargo test --workspace
    cargo test -p rurqe-sqlite --features r2d2 --doc
