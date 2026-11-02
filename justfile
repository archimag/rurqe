default: verify

# Run full workspace verification (checks, clippy, tests, doc build)
[group('tauqe')]
[doc('Run full workspace verification (checks, clippy, tests, doc build)')]
verify: check clippy test doc-build

# Run cargo check across all workspace targets
[group('tauqe')]
[doc('Run cargo check across all workspace targets')]
check: check-features
    # `--workspace` includes the `docs/examples` crate and `--all-targets`
    # its `[[example]]` targets, so every documented example is compiled here.
    cargo check --workspace --all-targets

# Check the PostgreSQL adapter under each feature combination a la carte
[group('tauqe')]
[doc('Check the PostgreSQL adapter under each feature combination a la carte')]
check-features:
    # A pool adapter executes through the driver it pools: `deadpool-postgres`
    # carries the asynchronous adapter and `r2d2` the synchronous one. Each
    # combination below is built on its own rather than under the feature
    # unification of the workspace check, so the coupling declared in the
    # adapter's manifest is exercised by a build instead of only documented.
    cargo check -p rurqe-postgres --no-default-features
    cargo check -p rurqe-postgres --no-default-features --features deadpool-postgres
    cargo check -p rurqe-postgres --no-default-features --features r2d2
    cargo check -p rurqe-postgres --no-default-features --all-features

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
    # The book root is `docs/`: `docs/book.toml` declares `src = "."`, so the
    # chapters sit directly under `docs/`, and the compiled examples crate sits
    # beside them at `docs/examples`.
    cargo test -p rurqe-examples --test doc_coverage
    mdbook build docs --dest-dir target/book

# Build the mdBook documentation into target/book
[group('tauqe')]
[doc('Build the mdBook documentation into target/book')]
doc-build:
    # `docs/book.toml` sets `src = "."`, so the chapters live directly in
    # `docs/` and the book renders into `target/book` at the repository root.
    mdbook build docs --dest-dir target/book

# Serve the mdBook documentation locally with live reload
[group('tauqe')]
[doc('Serve the mdBook documentation locally with live reload')]
doc-serve:
    mdbook serve docs --dest-dir target/book --open

# Format all workspace code
[group('tauqe')]
[doc('Format all workspace code')]
fmt:
    cargo fmt --all

# Check formatting across all workspace targets
[group('tauqe')]
[doc('Check formatting across all workspace targets')]
fmt-check:
    cargo fmt --all -- --check
