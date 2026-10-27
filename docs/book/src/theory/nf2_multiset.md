# NF² и MULTISET: вложенные отношения

Классическая реляционная модель требует первой нормальной формы: значения атрибутов атомарны. Модель NF² (Non-First-Normal-Form) разрешает атрибуту быть вложенным отношением. RURQE реализует это не клиентской склейкой, а одной SQL-агрегацией.

## Проблема

Связь «один ко многим» в наивном виде даёт либо N+1 запросов (по запросу на родителя), либо декартово произведение в `JOIN`, из которого клиент вручную собирает вложенные списки. Оба варианта — источник ошибок и лишнего трафика.

## Решение: проекция-мультимножество

```rust
pub fn nest_many<Q>(query: Q) -> NestMany<Q>;
pub fn nest_one<Q>(query: Q) -> NestOne<Q>;

pub struct NestMany<Q>(pub Q);  // RustType = Vec<Row>
pub struct NestOne<Q>(pub Q);   // RustType = Option<Row>
```

Проекция выглядит как обычный элемент списка проекций:

```rust
let users_with_orders = select_from(Users)
    .select(dto!(UserWithOrders {
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Orders)
                .select(dto!(OrderSummary {
                    order_id: OrderId,
                    total: OrderTotal,
                }))
                .where_(OrderUserId.eq(UserId))
        ),
    }));
```

Здесь `where_(OrderUserId.eq(UserId))` — корреляция с внешним запросом: вложенное утверждение видит `UserId` благодаря тому, что `ExprInScope` проверяет его против объемлющего запроса, а `Unscoped` внутри подзапроса означает «проверку делает тот, кто вложил».

## Как это рендерится

Проекция становится узлом AST:

```rust
ExprKind::Multiset {
    query: Subquery::new(query.to_ast()),
    fields: Row::FIELDS.to_vec(),
    single,
}
```

PostgreSQL:

```sql
SELECT "id",
       COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
                 FROM (SELECT "sku" AS "sku", "qty" AS "qty" FROM "lines")
                 AS "__rurqe_sub"), '[]'::jsonb)
FROM "orders"
```

Диалект без нативной агрегации строк собирает каждый объект через `json_object` и массив через `json_group_array`. Клиент декодирует один и тот же JSON-документ независимо от движка.

Для SQLite функции JSON доступны начиная с версии, у которой они всегда вкомпилированы: поэтому для вложенных проекций используется прелюдия `sqlite3_38` (`Sqlite<Sqlite3_38>`), а требование выражено трейтом `SupportsJsonAggregation`.

## Требования компилятора

```rust
impl<Tables, InnerD, QueryD, S, Row> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestMany<SelectQuery<InnerD, S, Row, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Vec<Row>;
}
```

Три инварианта:

1. **Один диалект.** Вложенное утверждение принадлежит тому же диалекту, что и объемлющий запрос (`DialectInScope<QueryD>`).
2. **Нативная агрегация.** Диалект обязан реализовывать `SupportsJsonAggregation`; иначе проекция просто не компилируется, а не падает внутри движка.
3. **Тип строки.** `Row: MultisetRow` — декларация ключей документа.

Позиция `At<0>` фиксирована: вложенная проекция читает из собственного утверждения и не ссылается на таблицы внешней области, поэтому нет смысла выводить её позицию.

## `MultisetRow` и документ

```rust
pub trait MultisetRow: SqlArrayElement + 'static {
    /// JSON-ключи проецируемых полей в порядке проекции.
    const FIELDS: &'static [&'static str];
}
```

`FromSqlValue` реализации получает JSON-объект одной строки — как `SqlVal::Json` из PostgreSQL или как JSON-текст из SQLite — и превращает его в объект через `json_object`. Обратное направление (например, при передаче документа параметром) даёт `multiset_object` / `sql_val_to_json`.

Соглашение о представлении:

| Тип значения | В JSON |
|---|---|
| `i32`, `i64` | число |
| `f64` | число; `NaN` и бесконечности — `null` |
| `bool` | булево |
| `String` | строка |
| `Vec<u8>` | канонический hex-текст, который `FromSqlValue for Vec<u8>` читает обратно |
| `Decimal` (фича `decimal`) | число, если все цифры сохраняются, иначе текст |
| `Uuid`, `NaiveDateTime`, `DateTime<Utc>`, `NaiveDate` | текст `Display`, который `FromSqlValue` парсит обратно |
| вложенный JSON | как есть |

Благодаря этому поле вложенной строки декодируется тем же контрактом `FromSqlValue`, что и колонка того же типа.

## Почему MULTISET, а не множество

Реляционная алгебра Кодда работает с множествами: дубликаты невозможны. SQL — с мультимножествами: `SELECT` не удаляет дубликаты без `DISTINCT`. RURQE следует SQL, а не учебнику:

- кортеж строк — это мультимножество, порядок определяется `ORDER BY`;
- `Vec<Row>` во вложенной проекции — это именно мультимножество в порядке, заданном вложенным запросом;
- `DISTINCT` / `DISTINCT ON` — отдельные операции, меняющие тип-состояние (после них блокировка строки недоступна, что зафиксировано и в типах, и в `BuildError::DistinctWithLock` / `DistinctOnWithLock`).

`nest_one` отличается от `nest_many` только тем, что агрегация останавливается на первой строке: отсутствие ребёнка даёт `None`, а не пустой вектор, поэтому «нет значения» остаётся явным.

## Что это даёт

| Было | Стало |
|---|---|
| N+1 запросов | один запрос |
| декартово произведение с клиентской склейкой | нативная агрегация |
| ручной JSON-парсинг | `Vec<ChildDto>` / `Option<ChildDto>` с декодированием |
| ошибка «нет функции JSON» в рантайме | ошибка компиляции без `SupportsJsonAggregation` |

## Связанные главы

- [Кодд → Rust](codd_to_rust.md) — место NF² в таблице операторов.
- [Области видимости](scopes.md) — почему корреляция в подзапросе допустима, а ссылка на постороннюю таблицу нет.
