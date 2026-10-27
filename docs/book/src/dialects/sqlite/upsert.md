# Upsert: `ON CONFLICT`

SQLite получил `ON CONFLICT` в 3.24.0, поэтому клауза доступна для
`Sqlite<Sqlite3_24>` и новее через маркер `SupportsOnConflict`. Для
`Sqlite<Sqlite3_0>` методов upsert не существует, и попытка их вызвать —
ошибка компиляции с диагностикой маркера.

## Базовые формы

```rust
use rurqe::sqlite::prelude::*;

// Пропустить конфликтующую строку.
insert_into(users)
    .values((users.id, users.name.set("alice")))
    .on_conflict(users.id)
    .do_nothing();

// Обновить конфликтующую строку значением из отклонённой.
insert_into(users)
    .values((users.id, users.name.set("alice")))
    .on_conflict(users.id)
    .do_update((users.name.set(excluded(users.name)),));
```

## Предикаты конфликта

* `target_where(…)` сужает цель конфликта до строк, удовлетворяющих предикату
  — форма частичного уникального индекса, рендерится как
  `ON CONFLICT (…) WHERE …`.
* `update_where(…)` добавляет `WHERE` к `DO UPDATE`, то есть
  `… DO UPDATE SET … WHERE …`: обновляются только строки, прошедшие предикат.

Оба предиката рендерятся нативно — без `CASE WHEN`-эмуляции и без
клиентского round-trip.

## `excluded(col)` — типизированная ссылка на отклонённую строку

`excluded(col)` возвращает типизированную колонку псевдо-отношения `EXCLUDED`:
она рендерится как `"EXCLUDED"."col"` и проверяется по типу той колонки,
которую оборачивает, поэтому присваивание не может прочитать из отклонённой
строки значение другого рода.

## Чего здесь нет

`INSERT … SELECT` в `ON CONFLICT DO UPDATE` опирается на тот же маркер, что и
обычный upsert, но выбор диалекта не меняет его формы: SQLite принимает те же
`ON CONFLICT`/`DO UPDATE` клаузы, что и PostgreSQL, — отдельных SQLite-only
вариантов синтаксиса библиотека не синтезирует.
