# Quickstart за 5 минут

Цель этого раздела — работающий запрос и понимание того, что именно проверяет компилятор. Все фрагменты ниже соответствуют реальному коду: сценарий взят из интеграционного теста `crates/rurqe/tests/sqlite_e2e.rs`.

## 1. Зависимость и фичи

```toml
[dependencies]
rurqe = { version = "0.1", features = ["sqlite"] }
# или для PostgreSQL:
# rurqe = { version = "0.1", features = ["postgres"] }
```

Адаптеры и их фичи:

| Фича | Что добавляет |
|---|---|
| `postgres` | `PgConnection` — синхронный драйвер `postgres` |
| `tokio-postgres` | `PgAsyncConnection` — асинхронный драйвер |
| `deadpool-postgres` | `PgAsyncPool` — асинхронный пул |
| `r2d2` | `PgPool` / `SqlitePool` — синхронные пулы |
| `sqlite` | `Sqlite<V>`, `SqliteConnection`, версии `Sqlite3_0` … `Sqlite3_38` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `migrate` | встроенный раннер миграций `EmbeddedMigrations` |

Любая фича PostgreSQL-адаптера компилирует поверхность диалекта, поэтому `Postgres`, `Pg14`, `Pg15`, `Pg16` доступны даже в сборке без синхронного драйвера.

## 2. Схема

```rust
use rurqe::prelude::*;

// Ручное объявление: два ZST на таблицу и по ZST на колонку.
table! {
    /// Таблица `users`.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
        UserScore => "score": i32,
    }
}
```

В реальном проекте этот код не пишется руками: `cargo db-codegen` генерирует его из живого каталога базы.

## 3. Подключение

```rust
use rurqe::sqlite::rusqlite::Connection;

let connection = Connection::open_in_memory()?;
connection.execute_batch(
    "CREATE TABLE users (\
         id INTEGER NOT NULL PRIMARY KEY,\
         name TEXT NOT NULL,\
         active BOOLEAN NOT NULL,\
         score INTEGER NOT NULL);",
)?;
let mut db = SqliteConnection::new(connection);
```

## 4. Запись и чтение

```rust
// INSERT ... RETURNING: возвращает спроецированную строку.
let insert = insert_into::<Sqlite<Sqlite3_35>, User>()
    .values((
        UserId::set(1i64),
        UserName::set("root"),
        UserActive::set(true),
        UserScore::set(42i32),
    ))
    .returning((UserId, UserName, UserActive, UserScore));

let row: (i64, String, bool, i32) = db.fetch_one(&insert)?;
assert_eq!(row, (1i64, "root".to_owned(), true, 42i32));

// SELECT с типизированным предикатом и сортировкой.
let query = select_from::<Sqlite<Sqlite3_35>, User>()
    .select((UserId, UserName, UserScore))
    .where_(Expr::column::<UserActive>().eq(true))
    .order_by_asc(UserId);

let rows: Vec<(i64, String, i32)> = db.fetch_all(&query)?;
```

`values` проверяет тип каждого значения против `Column::RustType` в точке вызова: `UserId::set("1")` не скомпилируется.

## 5. Мутации

```rust
let update = update::<Sqlite<Sqlite3_35>, User>()
    .set(UserName::set("bob"))
    .where_(Expr::column::<UserId>().eq(1i64))
    .returning((UserName, UserScore));
let renamed: (String, i32) = db.fetch_one(&update)?;

let delete = delete_from::<Sqlite<Sqlite3_35>, User>()
    .where_(Expr::column::<UserId>().eq(1i64));
let affected: usize = db.execute(&delete)?;
assert_eq!(affected, 1);
```

`update(...)` и `delete_from(...)` стартуют в состоянии «не фильтровано»: `.build()` недоступен, пока не вызван `.where_(...)` или явный `.allow_all()`. Случайное массовое удаление невозможно по построению.

## 6. Без turbofish: диалектные прелюдии

Диалект можно зафиксировать один раз импортом:

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;

let rows = select_from(User)          // таблица передаётся значением
    .select(cols!(UserId, UserName))  // Sqlite3_35 зафиксирован
    .fetch_all(&mut db)?;
```

Для PostgreSQL:

```rust
use rurqe::postgres::pg16::prelude::*;

let built = select_from(Users).select(cols!(UsersId)).build()?;
```

Сгенерированный прелюдией модуль определяет `select_from`, `insert_into`, `update`, `delete_from`, алиасы `Error` и `Result<T, E = Error>`, трейт `SelectFrom` для `.from(table)` и вложенный `prelude`, реэкспортирующий общую прелюдию с приоритетом закреплённых точек входа.

## 7. Ошибки — это типы

```rust
let query = select_from::<Sqlite<Sqlite3_35>, User>()
    .select((UserName,))
    .where_(Expr::column::<UserId>().eq(1i64));

match db.fetch_one(&query) {
    Ok((name,)) => println!("{name}"),
    Err(error) if error.is_not_found() => println!("нет такой строки"),
    Err(error) => return Err(error.into()),
}
```

`ExecutionError<E>` различает сборку запроса (`is_build`), отсутствие строки (`is_not_found`), лишние строки (`is_multiple_rows`), ошибку декодирования (`is_row_decode`) и ошибку драйвера (`as_backend`, `into_backend`).

## 8. Что дальше

- Почему `LEFT JOIN` меняет тип колонки — [Nullability](../theory/nullability.md).
- Где проходит граница проверок компилятора и `.build()` — [Two-Tier SQL Correctness](two_tier.md).
- Как избавиться от ручного `table!` — глава про кодогенерацию (отдельный раздел книги).
