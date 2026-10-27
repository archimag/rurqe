# MULTISET: как вложенность превращается в SQL

Вложенная проекция не склеивается на клиенте: она становится узлом AST, который рендерится одной JSON-агрегацией.

## Узел AST

```rust
// crates/rurqe-core/src/nested.rs
fn nested_kind<D, S, Row>(query: &SelectQuery<D, S, Row, HasProjection>, single: bool) -> ExprKind
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    ExprKind::Multiset {
        query: Subquery::new(query.to_ast()),
        fields: Row::FIELDS.to_vec(),
        single,
    }
}
```

Узел несёт три вещи: вложенное утверждение, ключи документа и признак «не более одной строки».

## Ключи документа

Рендерер (`crates/rurqe-core/src/expr/render/nested.rs`) присваивает вложенному утверждению `projection_aliases = Some(fields)`: каждое проецируемое выражение получает имя поля, которое оно питает. Поэтому документ строки несёт ровно те ключи, что объявляет `MultisetRow::FIELDS`, а клаузы вложенного запроса — фильтр, порядок, лимит — остаются в силе и определяют содержимое коллекции.

## Две формы рендеринга

Форму выбирает константа `Dialect::SUPPORTS_JSONB_ROW_AGG`. Обе формы дают один и тот же JSON-документ, поэтому клиент декодирует одно представление независимо от движка, который его произвёл.

### PostgreSQL: нативная агрегация строки

`SUPPORTS_JSONB_ROW_AGG = true`, поэтому коллекция рендерится как `jsonb_agg(to_jsonb(...))`, а отсутствие строк заменяется пустым массивом:

```sql
COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
          FROM (SELECT "order_id" AS "order_id", "total" AS "total" FROM "orders")
               AS "__rurqe_sub"), '[]'::jsonb)
```

### SQLite: объект по полям

Диалект без нативной агрегации строки собирает каждый объект через `json_object` поле за полем и склеивает объекты `json_group_array`; ключи пишутся строковыми литералами, а значения читаются через алиас `__rurqe_sub`:

```sql
COALESCE((SELECT json_group_array(json_object('order_id', "__rurqe_sub"."order_id",
                                               'total', "__rurqe_sub"."total"))
          FROM (SELECT "order_id" AS "order_id", "total" AS "total" FROM "orders")
               AS "__rurqe_sub"), '[]')
```

## `nest_one`: агрегат без `COALESCE` и лимит 1

Для одной строки форма отличается двумя деталями: агрегат не оборачивается в `COALESCE`, а лимит вложенного запроса становится `1` — либо лимитом компаундного запроса, если внутри есть `UNION`:

```rust
if single {
    if inner.set_operations.is_empty() {
        inner.limit = Some(1);
    } else {
        inner.compound_limit = Some(1);
    }
}
```

PostgreSQL: `(SELECT to_jsonb("__rurqe_sub") FROM (...) AS "__rurqe_sub")`. SQLite: `(SELECT json_object(...) FROM (...) AS "__rurqe_sub")`. Порядок и пагинация вложенного запроса решают, какая именно строка попадёт в `Option<Row>`, а отсутствие строки даёт `NULL` — то есть `None` при декодировании.

## Сырые фрагменты и плейсхолдеры

`render_multiset` возвращает `Result<(), RawSqlError>`: `sql!`-выражение внутри вложенного запроса может объявить число параметров, не совпадающее с фактическими фрагментами, и расхождение сообщается как `RawSqlError` вместо отправки SQL с потерянным или смещённым плейсхолдером.

## Мультимножество, а не множество

- `Vec<Row>` сохраняет порядок, заданный `ORDER BY` вложенного запроса, и сохраняет дубликаты: SQL без `DISTINCT` их не удаляет.
- `nest_one` возвращает `Option<Row>`: отсутствие ребёнка — это `None`, а не пустая коллекция.
- `DISTINCT` и `DISTINCT ON` — отдельная операция над утверждением со своим тип-состоянием, а не свойство вложенной проекции.

## Связанные главы

- [Вложенные результаты: nest_many и nest_one](nested.md)
- [Движки, capability-гейты и декодирование](multiset_engines.md)
- [NF² и MULTISET: вложенные отношения](../theory/nf2_multiset.md)
