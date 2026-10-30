# Stage-based schema evolution

The stage lifecycle keeps a declarative DDL state as the single source of truth
and a short tail of migration scripts on top of it. A clean environment is
initialized straight from the declarative state, an existing database is
advanced through the tail, and a checkpoint marker records the boundary below
which the history has been pruned.

The two paths converge on one schema: the declarative state describes where the
database has to end up, and the checkpoint plus the tail describes how an
existing database reaches it. Pruning old scripts below the checkpoint removes
history without rewriting a baseline, and a database older than the checkpoint
is refused with a diagnostic rather than migrated into an unknown state.

This chapter covers the declarative stage, cold start, checkpoint markers,
history pruning, and the commands that maintain the pair.