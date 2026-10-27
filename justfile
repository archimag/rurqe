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

# Build the mdBook documentation into target/book
[group('tauqe')]
[doc('Build the mdBook documentation into target/book')]
doc-build:
    mdbook build docs --dest-dir ../target/book

# Serve the mdBook documentation locally with live reload
[group('tauqe')]
[doc('Serve the mdBook documentation locally with live reload')]
doc-serve:
    mdbook serve docs --dest-dir ../target/book --open
