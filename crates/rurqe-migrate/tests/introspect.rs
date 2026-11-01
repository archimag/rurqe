//! Integration tests of the engine-neutral catalog vocabulary.
//!
//! Every test describes rows the way an engine reports them and asserts the
//! catalog the shared assembly builds out of them, so the vocabulary stays
//! independent of the driver that produced the rows.

use rurqe_migrate::catalog::{ForeignKeyAction, TypeKind};
use rurqe_migrate::introspect::{
    catalog_from_metadata, catalog_from_metadata_with_sections, map_sql_type_to_kind,
    map_sqlite_declared_type_to_kind, RawCatalogSections, RawCheckMetadata, RawColumnMetadata,
    RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata, RawPrimaryKeyMetadata,
    RawTableMetadata, RawTriggerMetadata, RawViewMetadata,
};

#[test]
fn metadata_assembly_builds_catalog() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "email".to_owned(),
            data_type: "character varying".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: None,
            udt_schema: None,
        },
    ];
    let pks = vec![RawPrimaryKeyMetadata {
        schema: None,
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
    }];

    let catalog = catalog_from_metadata(&tables, &cols, &pks, &[], &[], &[]);

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");
    assert_eq!(catalog.tables[0].columns.len(), 2);
    assert_eq!(catalog.tables[0].columns[0].type_kind, TypeKind::Int64);
    assert_eq!(catalog.tables[0].columns[1].type_kind, TypeKind::Text);
    assert_eq!(catalog.tables[0].primary_key, vec!["id"]);
}

#[test]
fn composite_foreign_key_rows_fold_into_one_constraint() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "order_items".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
            udt_schema: None,
        },
    ];
    let fks = vec![
        RawForeignKeyMetadata {
            schema: None,
            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "id".to_owned(),
            referenced_schema: None,
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::NoAction,
        },
        RawForeignKeyMetadata {
            schema: None,
            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "line_no".to_owned(),
            referenced_schema: None,
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::NoAction,
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &fks, &[], &[]);

    let table = &catalog.tables[0];
    assert_eq!(table.foreign_keys.len(), 1);
    assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
    assert_eq!(
        table.foreign_keys[0].referenced_columns,
        vec!["id", "line_no"]
    );
}

#[test]
fn a_cross_schema_reference_keeps_its_referenced_schema() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "invoices".to_owned(),
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,
        name: "invoices_country_fkey".to_owned(),
        table_name: "invoices".to_owned(),
        column_name: "country_code".to_owned(),
        referenced_table: "countries".to_owned(),
        referenced_column: "code".to_owned(),
        referenced_schema: Some("shared".to_owned()),
        on_delete: ForeignKeyAction::NoAction,
        on_update: ForeignKeyAction::NoAction,
    }];

    let catalog = catalog_from_metadata(&tables, &[], &[], &fks, &[], &[]);
    let foreign_key = &catalog.tables[0].foreign_keys[0];

    assert_eq!(foreign_key.referenced_schema.as_deref(), Some("shared"));
}

#[test]
fn a_reference_inside_the_schema_stays_schema_less() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "invoices".to_owned(),
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,
        name: "invoices_country_fkey".to_owned(),
        table_name: "invoices".to_owned(),
        column_name: "country_code".to_owned(),
        referenced_table: "countries".to_owned(),
        referenced_column: "code".to_owned(),
        referenced_schema: None,
        on_delete: ForeignKeyAction::NoAction,
        on_update: ForeignKeyAction::NoAction,
    }];

    let catalog = catalog_from_metadata(&tables, &[], &[], &fks, &[], &[]);

    assert!(catalog.tables[0].foreign_keys[0].referenced_schema.is_none());
}

#[test]
fn sql_type_mapping_covers_builtin_kinds() {
    assert_eq!(map_sql_type_to_kind("BOOLEAN"), TypeKind::Boolean);
    assert_eq!(map_sql_type_to_kind("INTEGER"), TypeKind::Int32);
    assert_eq!(map_sql_type_to_kind("bigint"), TypeKind::Int64);
    assert_eq!(map_sql_type_to_kind("double precision"), TypeKind::Float64);
    assert_eq!(
        map_sql_type_to_kind("timestamp with time zone"),
        TypeKind::Timestamp
    );
    assert_eq!(map_sql_type_to_kind("uuid"), TypeKind::Uuid);
    assert_eq!(map_sql_type_to_kind("jsonb"), TypeKind::Json);
    assert_eq!(
        map_sql_type_to_kind("inet"),
        TypeKind::Other("inet".to_owned())
    );
}

#[test]
fn type_parameters_do_not_hide_the_builtin_kinds() {
    assert_eq!(map_sql_type_to_kind("varchar(255)"), TypeKind::Text);
    assert_eq!(
        map_sql_type_to_kind("character varying(80)"),
        TypeKind::Text
    );
    assert_eq!(map_sql_type_to_kind("smallint"), TypeKind::Int32);
    assert_eq!(map_sql_type_to_kind("int2"), TypeKind::Int32);
}

#[test]
fn numeric_and_decimal_types_map_to_the_numeric_kind() {
    assert_eq!(map_sql_type_to_kind("numeric"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("NUMERIC(18,4)"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("decimal"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("DECIMAL(10, 2)"), TypeKind::Numeric);

    assert_eq!(
        map_sqlite_declared_type_to_kind("NUMERIC"),
        TypeKind::Numeric
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("NUMERIC(10,2)"),
        TypeKind::Numeric
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("DECIMAL(10,2)"),
        TypeKind::Numeric
    );
}

#[test]
fn unknown_types_keep_their_declared_precision() {
    let interval = map_sql_type_to_kind("interval(6)");
    assert_eq!(interval, TypeKind::Other("interval(6)".to_owned()));

    let custom = map_sqlite_declared_type_to_kind("CUSTOM(10,2)");
    assert_eq!(custom, TypeKind::Other("custom(10,2)".to_owned()));
}

#[test]
fn array_spellings_map_to_their_element_type() {
    assert_eq!(
        map_sql_type_to_kind("integer[]"),
        TypeKind::Array(Box::new(TypeKind::Int32))
    );
    assert_eq!(
        map_sql_type_to_kind("character varying(80)[]"),
        TypeKind::Array(Box::new(TypeKind::Text))
    );
    assert_eq!(
        map_sql_type_to_kind("numeric(18,4)[]"),
        TypeKind::Array(Box::new(TypeKind::Numeric))
    );
}

#[test]
fn postgres_array_columns_map_to_their_element_type() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "posts".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "posts".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "posts".to_owned(),
            column_name: "tags".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("_text".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "posts".to_owned(),
            column_name: "moods".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("_post_status".to_owned()),
            udt_schema: None,
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,
            name: "post_status".to_owned(),
            variant: "draft".to_owned(),
        },
        RawEnumMetadata {
            schema: None,
            name: "post_status".to_owned(),
            variant: "published".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &[], &[], &enums);
    let table = &catalog.tables[0];
    let column = |name: &str| {
        table
            .columns
            .iter()
            .find(|column| column.name == name)
            .expect("the column is introspected")
    };

    assert_eq!(
        column("tags").type_kind,
        TypeKind::Array(Box::new(TypeKind::Text))
    );
    assert!(!column("tags").nullable);
    assert_eq!(
        column("moods").type_kind,
        TypeKind::Array(Box::new(TypeKind::Enum("post_status".to_owned())))
    );
    assert!(column("moods").nullable);
}

#[test]
fn sqlite_declared_types_follow_the_affinity_rules() {
    assert_eq!(map_sqlite_declared_type_to_kind("INTEGER"), TypeKind::Int64);
    assert_eq!(
        map_sqlite_declared_type_to_kind("UNSIGNED BIG INT"),
        TypeKind::Int64
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("VARCHAR(255)"),
        TypeKind::Text
    );
    assert_eq!(map_sqlite_declared_type_to_kind("CLOB"), TypeKind::Text);
    assert_eq!(map_sqlite_declared_type_to_kind("BLOB"), TypeKind::Bytes);
    assert_eq!(map_sqlite_declared_type_to_kind(""), TypeKind::Bytes);
    assert_eq!(map_sqlite_declared_type_to_kind("REAL"), TypeKind::Float64);
    assert_eq!(
        map_sqlite_declared_type_to_kind("DOUBLE PRECISION"),
        TypeKind::Float64
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("BOOLEAN"),
        TypeKind::Boolean
    );
    assert_eq!(map_sqlite_declared_type_to_kind("UUID"), TypeKind::Uuid);
    assert_eq!(
        map_sqlite_declared_type_to_kind("DATETIME"),
        TypeKind::Timestamp
    );
    assert_eq!(map_sqlite_declared_type_to_kind("DATE"), TypeKind::Date);
}

#[test]
fn index_rows_fold_into_composite_indexes() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let indexes = vec![
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_tenant_email_idx".to_owned(),
            column_name: Some("tenant".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_tenant_email_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
    ];

    let catalog = catalog_from_metadata(&tables, &[], &[], &[], &indexes, &[]);

    let table = &catalog.tables[0];
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.indexes[0].name, "users_tenant_email_idx");
    assert_eq!(table.indexes[0].columns, vec!["tenant", "email"]);
    assert!(!table.indexes[0].unique);
}

#[test]
fn an_index_expression_keeps_only_the_functional_terms() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let indexes = vec![
        // PostgreSQL renders the expression list of a mixed index with `NULL`
        // standing in for the key attribute that is a plain column.
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_email_lower_name_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: false,
            method: Some("btree".to_owned()),
            predicate: None,
            expression: Some("NULL, lower(name)".to_owned()),
        },
        // An index computing no expression keeps none, even when the rendering it
        // reports carries nothing but placeholders.
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_email_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: true,
            method: None,
            predicate: None,
            expression: Some("NULL".to_owned()),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &[], &[], &[], &indexes, &[]);
    let index = |name: &str| {
        catalog.tables[0]
            .indexes
            .iter()
            .find(|index| index.name == name)
            .expect("the index is folded")
    };

    assert_eq!(index("users_email_lower_name_idx").columns, vec!["email"]);
    assert_eq!(
        index("users_email_lower_name_idx").expression.as_deref(),
        Some("lower(name)")
    );
    assert!(index("users_email_idx").expression.is_none());
}

#[test]
fn postgres_enum_metadata_assembly_maps_enum_columns() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "posts".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "posts".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "posts".to_owned(),
            column_name: "status".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("post_status".to_owned()),
            udt_schema: None,
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,
            name: "post_status".to_owned(),
            variant: "draft".to_owned(),
        },
        RawEnumMetadata {
            schema: None,
            name: "post_status".to_owned(),
            variant: "published".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &[], &[], &enums);

    assert_eq!(catalog.enums.len(), 1);
    assert_eq!(catalog.enums[0].name, "post_status");
    assert_eq!(catalog.enums[0].variants, vec!["draft", "published"]);

    let status_col = &catalog.tables[0].columns[1];
    assert_eq!(status_col.type_kind, TypeKind::Enum("post_status".to_owned()));
}

#[test]
fn a_domain_column_is_reported_as_the_type_it_is_declared_over() {
    // The columns query resolves a domain through `pg_type.typbasetype`, so the
    // catalog receives the base type name and never the domain itself.
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let columns = vec![RawColumnMetadata {
        schema: None,
        table_name: "users".to_owned(),
        column_name: "email".to_owned(),
        data_type: "USER-DEFINED".to_owned(),
        is_nullable: false,
        column_default: None,
        udt_name: Some("text".to_owned()),
        udt_schema: None,
    }];

    let catalog = catalog_from_metadata(&tables, &columns, &[], &[], &[], &[]);

    assert_eq!(catalog.tables[0].columns[0].type_kind, TypeKind::Text);
}

#[test]
fn same_named_objects_of_two_schemas_assemble_independently() {
    let tables = vec![
        RawTableMetadata {
            schema: None,
            name: "users".to_owned(),
        },
        RawTableMetadata {
            schema: Some("auth".to_owned()),
            name: "users".to_owned(),
        },
    ];
    let columns = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "integer".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("int4".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: Some("auth".to_owned()),
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "legacy".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: Some("archive".to_owned()),
        },
    ];
    let pks = vec![
        RawPrimaryKeyMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
        },
        RawPrimaryKeyMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
        },
    ];
    let fks = vec![
        RawForeignKeyMetadata {
            schema: None,
            name: "users_mood_fkey".to_owned(),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            referenced_table: "moods".to_owned(),
            referenced_column: "name".to_owned(),
            referenced_schema: None,
            on_delete: ForeignKeyAction::Cascade,
            on_update: ForeignKeyAction::NoAction,
        },
        RawForeignKeyMetadata {
            schema: Some("auth".to_owned()),
            name: "users_mood_fkey".to_owned(),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            referenced_table: "moods".to_owned(),
            referenced_column: "label".to_owned(),
            referenced_schema: Some("auth".to_owned()),
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::Restrict,
        },
    ];
    let indexes = vec![
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_mood_idx".to_owned(),
            column_name: Some("mood".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
        RawIndexMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            index_name: "users_mood_idx".to_owned(),
            column_name: Some("mood".to_owned()),
            unique: true,
            method: None,
            predicate: Some("mood IS NOT NULL".to_owned()),
            expression: None,
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,
            name: "mood".to_owned(),
            variant: "happy".to_owned(),
        },
        RawEnumMetadata {
            schema: None,
            name: "mood".to_owned(),
            variant: "sad".to_owned(),
        },
        RawEnumMetadata {
            schema: Some("auth".to_owned()),
            name: "mood".to_owned(),
            variant: "calm".to_owned(),
        },
    ];
    let checks = vec![
        RawCheckMetadata {
            schema: None,
            table_name: "users".to_owned(),
            name: "users_mood_check".to_owned(),
            expression: "mood <> 'banned'".to_owned(),
        },
        RawCheckMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            name: "users_mood_check".to_owned(),
            expression: "mood <> 'calm'".to_owned(),
        },
    ];
    let views = vec![
        RawViewMetadata {
            schema: None,
            name: "mood_counts".to_owned(),
            definition: "SELECT mood FROM users".to_owned(),
        },
        RawViewMetadata {
            schema: Some("auth".to_owned()),
            name: "mood_counts".to_owned(),
            definition: "SELECT mood FROM users WHERE mood IS NOT NULL".to_owned(),
        },
    ];
    let triggers = vec![
        RawTriggerMetadata {
            schema: None,
            name: "users_touch".to_owned(),
            table_name: "users".to_owned(),
            definition: "EXECUTE FUNCTION touch_public()".to_owned(),
        },
        RawTriggerMetadata {
            schema: Some("auth".to_owned()),
            name: "users_touch".to_owned(),
            table_name: "users".to_owned(),
            definition: "EXECUTE FUNCTION touch_auth()".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata_with_sections(
        &tables,
        &columns,
        &pks,
        &fks,
        &indexes,
        &enums,
        &RawCatalogSections {
            checks: &checks,
            views: &views,
            triggers: &triggers,
        },
    );

    assert_eq!(catalog.tables.len(), 2);
    let public = catalog
        .tables
        .iter()
        .find(|table| table.schema.is_none())
        .expect("the primary schema keeps its table");
    let auth = catalog
        .tables
        .iter()
        .find(|table| table.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its table");

    assert_eq!(public.name, "users");
    assert_eq!(auth.name, "users");
    assert_eq!(public.columns.len(), 2);
    assert_eq!(auth.columns.len(), 3);

    let public_id = public
        .columns
        .iter()
        .find(|column| column.name == "id")
        .expect("the primary table declares the column");
    let auth_id = auth
        .columns
        .iter()
        .find(|column| column.name == "id")
        .expect("the secondary table declares the column");
    assert_eq!(public_id.type_kind, TypeKind::Int64);
    assert!(!public_id.nullable);
    assert_eq!(auth_id.type_kind, TypeKind::Int32);
    assert!(auth_id.nullable);

    let public_mood = public
        .columns
        .iter()
        .find(|column| column.name == "mood")
        .expect("the primary table declares the column");
    let auth_mood = auth
        .columns
        .iter()
        .find(|column| column.name == "mood")
        .expect("the secondary table declares the column");
    assert_eq!(public_mood.type_kind, TypeKind::Enum("mood".to_owned()));
    assert_eq!(auth_mood.type_kind, TypeKind::Enum("mood".to_owned()));

    let auth_legacy = auth
        .columns
        .iter()
        .find(|column| column.name == "legacy")
        .expect("the secondary table declares the column");
    assert_eq!(auth_legacy.type_kind, TypeKind::Other("mood".to_owned()));

    assert_eq!(public.primary_key, vec!["id"]);
    assert_eq!(auth.primary_key, vec!["mood"]);

    assert_eq!(public.foreign_keys.len(), 1);
    assert_eq!(auth.foreign_keys.len(), 1);
    assert_eq!(public.foreign_keys[0].columns, vec!["mood"]);
    assert_eq!(public.foreign_keys[0].on_delete, ForeignKeyAction::Cascade);
    assert!(public.foreign_keys[0].referenced_schema.is_none());
    assert_eq!(auth.foreign_keys[0].referenced_columns, vec!["label"]);
    assert_eq!(auth.foreign_keys[0].on_update, ForeignKeyAction::Restrict);
    assert_eq!(auth.foreign_keys[0].referenced_schema.as_deref(), Some("auth"));

    assert_eq!(public.checks.len(), 1);
    assert_eq!(auth.checks.len(), 1);
    assert_eq!(public.checks[0].expression, "mood <> 'banned'");
    assert_eq!(auth.checks[0].expression, "mood <> 'calm'");

    assert_eq!(public.indexes.len(), 1);
    assert_eq!(auth.indexes.len(), 1);
    assert_eq!(public.indexes[0].columns, vec!["mood"]);
    assert!(!public.indexes[0].unique);
    assert!(public.indexes[0].predicate.is_none());
    assert!(auth.indexes[0].unique);
    assert_eq!(auth.indexes[0].predicate.as_deref(), Some("mood IS NOT NULL"));

    assert_eq!(catalog.enums.len(), 2);
    let public_mood_type = catalog
        .enums
        .iter()
        .find(|definition| definition.schema.is_none())
        .expect("the primary schema keeps its enum type");
    assert_eq!(public_mood_type.variants, vec!["happy", "sad"]);
    let auth_mood_type = catalog
        .enums
        .iter()
        .find(|definition| definition.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its enum type");
    assert_eq!(auth_mood_type.variants, vec!["calm"]);

    let public_view = catalog
        .views
        .iter()
        .find(|view| view.schema.is_none())
        .expect("the primary schema keeps its view");
    assert_eq!(public_view.definition, "SELECT mood FROM users");
    let auth_view = catalog
        .views
        .iter()
        .find(|view| view.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its view");
    assert_eq!(
        auth_view.definition,
        "SELECT mood FROM users WHERE mood IS NOT NULL"
    );

    let public_trigger = catalog
        .triggers
        .iter()
        .find(|trigger| trigger.schema.is_none())
        .expect("the primary schema keeps its trigger");
    assert_eq!(public_trigger.definition, "EXECUTE FUNCTION touch_public()");
    let auth_trigger = catalog
        .triggers
        .iter()
        .find(|trigger| trigger.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its trigger");
    assert_eq!(auth_trigger.definition, "EXECUTE FUNCTION touch_auth()");
}

#[test]
fn a_single_schema_catalog_carries_no_schema_attribution() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let columns = vec![RawColumnMetadata {
        schema: None,
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
        data_type: "bigint".to_owned(),
        is_nullable: false,
        column_default: None,
        udt_name: None,
        udt_schema: None,
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,
        name: "users_parent_fkey".to_owned(),
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
        referenced_table: "users".to_owned(),
        referenced_column: "id".to_owned(),
        referenced_schema: None,
        on_delete: ForeignKeyAction::NoAction,
        on_update: ForeignKeyAction::NoAction,
    }];
    let enums = vec![RawEnumMetadata {
        schema: None,
        name: "mood".to_owned(),
        variant: "happy".to_owned(),
    }];
    let views = vec![RawViewMetadata {
        schema: None,
        name: "mood_counts".to_owned(),
        definition: "SELECT mood FROM users".to_owned(),
    }];
    let triggers = vec![RawTriggerMetadata {
        schema: None,
        name: "users_touch".to_owned(),
        table_name: "users".to_owned(),
        definition: "EXECUTE FUNCTION touch()".to_owned(),
    }];

    let catalog = catalog_from_metadata_with_sections(
        &tables,
        &columns,
        &[],
        &fks,
        &[],
        &enums,
        &RawCatalogSections {
            checks: &[],
            views: &views,
            triggers: &triggers,
        },
    );

    assert!(catalog.tables[0].schema.is_none());
    assert!(catalog.tables[0].foreign_keys[0].referenced_schema.is_none());
    assert!(catalog.enums[0].schema.is_none());
    assert!(catalog.views[0].schema.is_none());
    assert!(catalog.triggers[0].schema.is_none());
}
