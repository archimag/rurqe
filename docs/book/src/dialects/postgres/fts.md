# Full-text search

A text column matches a `tsquery` through `matches_tsquery`, gated by the
`PgTextSearchValue` marker — the same value kinds as the regex operators:
`String`, `Option<String>`, `&'static str`, `Option<&'static str>`.

```rust
let hits = select_from(docs)
    .select((docs.id, docs.title))
    .where_(docs.content.matches_tsquery("english & rust"))
    .fetch_all(&mut conn)?;
```

The predicate renders as the match operator over the query constructor:

```sql
("content" @@ to_tsquery($1))
```

A numeric column has no full-text operators — the marker diagnostic says the
operators match text:

```rust,compile_fail
// Fails to compile: full-text search matches text.
let _ = DocRank.matches_tsquery("rust");
```

## Query constructors

Three helpers render as function calls with one bound text parameter each,
usable wherever an `Expr<String>` is accepted:

```rust
to_tsvector("fat cats");      // to_tsvector($1)
to_tsquery("cat & rat");       // to_tsquery($1)
plainto_tsquery("the fat cat"); // plainto_tsquery($1)
```
