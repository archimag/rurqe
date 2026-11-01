//! The embedded migrator: migration scripts baked into the binary.
//!
//! The file assembles the collection and stops there: it names no dialect and
//! opens no connection, because applying the collection names a backend that
//! the chapter describes in prose.
//!
//! The macro reads the artifacts the `rurqe.toml` argument declares — the
//! migration tail of this example, and nothing else — and resolves that path
//! against the manifest directory of this package, so every script reaches the
//! binary through `include_str!` and is an input of the compilation.

use rurqe::migrate::Error;
use rurqe::prelude::embed_migrations;

fn main() -> Result<(), Error> {
    // ANCHOR: embed
    let embedded = embed_migrations!("tests/fixtures/rurqe.toml")?;

    // The names, the version grammar, and the digests are the ones the
    // migration directory holds, so a malformed script stops the process at
    // startup instead of leaving a gap in the recorded history.
    println!("{} embedded migration(s)", embedded.len());
    // ANCHOR_END: embed

    Ok(())
}
