# Вложенные результаты: `nest_many` и `nest_one`

Связь «один ко многим» в наивной реализации читается либо N+1 запросами (по одному на родителя), либо `JOIN` с декартовым произведением, который клиент вручную сворачивает в списки. RURQE читает такую связь одним утверждением: движок агрегирует строки детей каждого родителя в JSON-документ, а клиент декодирует документ в тип строки дочерней проекции — без дополнительных запросов и без склейки на стороне клиента.

```rust
// crates/rurqe-core/src/nested.rs
pub fn nest_many<Q>(query: Q) -> NestMany<Q>;
pub fn nest_one<Q>(query: Q) -> NestOne<Q>;

pub struct NestMany<Q>(pub Q); // RustType = Vec<Row>
pub struct NestOne<Q>(pub Q);  // RustType = Option<Row>
```

## Пример: пользователи со своими заказами

```rust
use rurqe::prelude::*;

let report = select_from(Users)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Orders)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .order_by_asc(OrderCreatedAt)
        ),
    ))
    .fetch_all(&mut conn)?;

for row in report {
    println!("{} — {} заказов", row.user_name, row.orders.len());
    for order in row.orders {
        println!("  #{} на {}", order.order_id, order.total);
    }
}
```

Внешняя проекция — `select!`, а не `dto!`: `nest_many(...)` это вычисляемая проекция, а `dto!` принимает только маркеры колонок и отвергает вычисляемое поле ошибкой расширения (`use `select!` for computed projections`). По той же причине и вложенная проекция описывается `select!` — именно её запись становится типом элемента коллекции.

## Тип строки и позиция

Проекция реализует `SelectTarget` (тип значения) и `SelectTargetInScope` (тип значения с учётом области видимости):

```rust
impl<D, S, Row> SelectTarget for NestMany<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Vec<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind(&self.0, false)
    }
}

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

Позиция `At<0>` зафиксирована: вложенная проекция читает из собственного утверждения и не ссылается на таблицы внешней области, поэтому выводить её позицию не из чего — фиксированная позиция делает маркер определённым, вместо того чтобы заставлять вывод выбирать одну из позиций области.

## Корреляция с внешним запросом

Вложенное утверждение — самостоятельный `SelectQuery`, и его типовая область видимости содержит только его собственный `FROM`. Колонка внешнего запроса попадает в предикат через `Expr::without_scope()`, который заменяет таблицы выражения маркером `Unscoped`: такое выражение принимается в любой области, потому что его таблицы проверяет объемлющее утверждение.

```rust
orders: nest_many(
    select_from(Orders)
        .select(select!(order_id: OrderId, total: OrderTotal))
        .where_(OrderUserId.eq(Expr::column::<UserId>().without_scope()))
)
```

## `nest_one`: отсутствие ребёнка как `None`

`nest_one` отличается от `nest_many` только тем, что агрегация останавливается на первой строке: отсутствие ребёнка даёт `None`, а не пустой вектор, поэтому «нет значения» остаётся явным и не смешивается с «есть, но пусто».

## Контракт типа строки

Тип элемента объявляет, из каких ключей читается документ одной строки:

```rust
pub trait MultisetRow: SqlArrayElement + 'static {
    /// The JSON keys of the projected fields, in projection order.
    const FIELDS: &'static [&'static str];
}
```

`select!` синтезирует запись вместе с реализациями `FromRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement` и `MultisetRow`; `FIELDS` — имена полей в порядке проекции, и те же имена становятся ключами JSON-документа (поле `r#type` пишется как `type`).

`dto!` объявляет тот же набор контрактов (`MultisetRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`) с теми же ключами в порядке объявления, но проекций не порождает: DTO — это тип декодирования и вложенной строки, а не список проекций, поэтому строку проецирует `select!`, а DTO её читает.

## Compiler Guard in Action

- **Нет нативной JSON-агрегации — нет проекции.** `QueryD: SupportsJsonAggregation` в `SelectTargetInScope` означает, что `nest_many` для `Sqlite<Sqlite3_35>` не компилируется: отказ приходит на этапе типов, а не из движка.
- **Проекция без строк.** Вложенный запрос обязан быть в состоянии `HasProjection`; `nest_many(select_from(Orders))` без `.select(...)` вложить нельзя.
- **Тип строки обязан объявить ключи.** `Row: MultisetRow` связывает документ с декларацией полей, поэтому произвольный тип элемента не подходит.

## Связанные главы

- [MULTISET: как вложенность превращается в SQL](multiset.md)
- [Движки, capability-гейты и декодирование](multiset_engines.md)
- [NF² и MULTISET: вложенные отношения](../theory/nf2_multiset.md)
