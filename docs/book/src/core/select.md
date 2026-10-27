# Проекции и `SELECT`

Проекция — реляционный оператор π: выбор подмножества атрибутов с сохранением порядка. В RURQE она записывается в AST на месте вызова и фиксирует **тип строки** запроса.

## `.select(...)` и тип строки

```rust
pub trait SelectTarget {
    type RustType;
    fn push_column_name(&self, out: &mut Vec<&'static str>) { /* по умолчанию ничего */ }
    fn to_expr_kind(&self) -> ExprKind;
}

pub trait Selection {
    type Row;
    fn push_column_names(&self, out: &mut Vec<&'static str>) { /* по умолчанию ничего */ }
    fn push_projections(&self, out: &mut Vec<ExprKind>);
}

pub trait SelectionFor<Tables, Positions, D = ()>: Selection {
    type Row;
}
```

`SelectionFor::Row` вычисляет тип строки с учётом области видимости: колонка `LEFT JOIN`-нутой таблицы приедет как `Option<T>` (см. [Nullability](../theory/nullability.md)).

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, User>()
    .select((UserId, UserName, UserActive))
    .build()
    .expect("valid query");
```

```text
SELECT "id", "name", "active" FROM "users"
```

Кортежные селекции реализованы для арности 1..=8 (`impl_selection!` в `query/selection.rs`).

## Четыре формы проекции

| Форма | Тип строки | Пример |
|---|---|---|
| кортеж | позиционный кортеж | `.select((UserId, UserName))` |
| `cols!` | позиционный кортеж, с алиасами | `.select(cols!(UserId, UserName as contact_email))` |
| `select!` | анонимная структура с именованными полями | `.select(select!(UserId, UserName as contact_email))` |
| `dto!` | именованная структура только для декодирования | `dto! { pub struct UserSummary { UserId, UserName as contact_email } }` |

## `select!`: именованные проекции

`select!` синтезирует на месте вызова анонимную запись `__RurqeRecord` и селекцию `__RurqeSelection`, которая рендерит каждый элемент через `SelectTarget` и оборачивает его в `ExprKind::Aliased` под **имя поля**. Поэтому выходные колонки запроса — это ключи, по которым декодируется запись, и они остаются достижимыми, если запрос вложить как производную таблицу или CTE.

```rust
use rurqe::prelude::*;

let built = select!(UserId, UserName as contact_email)
    .from(User)
    .where_(Expr::column::<UserId>().gt(1i64))
    .build()
    .expect("valid query");
```

```text
SELECT "id" AS "user_id", "name" AS "contact_email" FROM "users" WHERE ("id" > $1)
```

Порождённая запись реализует `FromRow`, так что строка декодируется в поля по имени, а не по индексу:

```rust
let row = UserSummary::from_row(&[
    SqlVal::I64(7),
    SqlVal::Text("root".to_owned()),
])
.expect("row decodes");

assert_eq!(row.user_id, 7);
assert_eq!(row.contact_email, "root");
```

Та же запись описывает строку вложенной коллекции: реализуются `ToSqlVal`, `FromSqlValue`, `SqlArrayElement` и `MultisetRow` с константой `FIELDS`, поэтому `select!`-проекция читается как `Vec<...>` из `nest_many` (см. [NF² и MULTISET](../theory/nf2_multiset.md)).

`select!`-селекция — это `TableSelection<T>` **только** если все её элементы являются колонками таблицы `T`: каждый элемент утверждает `ColumnBelongsTo<T>`, а список с вычисляемым элементом не порождает реализацию вовсе. Именно поэтому `RETURNING` принимает `select!` из колонок и отвергает `select!(UserId, doubled: Expr::count_all())`.

## `dto!`: структура для границы функций

```rust
use rurqe::prelude::*;

dto! {
    /// Проекция пользователя, возвращаемая из репозитория.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}
```

`dto!` порождает `pub struct` с полями, типы которых выведены из `<Column as Column>::RustType` (не нужно повторять типы), и реализует `FromRow`, `MultisetRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`. Ключи JSON-документа — имена полей в порядке объявления.

`dto!` **не** является проекцией: поля обязаны быть маркерами колонок (вычисляемое поле отвергается с ошибкой расширения), и никакой проекционной поверхности (`Selection`, `push_projections`) он не порождает. Запрос проецирует `select!` с теми же именами полей, а `dto!` декодирует полученную строку.

## Явный nullable-маркер

Иногда nullable-форма нужна без внешнего соединения — например, чтобы согласовать форму проекции с уже посчитанным результатом:

```rust
impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;
    const NAME: &'static str = C::NAME;
    fn expr_kind() -> ExprKind { C::expr_kind() }
}
```

Доступно как `Nullable::<C>::new()` и как метод запроса `query.nullable::<C>()`: это тот же столбец, но в типе значения на один слой `Option` больше. Двойного `Option<Option<T>>` не бывает — `MakeNullable for Option<T>` возвращает `Option<T>`.

## Инварианты

- **Проекция записывается один раз.** После `UNION`/`INTERSECT`/`EXCEPT` выходные колонки зафиксированы первым списком проекций, поэтому `ProjectionOpen` для такого запроса не реализован и `.select(...)` недостижим.
- **`SELECT` без проекции не рендерится.** `.build()` требует `ProjectionPresent`.
- **Пустая проекция отклоняется предполётно.** Ручная реализация `Selection`, записывающая пустой список, даёт `BuildError::EmptyProjection` вместо `SELECT  FROM "users"`.
- **Проекция проверяется по области.** Колонка таблицы, которой нет в `FROM`/`JOIN`, отвергается в точке вызова через `SelectTargetInScope`.

> **Compiler Guard in Action.** `Selection::push_projections` не имеет тела по умолчанию: реализация обязана перечислить элементы, иначе запрос отрендерил бы `SELECT  FROM ...`.

## Связанные главы

- [Предикаты и `WHERE`](filtering.md)
- [Сортировка и пагинация](ordering.md)
- [`GROUP BY` и `HAVING`](group_by.md) — те же проекции под правилом группировки.
- [Алиасы](aliasing.md)
