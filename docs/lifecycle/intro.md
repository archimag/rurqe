# Database migrations

The schema of a database is not written once. It changes with the code that uses
it, and every change has to reach the databases that already exist and already
hold data, without losing that data. A migration is the artifact that carries one
such change: a forward-only SQL script kept in the repository, applied in order,
and recorded when it succeeds, so the same change reaches every environment
exactly once.

Two questions shape every tool that manages that job: what the repository states
about the schema, and how an existing database is brought to it. The answers the
industry settled on fall into two paradigms — a history of deltas, and a declared
target state. RURQE runs both on one engine. This chapter states what each
paradigm is good at, and when a project moves from the first to the second; the
practical chapters that follow show how either route is written.

## The problem every lifecycle solves

- A database that holds data cannot be recreated from the current code. Its
  structure arrived through a sequence of changes, and each one has to be applied
  to every environment that already exists.
- A schema change ships together with the code that depends on it. Applying one
  without the other breaks either the deployment or the query it issues.
- Environments do not move in step. Development, integration, and production each
  sit at a different point of the same sequence, and a database has to report the
  point it stands at.
- An applied change is a fact. Running it again must not repeat its effect, and
  editing it afterwards must be detected instead of silently accepted.

Both paradigms answer all four. They differ in what each one costs as the number
of releases grows.

## A history of deltas

The first paradigm states the schema as the sum of the changes applied to it. The
repository holds an ordered directory of scripts, and the tool applies the ones
the target database has not seen yet. Flyway and Liquibase established this shape
and the vocabulary around it: a versioned script runs once, a repeatable script
runs again whenever its contents change, and the history table records every
entry together with the digest of the script that produced it.

What the paradigm gives a project:

- an exact audit trail: every change that reached a database is recorded with its
  digest and the moment it ran;
- a deterministic result: the scripts form one sequence, and every environment
  that applies the sequence ends on the same schema;
- a non-destructive upgrade: a database is never recreated, only advanced.

What it costs, as the number of releases grows:

- the schema is written nowhere, so learning what a table consists of means
  reading every script that ever touched it, in version order;
- a clean environment replays the whole sequence, so reaching the current schema
  in continuous integration costs what the number of releases costs;
- the history only grows: an applied script is bound to its recorded digest, so
  editing it is a refusal, while deleting it removes the only description of that
  change for a database that has not applied it yet;
- ordering constrains the design: a correction of an earlier script has to be a
  new script, so the directory accumulates steps whose only purpose is to preserve
  the order the history was written in.

## A declared target state

The second paradigm states the schema as it should be rather than as the sum of
what was done to it. The repository holds a description of the target, and the
tooling computes how a live database differs from it. Django's model migrations,
Prisma's schema file, and Atlas's declarative configuration each work this way.

What the paradigm gives a project:

- one readable description of the schema, which is what a developer answers a
  question about a table from;
- a clean environment that reaches the target directly, so the cold start costs
  what the schema costs rather than what the history costs;
- one description the query layer can be generated from, so code and database are
  derived from the same source.

What it costs:

- a computed plan is a guess about intent: dropping a column that a rename should
  have preserved is a decision the description cannot express, and a database that
  holds data pays for a wrong guess in data;
- the description stays true only while nothing else changes the database, so any
  intervention outside the tooling desynchronizes it until the next comparison;
- a data transformation has no natural home in the description, because a data
  migration is a procedure rather than a state.

## The model RURQE implements

RURQE does not choose between the paradigms. Both run on one engine, share one
history table, one script naming grammar, and one checksum algorithm, so a project
adopts the declarative route without rewriting a single entry the sequential route
already recorded.

### Sequential migrations

A directory of versioned and repeatable scripts applied in order. A versioned
script carries a version and runs once; a repeatable script carries no version and
runs again whenever its contents change. Every applied entry lands in the history
table with the SHA-256 digest of its script, so an edited script is detected and a
failed attempt stays visible until it is repaired. `migrate up` executes the
pending scripts, `migrate status` reports the point a database stands at,
`migrate repair` restores the recorded state after a failure, and `baseline`
records an existing database as the starting point without replaying the history
that produced it.

The route is complete on its own and stays correct for as long as a project wants
it.

### Stage-based schema evolution

The stage lifecycle separates the two jobs the migrations directory carries at
once. A declarative `rurqe/schema/` directory holds the complete DDL of the target
schema, the `rurqe/migrations/` directory keeps only the active tail, and a
checkpoint marker records the boundary where the two meet. Both directories sit
beneath the artifact directory `rurqe/`, which a single key of `rurqe.toml`
relocates.

- A clean database is initialised from the stage in one pass and records the stage
  version as its starting point, so a container reaches the current schema in the
  time the stage takes rather than in the time the history takes.
- A database that holds data is advanced through the tail alone, script by script,
  exactly as before. Nothing that carries data is recreated.
- The two descriptions are proven equal rather than assumed to be: replaying the
  boundary together with the tail on one side, and the stage on the other, then
  comparing the resulting catalogs, answers whether they still converge.
- Versioned scripts below the boundary are deleted once every deployed database
  has passed it, because the boundary snapshot folds them in. No squash is
  recorded and no baseline is rewritten.

## Moving from one to the other

```text
sequential migrations
    V1 -> V2 -> V3 -> V4          a clean database replays every script

stage-based schema evolution
    rurqe/schema/                 a clean database starts at the stage
    boundary + V4 -> V5           an existing database advances through the tail
```

Because the costs that motivate the second route are the costs of a long history,
the decision follows from measurement rather than preference. The stage pays for
itself once:

- answering what a table consists of requires opening more than a handful of
  scripts;
- the cold start of a container or a new development machine has become the
  slowest step of the pipeline;
- the directory is dominated by corrections, pairs of scripts whose only purpose
  is to preserve the order the history was written in;
- the history can no longer be shortened without a baseline recorded by hand.

The moment is a release every deployed database has already passed. That release
becomes the boundary, the stage states the schema it produced, and the scripts
below it are pruned. Nothing has to be decided in advance: the sequential route
stays correct for as long as a project wants it, and the history recorded under it
remains valid after the move.
