# Schema code generation

RURQE derives its schema markers from a live database catalog instead of from
hand-written declarations. The generator reads the catalog and emits a Rust
module declaring one marker per table and one marker per column, each carrying
the Rust type the column decodes into, so a query addresses a schema the
database actually has rather than a schema somebody remembered to update.

A normalized snapshot of the same catalog is written beside the generated code.
The snapshot is the artifact later verification compares a database against, and
the generated module embeds a hash of it, so the code and the snapshot are two
views of one catalog and cannot drift apart unnoticed.

This chapter covers the introspection targets, the generator configuration, the
shape of the generated code, and the custom type overrides that map a database
type onto a domain type of the application.