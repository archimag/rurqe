# Reusable Query Patterns (macro_rules!)

A query is an ordinary Rust expression, so a repeated query shape is factored with the tools Rust already offers. This chapter documents two application-side patterns: a `macro_rules!` macro that generates repository functions from one projection list, and a named `dto!` row with a conversion into a domain type. Neither pattern is a library API: the library adds no macro, trait, or builder method for them, and the code lives in the application.

## Why a macro

A projection written with `cols!` or `select!` produces an anonymous record type. A function cannot name that type in its signature, so a helper that returns the rows of such a statement has nothing to return. A `macro_rules!` expansion avoids the problem: the statement is assembled and executed inside the body of a generated function, the anonymous record is converted into a domain type in the same body, and only the domain type appears in the signature.

## A macro that generates a pair of repository functions

The macro below takes the domain type, the names of the two functions, one projection list, and one conversion closure. It generates a point lookup by key and a list with optional filters:

```rust,no_run
{{#rustdoc_include ../examples/reusable_queries.rs:repository_macro}}
```

- The lookup ends in `fetch_optional`, so a missing key is `None` and not an error.
- The list ends in `fetch_all`. The `.when(...)` call applies a predicate for a runtime flag, and `.if_some(...)` applies one only when a value is present. A filter that is not requested adds no condition to the statement.
- The projection list is forwarded as token trees into `cols!`, so the macro never restates the column list.
- The conversion closure receives the anonymous record inside the function body, so the record never crosses the boundary of the expansion.

One invocation declares both functions:

```rust,no_run
{{#rustdoc_include ../examples/reusable_queries.rs:repository_instance}}
```

The closure reads the fields by name: `user_id`, `user_name`, and `user_email` are the field names derived from the column markers in the projection. A field that the projection does not list does not exist on the record, so a conversion that reads it is rejected by the compiler.

The generated functions have ordinary signatures: `find_customer(pool, key)` returns `Result<Option<Customer>>`, and `list_customers(pool, managed_only, name)` returns `Result<Vec<Customer>>`. The queries inside them are checked like any other query written by hand.

## Hygiene: the macro generates functions, not types

Every expansion of `cols!` or `select!` produces a record type of its own. Two expansions that list the same columns still produce two distinct types, so a macro cannot declare one record and share it between the lookup and the list. This is why the pattern generates functions and keeps the record inside them: the lookup and the list each use their own record, and the single conversion closure is expanded once into each body. Both bodies type-check the closure against their own record, which has the same fields because it comes from the same projection list.

A type that has to be named, stored, or passed between modules is declared explicitly instead.

## A named row for module boundaries

`dto!` declares a row as a named struct whose field types are read from the schema:

```rust,no_run
{{#rustdoc_include ../examples/reusable_queries.rs:dto_row}}
```

The struct is a regular type, so it can appear in signatures and in other declarations. A conversion into the domain type is an ordinary `From` implementation, and the domain type stays independent of the schema:

```rust,no_run
{{#rustdoc_include ../examples/reusable_queries.rs:dto_conversion}}
```

The field names of a `dto!` declaration are also the keys of the document of a nested collection, so the same declaration decodes the rows of a nested projection. The decoded rows convert into domain values with the same `From` implementation:

```rust,no_run
{{#rustdoc_include ../examples/reusable_queries.rs:dto_document}}
```

## Choosing between the two patterns

- Use the macro when the query shape repeats and the result is converted into a domain type inside the function. The macro keeps the statement, its filters, and the conversion in one place.
- Use a named `dto!` when the row has to be named: in a signature, across a module boundary, or as the element type of a nested collection. A `dto!` accepts schema columns only, so a computed item is projected through `select!` or `cols!` and converted inside a function.

## Operational constraints

- A macro defined with `macro_rules!` must appear before its first use in the same module, and a macro that generates items is invoked at module scope.
- Function names are passed to the macro explicitly, because `macro_rules!` cannot build an identifier from fragments.
- The macro forwards the projection list unchanged, so the rules of `cols!` apply: a field name belongs to exactly one projected item, and a duplicate is refused while the macro expands.
