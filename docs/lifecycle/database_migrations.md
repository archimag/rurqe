# Database migrations

The schema of a database is not written once. It changes with the code that uses
it, and every change has to reach the databases that already exist and already
hold data, without losing that data. A migration is the artifact that carries one
such change: a forward-only SQL script kept in the repository, applied in order,
and recorded when it succeeds, so the same change reaches every environment
exactly once.

RURQE supports two lifecycles for that job. Sequential migrations describe the
schema as the sum of the scripts applied to it. Stage-based schema evolution
describes the schema as a declarative directory and keeps the scripts as a short
tail that advances a database written before the current description. This
chapter states what each route is good at, and when a project moves from the
first to the second.

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

Both routes answer all four. They differ in what each one costs as the number of
releases grows.

## Sequential migrations

A directory of scripts applied in order is the traditional shape. A versioned
script carries a version and runs once; a repeatable script carries no version
and runs again whenever its contents change. The applied entry is recorded in the
history table together with the digest of the script, so an edited script is
detected and a failed attempt stays visible until it is repaired.

What the route gives a project:

- an exact audit trail: every change that reached a database is recorded with its
  digest and the moment it ran;
- a deterministic result: the scripts form one sequence, and every environment
  that applies the sequence ends on the same schema;
- a non-destructive upgrade: a database is never recreated, only advanced.

The costs follow the length of the history rather than the size of the schema:

- the schema is written nowhere, so learning what a table consists of means
  reading every script that ever touched it, in version order;
- a clean environment replays the whole sequence, so reaching the current schema
  in continuous integration costs what the number of releases costs;
- the history only grows: an applied script is bound to its recorded digest, so
  editing it is a refusal, while deleting it removes the only description of that
  change for a database that has not applied it yet;
- ordering constrains the design: a correction of an earlier script has to be a
  new script, so the directory accumulates steps whose only purpose is to
  preserve the order the history was written in.

## Stage-based schema evolution

The stage lifecycle separates the two jobs the directory carries at once. A
declarative directory holds the complete DDL of the target schema, the migrations
directory keeps only the active tail, and a checkpoint marker records the boundary
where the two meet.

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

The stage lifecycle is an addition rather than a replacement. Both routes end on
one schema and share one history table, one script naming grammar, and one
checksum algorithm, so a project adopts the stage without rewriting a single entry
that already ran, and a project whose history stays short keeps the plain
directory and loses nothing.

## Moving from one to the other

```text
sequential migrations
    V1 ──▶ V2 ──▶ V3 ──▶ V4         a clean database replays every script

stage-based schema evolution
    schema/                         a clean database starts at the stage
    boundary + V4 ──▶ V5            an existing database advances through the tail
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
