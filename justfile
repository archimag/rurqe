default: verify

# Run full workspace verification (checks, clippy, tests)
# Run full workspace verification (checks, clippy, tests, doc build)
[group('tauqe')]
[doc('Run full workspace verification (checks, clippy, tests, doc build)')]
verify: check clippy test doc-build

# Run cargo check across all workspace targets
[group('tauqe')]
[doc('Run cargo check across all workspace targets')]
check:
    # `--workspace` includes the root `examples` crate and `--all-targets` its
    # `[[example]]` targets, so every documented example is compiled here.
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

# Verify the book: example coverage guard and a clean mdBook build
[group('tauqe')]
[doc('Verify the book: example coverage guard and a clean mdBook build')]
doc-test:
    cargo test -p rurqe-examples --test doc_coverage
    mdbook build docs --dest-dir ../target/book

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

# Format all workspace code
[doc('Format all workspace code')]
fmt:
    cargo fmt --all

# Check formatting across all workspace targets
[doc('Check formatting across all workspace targets')]
fmt-check:
    cargo fmt --all -- --check
