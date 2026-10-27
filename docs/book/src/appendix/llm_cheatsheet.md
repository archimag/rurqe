# LLM Cheatsheet / Prompt Pack

Концентрированная версия книги для загрузки в контекст модели. Все утверждения
ниже выведены из кода воркспейса; примеры совпадают с doc-тестами
`crates/rurqe/src/lib.rs` и doctest’ами диалектных крейтов.

## 1. Точка входа и диалект

```rust
// Without a version pin (dialect is named explicitly):
use rurqe::prelude::*;
let q = select_from::<Postgres, User>().select(cols!(UserId));

// Best practice: pin the dialect with a prelude, no turbofish anywhere.
use rurqe::postgres::prelude::*;      // PostgreSQL, default Pg16
use rurqe::sqlite::prelude::*;        // SQLite, default Sqlite3_35
use rurqe::postgres::pg16::prelude::*;  // explicit engine version
```

Модули версий: `pg14`, `pg15`, `pg16`, `sqlite3_0`, `sqlite3_24`, `sqlite3_25`,
`sqlite3_30`, `sqlite3_35`, `sqlite3_38`. Каждый ре-экспортирует общий prelude и
добавляет `select_from`, `insert_into`, `update`, `delete_from`, трейт `SelectFrom`,
а также `Error` и `Result`, привязанные к диалекту.

## 2. Схема

```rust
rurqe::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        IsActive => "is_active": bool,
    }
}

rurqe::table_alias! {
    U2 => "u2"
}
```

`table!` создаёт ZST-маркеры таблицы и колонок; `Column::Table` связывает колонку с
таблицей и проверяется через `ColumnBelongsTo<T>`.

## 3. Сопоставление SQL ↔ DSL

| SQL | DSL |
|---|---|
| `FROM users` | `.from(users)` / `.from(Users)` |
| `SELECT a, b AS x` | `.select((a, b))` / `select! { x: b }` / `.select(cols!(A, B))` |
| `WHERE a = $1` | `.where_(a.eq(v))` |
| `WHERE a IS NULL` | `.where_(a.is_null())` |
| `WHERE a LIKE $1` | `.where_(a.like("A%"))` |
| `WHERE a ILIKE $1` | `.where_(a.ilike("A%"))` (в SQLite рендерится `LOWER(x) LIKE LOWER(?)`) |
| `WHERE a BETWEEN $1 AND $2` | `.where_(a.between(lo, hi))` |
| `WHERE a IN (...)` | `.where_(a.in_list(v))` / `.not_in_list(v)` |
| `WHERE a IN (SELECT …)` | `.where_(a.in_subquery(sub))` |
| `EXISTS (…)` / `NOT EXISTS (…)` | `.where_(exists(sub))` / `.where_(not_exists(sub))` |
| `INNER JOIN u ON …` | `.inner_join(u, u.id.eq(UserId))` |
| `LEFT JOIN u ON …` | `.left_join(u, …)` — колонки `u` становятся `Option<T>` |
| `FULL OUTER JOIN` | `.full_outer_join(u, …)` — обе стороны `Option<T>` |
| `CROSS JOIN` | `.cross_join(u)` |
| `LATERAL` | `.cross_join_lateral(l)` / `.left_join_lateral(l)` (PostgreSQL) |
| `GROUP BY a, b` | `.group_by((a, b))` → `GroupedQuery` |
| `HAVING COUNT(*) > 1` | `.having(count_all().gt(1i64))` |
| `ORDER BY a ASC` | `.order_by_asc(a)` |
| `ORDER BY a DESC NULLS LAST` | `.order_by_desc_nulls_last(a)` |
| `LIMIT n OFFSET m` | `.limit(n).offset(m)` |
| `SELECT DISTINCT` | `.distinct()` |
| `DISTINCT ON (a)` | `.distinct_on(a)` (PostgreSQL), `ORDER BY` должен начинаться с `a` |
| `UNION [ALL]` | `.union(q)` / `.union_all(q)` |
| `INTERSECT` / `EXCEPT` | `.intersect(q)` / `.except(q)` |
| `FOR UPDATE` / `FOR SHARE` | `.for_update()` / `.for_share()` (PostgreSQL) |
| `… SKIP LOCKED` / `NOWAIT` | `.skip_locked()` / `.nowait()` |
| `WITH cte AS (…)` | `.with(cte(q))` / `cte_named("name", q)` |
| `INSERT INTO t (…) VALUES (…)` | `insert_into(t).values((A::set(v), B::set(w)))` |
| `INSERT … SELECT` | `.columns((A, B)).from_select(select_q)` |
| `UPDATE t SET a = $1 WHERE …` | `update(t).set(a.set(v)).where_(…)` |
| `DELETE FROM t WHERE …` | `delete_from(t).where_(…)` |
| `RETURNING a` | `.returning(a)` (PostgreSQL, SQLite ≥ 3.35) |
| `ON CONFLICT (a) DO NOTHING` | `.on_conflict_do_nothing(a)` |
| `ON CONFLICT (a) DO UPDATE SET …` | `.on_conflict(a).do_update((A.set(excluded(A)),))` |

## 4. Жёсткие правила (нарушение — ошибка компиляции или `BuildError`)

1. **Нет дублирующихся имён отношений.** `FROM users JOIN users` — `BuildError::DuplicateRelation { name }`.
   Лечение: `table_alias!` / `users.as_alias("u2")`.
2. **Нет имён, которых нет в области видимости.** Колонка таблицы, не введённой
   через `from`/join, отвергается на месте вызова (`TableInScope`, `ExprInScope`).
3. **Внешнее соединение поднимает тип.** Колонка из `LEFT JOIN` — это `Option<T>`;
   прочитать её как `T` нельзя.
4. **`UPDATE`/`DELETE` без фильтра не собираются.** Нужен `.where_(…)` либо явный
   `.allow_all()`.
5. **`SELECT` без проекции не собирается** (`ProjectionPresent`),
   `UPDATE` без `SET` — тоже (`AssignmentsPresent`).
6. **`INSERT` без источника строк не собирается** (`InsertSource`, `EmptyRows`).
7. **Группировка закрыта.** После `.group_by(…)` проектными остаются только
   сгруппированные колонки, агрегаты и константы/оконные выражения
   (`GroupedSelectTarget`). Агрегат внутри `coalesce`, `nullif`, `greatest`,
   `least`, `CASE` сохраняет «агрегатность» через `CombineAggr`.
8. **Агрегат нельзя ставить в построчные клаузы.** `WHERE`, `GROUP BY`, `ON`
   join’а, `SET`/`VALUES`, предикаты `ON CONFLICT` — это `BuildError::AggregateInRowClause`.
9. **`DISTINCT` + `ORDER BY`:** каждое выражение сортировки обязано быть в проекции
   (`BuildError::DistinctOrderNotProjected`).
10. **`DISTINCT ON` + `ORDER BY`:** сортировка начинается ровно с выражений
    `DISTINCT ON` (`BuildError::DistinctOnOrderMismatch`).
11. **`DISTINCT`/`DISTINCT ON` несовместимы с блокировкой** (`DistinctWithLock`,
    `DistinctOnWithLock`); сам fluent-типстейт до такой пары не доводит.
12. **Составной запрос сортируется по своим выходным колонкам или 1-based позиции**
    (`order_by_output*`, `order_by_index`), а не по выражениям;
    `BuildError::CompoundOrderExpression` / `CompoundOrderColumn` / `CompoundOrderIndex`.
13. **Операнды `UNION`/`INTERSECT`/`EXCEPT` имеют одинаковую арность**
    (`BuildError::SetOperandArity`).
14. **Вложенный запрос не может запрашивать блокировку** (`BuildError::NestedLock`).
15. **`IN`-подзапрос совпадает по арности и типам** (`InSubquery<Row>`).
16. **Нет `JOIN … USING (col)`** и нет эмуляции неподдерживаемых возможностей —
    только capability-гейт.

## 5. Типстейты, которые нельзя обойти

| Правило | Трейт-страж |
|---|---|
| проекция задана | `ProjectionPresent` |
| мутация отфильтрована | `ExecutableMutation` |
| присваивания заданы | `AssignmentsPresent` |
| источник строк задан | `InsertSource`, `EmptyRows` |
| только сгруппированные/агрегированные поля | `GroupedSelectTarget`, `ColumnInGroupBy` |
| возможность диалекта | `SupportsReturning`, `SupportsOnConflict`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsLateralJoin`, `SupportsFullOuterJoin`, `SupportsRowLocking`, `SupportsDistinctOn`, `SupportsAggregateFilter`, `SupportsWindowFunctions`, `SupportsNullsOrdering`, `SupportsGreatest`, `SupportsJsonAggregation`, `SupportsDistinctFrom` |

## 6. Исполнение

```rust
let built = select_from(Users).select(cols!(UsersId)).build()?;

let all  = conn.fetch_all(&q)?;
let one  = conn.fetch_one(&q)?;        // 0 rows  -> ExecutionError::NoRows
let opt  = conn.fetch_optional(&q)?;   // >1 rows -> ExecutionError::MultipleRows
let n    = conn.execute(&q)?;          // affected rows
```

`fetch_optional` читает максимум две строки — весь набор не материализуется.

Транзакции:

```rust
let value = conn.transaction(|tx| {
    tx.execute(&insert_into(Users).values((UserId::set(1i64),)))?;
    Ok(())
})?;

conn.transaction_with_isolation(IsolationLevel::Serializable, |tx| Ok(()))?;
```

Правила:

- внешний блок — `BEGIN` (`COMMIT` при успехе), вложенный — `SAVEPOINT rurqe_sp_N`;
- паника/выход без `commit()` откатывает блок;
- отказ драйвера в `ROLLBACK` отравляет соединение: следующий `begin` сначала
  повторяет откат;
- уровень изоляции действует только на внешний `BEGIN`.

## 7. Доменные типы

```rust
rurqe::impl_sql_binary_newtype!(Txid, 32);
rurqe::impl_sql_text_newtype!(Address);

#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub struct UserId(i64); // #[sql(transparent)]
```

Режимы derive: `transparent`, `as = Bytes` (+ `len = N`), `as = Text`; для enum —
метка варианта с `#[sql(rename = "...")]`.

## 8. Cargo-фичи

| Фича | Что добавляет |
|---|---|
| `postgres` | `PgConnection` (синхронный `postgres`) |
| `tokio-postgres` | `PgAsyncConnection`, `PgAsyncTransaction` |
| `deadpool-postgres` | `PgAsyncPool` |
| `r2d2` | `PgPool`, `SqlitePool` |
| `sqlite` | `Sqlite`, `Sqlite3_*`, `SqliteConnection` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `SqlType` |
| `migrate` | `rurqe::migrate` |
| `decimal` | `SqlVal::Decimal`, `rust_decimal::Decimal` |
| `uuid` | `SqlVal::Uuid`, `uuid::Uuid` |
| `chrono` | `SqlVal::NaiveDateTime` / `DateTimeUtc` / `NaiveDate` |
| `serde_json` | `SqlVal::Json`, `Json<T>` |

## 9. Чек-лист перед отправкой кода

- [ ] Нигде нет турбофиша `::<Postgres, …>` — используется prelude версии.
- [ ] Каждая таблица в запросе введена через `from`/join; при повторе — алиас.
- [ ] `UPDATE`/`DELETE` имеют `where_` или осознанный `allow_all()`.
- [ ] Поля из `LEFT JOIN` читаются как `Option<T>`.
- [ ] При `group_by` все проекции — группы или агрегаты.
- [ ] `DISTINCT`/`DISTINCT ON` согласованы с `ORDER BY`.
- [ ] `RETURNING`/`ON CONFLICT`/`LATERAL` используются только на версии, где они есть.
- [ ] Доменные типы описаны макросом или derive, а не `String`.

## 10. Матрица «симптом → причина»

| Симптом | Причина | Правка |
|---|---|---|
| `table X is not part of this query's scope` | таблица не введена в запрос | `.from(X)` или join |
| `column X does not belong to table Y` | использована колонка чужой таблицы | взять колонку `Y` |
| `dialect … does not support RETURNING` | версия не реализует capability | поднять версию движка или убрать `.returning` |
| `dialect … does not support row-level locking` | `.for_update()` на SQLite | перейти на PostgreSQL |
| `cannot decode a SQL `text` value into `i64`` | колонка отображена на другой тип | исправить кодогенерацию/override |
| `cannot decode a row of 1 values into a row type of 2 values` | проекция и тип строки рассинхронизированы | привести в соответствие `select` и `FromRow` |
| `duplicate unaliased relation …` | один и тот же effective name дважды | алиас |
| `aggregate functions are not allowed in …` (пре-флайт) | агрегат в построчной клаузе | вынести в подзапрос |
