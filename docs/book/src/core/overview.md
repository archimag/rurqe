# Переносимое SQL ядро

Эта часть книги описывает **переносимую** половину DSL — ту, что не зависит от конкретного движка: маркеры схемы, проекции, фильтры, сортировку, группировку, оконные функции, соединения, алиасы, подзапросы, CTE, операции над множествами и DML.

Код, на котором построены все главы:

| Крейт | Что задаёт |
|---|---|
| `rurqe-core` | построители `SelectQuery`, `GroupedQuery`, `InsertQuery`, `UpdateQuery`, `DeleteQuery`, тип-состояния, `BuildError` |
| `rurqe-macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `rurqe` | точки входа `select_from`, `insert_into`, `update`, `delete_from`, прелюдия, `pin_dialect_version!` |

Специфика движков (capability-трейты, версии, диалектные расширения) разобрана в отдельных частях книги.

## Две формы точек входа

Фасад публикует четыре родовые функции (`crates/rurqe/src/lib.rs`):

| Вызов | Сигнатура |
|---|---|
| `select_from::<D, T>()` | `pub fn select_from<D: Dialect, T: Table>() -> SelectQuery<D, T, (), NoProjection>` |
| `insert_into::<D, T>()` | `pub fn insert_into<D: Dialect, T: Table>() -> InsertQuery<D, T, (), ()>` |
| `update::<D, T>()` | `pub fn update<D: Dialect, T: Table>() -> UpdateQuery<D, T, Unfiltered, ()>` |
| `delete_from::<D, T>()` | `pub fn delete_from<D: Dialect, T: Table>() -> DeleteQuery<D, T, Unfiltered, ()>` |

Диалектная прелюдия (`rurqe::postgres::prelude`, `rurqe::sqlite::sqlite3_35::prelude` и т. п.) генерируется макросом `pin_dialect_version!` и закрепляет диалект, поэтому таблица передаётся **значением**, без turbofish:

```rust
pub fn select_from<T: Table>(table: T) -> SelectQuery<$dialect, T, (), NoProjection>
pub fn insert_into<T: Table>(table: T) -> InsertQuery<$dialect, T, (), ()>
pub fn update<T: Table>(table: T) -> UpdateQuery<$dialect, T, Unfiltered, ()>
pub fn delete_from<T: Table>(table: T) -> DeleteQuery<$dialect, T, Unfiltered, ()>

pub type Error = $crate::core::query::ExecutionError<$error>;
pub type Result<T, E = Error> = ::core::result::Result<T, E>;

pub trait SelectFrom<T: Table, Positions>: Sized {
    type Row;
    fn from(self, table: T) -> SelectQuery<$dialect, T, Self::Row, HasProjection>;
}
```

Маркеры таблиц — ZST, поэтому аргумент не несёт данных: `select_from(Users)` и `select_from(users)` с точки зрения типа идентичны, различие лишь в стиле именования.

> **Compiler Guard in Action.** Диалектная прелюдия реэкспортирует общую прелюдию, а затем закреплённые точки входа, поэтому `use rurqe::postgres::pg16::prelude::*;` делает `select_from` версией, принимающей значение таблицы и возвращающей `SelectQuery<Postgres<Pg16>, ...>`. Ни диалект, ни версия в тексте запроса больше не упоминаются.

## Что возвращают точки входа

```rust
use rurqe::postgres::prelude::*;

let query = select_from(Users)          // SelectQuery<Postgres, Users, (), NoProjection>
    .select(cols!(UsersId, UsersName)); // SelectQuery<Postgres, Users, (i64, String), HasProjection>
```

Ряд строковых проекций `cols!(UsersId, UsersName)` даёт позиционный кортеж; `select!` — именованные поля; `dto!` — именованную структуру для декодирования.

## Карта тип-состояний

Построители защищают недостижимые конструкции **типом**, а не рантайм-проверкой.

| Тип-состояние | Где объявлено | Что запрещает |
|---|---|---|
| `NoProjection` / `HasProjection` | `query/select/markers.rs` | `.build()` до `.select(...)` |
| `NoLock` / `Locked` / `LockWaitSet` | `query/select/markers.rs` | `nowait()` до `for_update()`; повторный модификатор ожидания |
| `LockUnavailable` | `query/select/markers.rs` | `FOR UPDATE` на `DISTINCT` / `DISTINCT ON` / `GROUP BY` |
| `SetOpApplied` / `TailClosed` | `query/select/markers.rs` | `.select(...)` после `UNION`; операнд после сортировки комбинированных строк |
| `Unfiltered` / `Filtered` | `dml.rs` | `.build()` мутации без `WHERE` и без `.allow_all()` |
| `NoAssignments` / `HasAssignments` | `dml.rs` | `.build()` `UPDATE` без `SET` |
| `()` / `InsertSource` | `dml/insert.rs` | `.build()` `INSERT` без источника строк |

Плюс отдельный тип `GroupedQuery` (`query/group_by.rs`): после `.group_by(...)` запрос перестаёт быть `SelectQuery` и правило «колонка сгруппирована или агрегирована» проверяется на каждой проекции.

## Где это в коде

| Модуль | Содержимое |
|---|---|
| `rurqe-core/src/schema.rs` | `Table`, `Column`, `ColumnBelongsTo`, `TableRef`, `Aliased` |
| `rurqe-core/src/query/select.rs` | `SelectQuery`, `BuiltQuery`, `Query`/`LockedQuery` |
| `rurqe-core/src/query/select/{clauses,from,joins,locking,ordering,set_ops,render,transitions}.rs` | клаузы построителя |
| `rurqe-core/src/query/group_by.rs` | `GroupedQuery`, `GroupedSelectTarget`, `HavingPredicate` |
| `rurqe-core/src/query/window.rs` | `Window`, `WindowSpec`, `WindowExpr`, `lag`/`lead`/`row_number` |
| `rurqe-core/src/query/selection.rs` | `Selection`, `SelectionFor`, `SelectTargetInScope`, `Nullable` |
| `rurqe-core/src/query/validate.rs` | `BuildError` и предполётные правила |
| `rurqe-core/src/dml.rs` + `dml/{insert,update,delete,conflict,excluded}.rs` | типизированные мутации |
| `rurqe-macros/src/{select,dto,parse}.rs` | раскрытие `select!` и `dto!` |

## Главы части

1. [Схема: ZST-маркеры](schema.md)
2. [Проекции и `SELECT`](select.md)
3. [Предикаты и `WHERE`](filtering.md)
4. [Сортировка и пагинация](ordering.md)
5. [`GROUP BY` и `HAVING`](group_by.md)
6. [Оконные функции](window.md)
7. [Соединения](joins.md)
8. [Алиасы](aliasing.md)
9. [Подзапросы](subqueries.md)
10. [Общие табличные выражения (`WITH`)](cte.md)
11. [Операции над множествами](set_ops.md)
12. [DML: `INSERT`, `UPDATE`, `DELETE`](dml.md)

Каждая глава следует одной схеме: реляционный смысл → SQL → инвариант типа. Это то же соответствие, что введено в [«Кодд → Rust»](../theory/codd_to_rust.md).
