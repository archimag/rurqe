use super::*;

#[test]
fn the_primary_key_statement_matches_columns_within_their_table() {
    // A constraint name is only unique per table, so a schema-wide match on the
    // name alone folds the columns of a foreign key or a unique constraint of
    // another table into the primary key it shares the name with.
    assert!(
        POSTGRES_PRIMARY_KEYS.contains("tc.table_name = kcu.table_name"),
        "the primary key query must constrain the join to one table"
    );
}

#[test]
fn the_index_statement_reads_only_the_key_attributes() {
    assert!(
        POSTGRES_INDEXES.contains("index_row.indnkeyatts >= position.index"),
        "the index query must stop before the non-key INCLUDE columns"
    );
}

#[test]
fn the_index_statement_keeps_expression_indexes() {
    // An expression occupies a zero attribute number, which matches no
    // `pg_attribute` row, so only an outer join keeps the index in the catalog.
    assert!(
        POSTGRES_INDEXES.contains("LEFT JOIN pg_attribute attribute"),
        "the index query must keep an index declared over an expression"
    );
    assert!(
        !POSTGRES_INDEXES.contains("NOT (0 = ANY"),
        "the index query must not drop the indexes it cannot name by column"
    );
}

#[test]
fn the_index_statement_reads_the_index_attributes() {
    assert!(POSTGRES_INDEXES.contains("access_method.amname"));
    assert!(POSTGRES_INDEXES.contains("pg_get_expr(index_row.indpred"));
    assert!(POSTGRES_INDEXES.contains("pg_get_expr(index_row.indexprs"));
}

#[test]
fn the_foreign_key_statement_reads_the_referential_actions() {
    assert!(POSTGRES_FOREIGN_KEYS.contains("constraint_row.confdeltype"));
    assert!(POSTGRES_FOREIGN_KEYS.contains("constraint_row.confupdtype"));
}

#[test]
fn the_foreign_key_statement_reads_the_referenced_schema() {
    // A reference held inside the introspected schema is reported without a
    // schema of its own, so the scratch schema of a stage verification and the
    // live schema it is compared against record the same catalog for the same
    // DDL; only a reference that crosses into another schema names one.
    assert!(
        POSTGRES_FOREIGN_KEYS.contains("referenced_namespace.nspname = $1"),
        "the foreign key query must report only a cross-schema reference"
    );
}
#[test]
fn the_check_statement_reads_only_check_constraints() {
    assert!(
        POSTGRES_CHECKS.contains("constraint_row.contype = 'c'"),
        "the check query must read the check constraints alone"
    );
    assert!(POSTGRES_CHECKS.contains("pg_get_expr"));
}

#[test]
fn the_schema_neighbour_statement_ignores_a_populated_selection() {
    assert!(
        POSTGRES_SCHEMA_NEIGHBOURS.contains("selected.relations = 0"),
        "the statement only answers for a selection that holds no relation"
    );
    assert!(
        POSTGRES_SCHEMA_NEIGHBOURS.contains("table_name != '__rurqe_schema_history'"),
        "the history table never makes a selection look populated"
    );
    assert!(
        POSTGRES_SCHEMA_NEIGHBOURS.contains("NOT IN ('pg_catalog', 'information_schema')"),
        "a system schema is never proposed as a target"
    );
}

#[test]
fn the_trigger_statement_skips_the_internal_triggers() {
    assert!(
        POSTGRES_TRIGGERS.contains("NOT trigger_row.tgisinternal"),
        "the trigger query must skip the triggers backing a constraint"
    );
}

#[test]
fn a_undecodable_column_names_the_query_and_the_column() {
    let error = column_error("columns", "column_default", "unexpected type");

    let message = error.to_string();
    assert!(
        message.contains("`column_default`"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("PostgreSQL columns query"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("unexpected type"),
        "unexpected message: {message}"
    );
}
