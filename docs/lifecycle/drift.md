# Schema drift verification

Drift verification answers one question: does the state the repository
describes still equal the state the repository materials produce? The
declarative stage and the checkpoint plus migration tail are both replayed into
ephemeral databases, their catalogs are compared, and any difference is
reported as the exact relation, column, index, or constraint that diverges. The
schema hash embedded in the generated code is compared against the live catalog
in the same pass.

Nothing about this check depends on a database that already carries the schema:
the comparison runs on throwaway targets, so it belongs in continuous
integration, where a divergence is caught before it is merged.

This chapter covers the catalog snapshot, the verification workflow, the
equivalence the two sides of the lifecycle have to satisfy, and the diagnostics
a mismatch produces.