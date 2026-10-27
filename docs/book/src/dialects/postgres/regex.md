# Regular expressions

Text columns match POSIX regular expressions through four operators, gated
by the `PgTextValue` marker — `String`, `Option<String>`, `&'static str`,
`Option<&'static str>`:

| Method | SQL |
|---|---|
| `regex_match(pattern)` | `"content" ~ $1` |
| `regex_match_ci(pattern)` | `"content" ~* $1` |
| `regex_not_match(pattern)` | `"content" !~ $1` |
| `regex_not_match_ci(pattern)` | `"content" !~* $1` |

The pattern binds as one text parameter; the column stays symbolic.

## Column references stay symbolic

Because the column is rendered by the statement it belongs to, a reference
qualifies itself against the statement's relations:

```text
one relation          ("content" ~ $1)
several relations     ("docs"."content" ~ $1)
an aliased column     ("d"."content" ~ $1)
an escaped name       ("a""b" ~ $1)
```

A numeric column has no regex operators:

```rust,compile_fail
// Fails to compile: `rank` holds numbers, not text.
let _ = DocRank.regex_match("^1");
```
