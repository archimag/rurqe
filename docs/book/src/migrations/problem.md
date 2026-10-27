# The crisis of traditional migrations in the AI era

The traditional model (Flyway, Liquibase) was built around a human who remembers the history of the project. A script `V051__add_credits.sql` is a delta whose meaning is only recoverable together with the previous fifty. While the history is read by the person who wrote it, this works. As soon as the history is read by an LLM or by a new engineer, the model breaks for four independent reasons.

## 1. Context instead of state

To understand "what table `users` looks like right now", an agent has to read and hold the whole `V001 … V050` chain in context: every `ALTER TABLE`, every `DROP COLUMN`, every renamed type. That is thousands of lines of which only a handful are current. A mistake in such reading is not diagnosed: a wrongly reconstructed schema looks exactly as plausible as a correct one.

The declarative stage (`schema/users.sql`) turns the task from "reconstruct the state from deltas" into "read the state". The token budget stops depending on the age of the project.

## 2. Initialisation time

In the traditional model a clean database (CI runner, local development, container) is obliged to replay the entire historical tail: N scripts, N transactions, N history rows. The time grows linearly with the age of the project and has no relation to its current complexity.

A cold start from the stage applies only the current DDL files, in a single transaction (`cold_start` batches consecutive transactional scripts into one transaction). The cost of initialisation is proportional to the current schema, not to its history.

## 3. Squashing as a risky operation

The only way to shorten history in the traditional model is a squash: rewrite the old migrations into one new one and move `baseline`. This is a dangerous operation:

- already deployed databases remember the old checksums, so a squash requires external coordination across all environments;
- `baseline` moves the version below which migrations are ignored, and an error at that boundary silently skips required scripts;
- rolling a squash back is another squash.

RURQE replaces the squash with pruning: `prune --before <version>` deletes files the checkpoint has already absorbed, without rewriting a single checksum and without touching history rows.

## 4. Divergence between declaration and execution

Even a carefully maintained history does not guarantee that `schema/` (or the code-generated `schema.rs`) matches what actually results from replaying the tail. Classic tools offer no such check at all.

Two RURQE mechanisms close this gap:

- `cargo db-verify` compares the live catalog against `schema.lock` (the codegen drift check);
- `cargo rurqe stage verify` materialises both sides of the equation in separate scratch databases and compares their catalogs bit-for-bit (the Golden Equation).

## What is not lost

Giving up the linear history as the *only* source of truth does not mean giving up sequential migrations. A live database is still advanced forward only by `V...` scripts, because data cannot be "declared" — it can only be transformed. The stage answers "what should be", the tail answers "how to move the live state into it".

| Question | Traditional model | Stage + Tail |
|---|---|---|
| What should the schema contain? | derived from the history | `schema/*.sql` |
| How to raise a clean database? | replay the history | apply the stage |
| How to advance a live database? | apply the tail | apply the tail |
| How to shorten the history? | squash | `prune` |
| How to make sure everything agrees? | manually | `stage verify` |

The next page is [Two modes and how to choose](modes.md).
