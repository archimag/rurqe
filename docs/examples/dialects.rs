//! One query, three dialect modules: identifier quoting, placeholders, and
//! the pinned capability set differ, the typed builder stays the same. The file
//! only assembles and renders statements, so it names no runtime at all.

use rurqe::postgres::pg14;
use rurqe::postgres::pg16;
use rurqe::postgres::prelude::*;
use rurqe::sqlite::{sqlite3_0, sqlite3_24, sqlite3_35};
use rurqe_examples::markers::*;

fn main() -> Result<(), BuildError> {
    // ANCHOR: postgres
    let postgres_query = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
    // ANCHOR_END: postgres
    println!("{}", postgres_query.sql);

    // ANCHOR: sqlite
    // This file composes both engines, so the entry point names the prelude it
    // starts from: the unversioned prelude of SQLite binds the statement to
    // SQLite 3.35.
    let sqlite_query = rurqe::sqlite::prelude::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
    // ANCHOR_END: sqlite
    println!("{}", sqlite_query.sql);

    // ANCHOR: version
    // The version marker pins the capability set, not the builder: the same
    // statement composes for any pinned release, and only the clauses that
    // release does not support stay unreachable.
    let legacy_query = sqlite3_0::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
    // ANCHOR_END: version
    println!("{}", legacy_query.sql);

    // ANCHOR: pg_versions
    // PostgreSQL pins its own releases the same way, so the statement records
    // the engine version it targets in its dialect parameter.
    let fourteen = pg14::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
    let sixteen = pg16::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
    // ANCHOR_END: pg_versions
    println!("{} / {}", fourteen.sql, sixteen.sql);

    // ANCHOR: jsonb
    // A JSONB column is filtered and projected through the native operators,
    // so a document never leaves the typed builder.
    let documents = select_from(Profile)
        .select(cols! {
            profile_id: ProfileId,
            owner: ProfileMetadata.json_get_text("owner"),
        })
        .where_(ProfileMetadata.json_has_key("verified"))
        .build()?;
    // ANCHOR_END: jsonb
    println!("{}", documents.sql);

    // ANCHOR: arrays
    // An array column compares through the native containment operators, and the
    // element type follows the column.
    let tagged = select_from(Profile)
        .select(cols!(ProfileId))
        .where_(ProfileTags.array_contains(vec!["rust".to_owned(), "sql".to_owned()]))
        .build()?;
    // ANCHOR_END: arrays
    println!("{}", tagged.sql);

    // ANCHOR: regex
    // A text column matches a POSIX regular expression.
    let matching = select_from(Profile)
        .select(cols!(ProfileId))
        .where_(ProfileBio.regex_match("^rust"))
        .build()?;
    // ANCHOR_END: regex
    println!("{}", matching.sql);

    // ANCHOR: array_agg
    // Each group reports its articles as a list and as one text. The ordering
    // sits inside the aggregate call, and an aggregate over no rows yields an
    // empty list and an empty text instead of NULL.
    let summaries = select_from(OrderItem)
        .group_by(OrderItemOrderId)
        .select((
            OrderItemOrderId,
            array_agg(OrderItemSku).order_by_asc(OrderItemSku),
            string_agg(OrderItemSku, ", ").order_by_desc(OrderItemSku),
        ))
        .build()?;
    // ANCHOR_END: array_agg
    println!("{}", summaries.sql);

    // ANCHOR: for_update
    // A query claims the rows it selects until the transaction ends, and
    // `SKIP LOCKED` passes over the rows another transaction already holds.
    let claimed = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .for_update()
        .skip_locked()
        .build()?;
    // ANCHOR_END: for_update
    println!("{}", claimed.sql);

    // ANCHOR: aliased_insert
    // PostgreSQL accepts an alias on the target of an `INSERT`, so the inserted
    // row stays reachable through it. The aliased table enters the pinned entry
    // point as a value, and the row is recorded and projected through the
    // accessor the alias declares.
    let aliased = pg16::insert_into(User.as_alias(UserAlias))
        .values(UserAlias.UserId.set(7i64))
        .returning((UserAlias.UserId,))
        .build()?;
    // ANCHOR_END: aliased_insert
    println!("{}", aliased.sql);

    // ANCHOR: sqlite_upsert
    // `ON CONFLICT` arrived in SQLite 3.24, so the release the statement is
    // pinned to decides whether the clause exists.
    let upserted = sqlite3_24::insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict_do_nothing()
        .build()?;
    // ANCHOR_END: sqlite_upsert
    println!("{}", upserted.sql);

    // ANCHOR: sqlite_returning
    // `RETURNING` arrived in SQLite 3.35, and the module pinned to that release
    // is where the clause is reachable.
    let returned = sqlite3_35::insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .returning(cols!(UserId))
        .build()?;
    // ANCHOR_END: sqlite_returning
    println!("{}", returned.sql);

    Ok(())
}
