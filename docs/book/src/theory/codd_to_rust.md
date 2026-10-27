# От реляционной алгебры Кодда к системе типов Rust

Глава сопоставляет каждый оператор реляционной алгебры с конкретным местом в кодовой базе: типом, трейтом, методом и SQL-клаузой. Никаких «примерно так» — только то, что реализовано и покрыто тестами.

## Уровень данных

### Отношение

| Алгебра | RURQE |
|---|---|
| Имя и схема отношения | `Table` с константой `NAME`, реализация обычно из макроса `table!` |
| Заголовок (набор атрибутов) | Набор ZST-типов, реализующих `Column` |
| Экземпляр отношения в запросе | `TableScope` — упорядоченный список отношений |
| Ссылка отношения с алиасом | `TableRef { name, alias }`, `Table::table_ref()` |

`TableScope` умеет отдать имена (`push_table_names`) и полноценные ссылки для рендеринга (`push_table_refs`), причём реализация по умолчанию строит вторые из первых, а `Aliased<T, A>` переопределяет метод, чтобы в `FROM` попал `AS "alias"`.

### Атрибут

```rust
pub trait Column {
    type Table: Table;
    type RustType;
    const NAME: &'static str;
    fn expr_kind() -> ExprKind;
}
```

`Column::RustType` — это мост между SQL-типом и типом значения в Rust: `VALUES`, `SET`, сравнения и декодирование строки проверяются против него. `ColumnBelongsTo<T>` статически утверждает принадлежность колонки таблице.

## Унарные операторы

### Селекция σ (фильтрация)

- Алгебра: подмножество кортежей, удовлетворяющих предикату.
- RURQE: `.where_(Expr<bool, Scope>)`; динамические формы `.where_opt`, `.where_if`, `.when`, `.if_some`.
- SQL: `WHERE`.
- Инвариант: предикат обязан быть `Expr<bool>` и ссылаться только на отношения области видимости (`ExprInScope`).

### Проекция π

```rust
pub trait SelectTarget { type RustType; fn to_expr_kind(&self) -> ExprKind; }
pub trait Selection { type Row; fn push_projections(&self, out: &mut Vec<ExprKind>); }
```

- Алгебра: выбор подмножества атрибутов с сохранением порядка.
- RURQE: `.select((UserId, UserName))` для кортежа, `cols!` для списка, `select!` / `dto!` для именованных полей.
- SQL: список проекций.
- Инвариант: `SelectionFor<Tables, Positions, D>::Row` вычисляет тип строки с учётом nullability области; проекция колонки вне области отклоняется `SelectTargetInScope`.

`Selection::push_projections` не имеет тела по умолчанию: реализация обязана перечислить проецируемые узлы, иначе запрос отрендерил бы `SELECT  FROM ...`. Пустой список отклоняется и предполётной проверкой (`BuildError::EmptyProjection`).

### Сортировка и пагинация

- RURQE: семейство `.order_by_*` (включая `_nulls_first` / `_nulls_last` через `SupportsNullsOrdering`), `.limit`, `.offset`.
- SQL: `ORDER BY`, `LIMIT`, `OFFSET`.
- Инвариант: сортировка по колонке требует её присутствия в области; для `DISTINCT` и `DISTINCT ON` согласование с проекцией — Tier 2 (см. `BuildError::DistinctOrderNotProjected`, `BuildError::DistinctOnOrderMismatch`).

## Бинарные операторы

### Тета-соединение ⋈θ

```rust
select_from(Users).inner_join_table(Orders, Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
```

- Тип результата: `SelectQuery<D, Joined<Tables, T>, ...>`.
- SQL: `INNER JOIN ... ON`.
- Инвариант: предикат проверяется уже против расширенной области `Joined<Tables, T>`, поэтому колонка новой таблицы допустима, а посторонняя — нет.

### Левое внешнее соединение ⟕

- Тип: `SelectQuery<D, LeftJoined<Tables, T>, ...>`.
- SQL: `LEFT JOIN ... ON`.
- Инвариант: `TableNullabilityInScope<T, At<0>> for LeftJoined<..>` возвращает `NullableRel`, поэтому проекция колонки даёт `Option<T>` (см. [Nullability](nullability.md)).

### Полное внешнее соединение ⟗

- Тип: `FullOuterJoined<Tables, T>`; требует `D: SupportsFullOuterJoin`.
- SQL: `FULL OUTER JOIN ... ON`.
- Инвариант: поднимаются **обе** стороны — реализация `TableNullabilityInScope` для `FullOuterJoined` возвращает `NullableRel` и для новой таблицы, и для всех таблиц внутренней области.

### Декартово произведение ×

- `cross_join_table(T)` → `CrossJoined<Tables, T>`; `Nullability = NonNullableRel`.
- SQL: `CROSS JOIN`.

### Коррелированное соединение (LATERAL)

- `cross_join_lateral(Lateral<T, ..>)` → `CrossLateralJoined` (NonNullable), `left_join_lateral(...)` → `LeftLateralJoined` (Nullable).
- Требует `D: SupportsLateralJoin` (только PostgreSQL).
- SQL: `JOIN LATERAL (SELECT ...) AS "t"`, для левой формы — `LEFT JOIN LATERAL ... ON true`.

## Группировка и агрегация

### γ (группировка)

- `.group_by((UsersId, UsersName))` переводит запрос в `GroupedQuery<D, Tables, Output, GroupByCols, Projected>`.
- SQL: `GROUP BY`.
- Инвариант: только сгруппированные колонки, агрегаты, литералы и оконные выражения реализуют `GroupedSelectTarget`; попытка спроецировать негруппированную колонку — ошибка компиляции, в том числе если колонка спрятана внутри `coalesce` / `CASE` (это отслеживают `FunctionOperand` и `CombineAggr`).
- Остаточные случаи (например, `HAVING` над негруппированной колонкой) добирает Tier 2: `UngroupedHavingColumn`, `UngroupedColumn`, `UngroupedColumnWithAggregate`.

## Операции над множествами

| Алгебра | Метод | Тип-состояние |
|---|---|---|
| ∪ | `.union(other)`, `.union_all(other)` | `Compound`, затем `TailClosed` после первой сортировки |
| ∩ | `.intersect(other)` | то же |
| − | `.except(other)` | то же |

- SQL: `UNION` / `UNION ALL` / `INTERSECT` / `EXCEPT`.
- Инвариант Tier 1: операнды обязаны иметь одинаковый диалект и одинаковый тип строки, а `SetOperand` недоступен после `TailClosed`, поэтому нельзя присоединить операнд после сортировки комбинированных строк.
- Инвариант Tier 2: `SetOperandArity`, `CompoundOrderIndex`, `CompoundOrderColumn`, `CompoundOrderExpression`.
- Сортировка комбинированных строк — только по выходным именам или 1-based позициям: `.order_by_output_*`, `.order_by_index*`.

## Полусоединения и антисоединения

| Алгебра | RURQE | SQL |
|---|---|---|
| Полусоединение ⋉ | `exists(subquery)` | `EXISTS (SELECT ...)` |
| Антисоединение ▷ | `not_exists(subquery)` | `NOT EXISTS (SELECT ...)` |
| Полусоединение по атрибуту | `col.in_subquery(sub)` | `IN (SELECT ...)` |
| Антисоединение по атрибуту | `col.not_in_subquery(sub)` | `NOT IN (SELECT ...)` |
| Кортежное полусоединение | `(a, b).in_subquery(sub)` через `RowExpr` | `(a, b) IN (SELECT ...)` |

Инвариант: `InSubquery<Row>` связывает ожидаемый тип строки с проекцией вложенного запроса — скалярная форма принимает только `(T,)`, кортежная — ровно ту арность и те типы, что объявлены. Несовпадение отклоняется `rustc`.

Оператор реляционного деления отдельным методом не реализован: он выражается композицией `not_exists` / `exists`.

## Вложенные отношения (NF²)

- `nest_many(query)` → `Vec<Row>`, `nest_one(query)` → `Option<Row>`.
- Требует `Row: MultisetRow` с константой `FIELDS` и `QueryD: SupportsJsonAggregation`.
- SQL: PostgreSQL — `jsonb_agg(to_jsonb(...))` с `COALESCE(..., '[]'::jsonb)`; диалект без нативной агрегации строк собирает объекты `json_object` и массив `json_group_array`.
- Подробности — [NF² и MULTISET](nf2_multiset.md).

## Сводная таблица инвариантов

| Оператор | Что проверяет компилятор | Что добирает `.build()` |
|---|---|---|
| σ, π | область видимости, тип предиката, тип строки | пустая проекция |
| ⋈θ, ⟕, ⟗, ×, LATERAL | область, nullability, capability | дубликаты отношений |
| γ | группа или агрегат | `HAVING` и остаточные ссылки |
| ∪ / ∩ / − | одинаковый диалект и тип строки, порядок клауз | арность операндов, форма сортировки |
| ⋉ / ▷ | арность и типы подзапроса | — |
| NF² | `MultisetRow`, capability JSON-агрегации | — |
