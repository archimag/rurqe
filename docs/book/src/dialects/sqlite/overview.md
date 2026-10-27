# SQLite

`rurqe-sqlite` — диалект SQLite, маркеры версий движка и рантайм-адаптеры
поверх `rusqlite`. Крейт устроен так же, как `rurqe-postgres`: `dialect`
описывает рендеринг и версионные пороги, а `backend` — слой значений,
соединение и пул.

## Тип диалекта

```rust
pub struct Sqlite<V: SqliteVersion = Sqlite3_35>(PhantomData<fn() -> V>);
```

Параметр версии по умолчанию — `Sqlite3_35`; объявлены маркеры `Sqlite3_0`,
`Sqlite3_24`, `Sqlite3_25`, `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38`. Версия
доказывает возможности, а не меняет рендеринг — см.
[capabilities](capabilities.md) и [versions](../versions.md).

## Правила рендеринга

Идентификаторы заключаются в двойные кавычки, внутренняя кавычка удваивается:

```text
users        →  "users"
user"name    →  "user""name"
```

Плейсхолдеры анонимные: индекс параметра игнорируется, потому что SQLite
нумерует параметры позиционно.

```text
параметр 1   →  ?
параметр 42  →  ?
```

Два флага `Dialect` меняют не написание, а семантику SQL, поэтому они
объявлены константами, а не спрятаны в телах методов:

* `SUPPORTS_ILIKE = false` — у SQLite нет `ILIKE`, есть только регистрозависимый
  `LIKE`. Предикат `ilike` сохраняет регистронезависимую семантику, складывая
  оба операнда через `LOWER`, то есть рендерится как
  `(LOWER("name") LIKE LOWER(?))`. `LOWER` складывает только ASCII-буквы, что
  делает обход независимым от коллации базы.
* `SUPPORTS_JSONB_ROW_AGG = false` — у SQLite нет агрегации строки в JSON,
  поэтому вложенная коллекция собирается поле за полем через `json_object` и
  склеивается `json_group_array` — см. [json](json.md).

## Возможности и ограничения

Каждая возможность движка вынесена в маркерный трейт ядра и доказана для
конкретного диапазона версий: `ON CONFLICT` с 3.24, окна с 3.25, `FILTER
(WHERE …)` и явная сортировка NULL с 3.30, `RETURNING` и `UPDATE … FROM` с
3.35, JSON-агрегация с 3.38.

| Класс | Глава |
|---|---|
| версионные пороги и полная таблица маркеров | [capabilities](capabilities.md) |
| `ON CONFLICT`, `EXCLUDED`, условные предикаты | [upsert](upsert.md) |
| `FILTER (WHERE …)`, окна, `NULLS FIRST/LAST` | [filter](filter.md) |
| `RETURNING`, `UPDATE … FROM` | [returning](returning.md) |
| чего нет и почему нет эмуляции | [limitations](limitations.md) |
| вложенные коллекции и привязка массивов | [json](json.md) |
| перечисления через `CHECK (col IN (…))` | [enums](enums.md) |

## Драйверы

Адаптеры повторяют слоение PostgreSQL-крейта: слой значений (`backend/types`),
соединение `rusqlite` (`backend/rusqlite`) и пул `r2d2` (`backend/pools`).
Каждый слой закрыт фичей собственной зависимости, поэтому сборка без драйвера
компилирует пустой модуль.

```rust
use rurqe::sqlite::prelude::*;

let mut db = SqliteConnection::open_in_memory()?;
let rows = select_from(users)
    .select((users.id, users.name))
    .where_(users.is_active.eq(true))
    .fetch_all(&mut db)?;
```

Драйвер и модель конкурентности миграций описаны в [driver](driver.md) и
[concurrency](concurrency.md).
