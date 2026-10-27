# `RETURNING` и `UPDATE … FROM`

## `RETURNING` — с 3.35.0

`SupportsReturning` реализован для `Sqlite<Sqlite3_35>` и новее, поэтому
`returning(…)` существует на `INSERT`, `UPDATE` и `DELETE` только там. Он
проецирует отдельные колонки, кортежи или целые селекции записей и
декодируется так же, как проекция `SELECT`:

```rust
use rurqe::sqlite::prelude::*;

let new_id: i64 = insert_into(users)
    .values((
        users.email.set("user@example.com"),
        users.is_active.set(true),
    ))
    .returning(users.id)
    .fetch_one(&mut conn)?;
```

Для `Sqlite<Sqlite3_24>` метода нет вообще: версия не проходит порог, и
ошибка компиляции называет отсутствующий маркер, а не падает в рантайме.

## `UPDATE … FROM` — с 3.35.0

`SupportsUpdateFrom` позволяет читать присваиваемые значения из другого
отношения:

```rust
update(users)
    .set(users.name.set(staging.name))
    .from(staging)
    .where_(staging.id.eq(users.id));
```

Движок получил `UPDATE … FROM` в 3.33.0, но это ниже базовой версии диалекта
(`Sqlite3_35`), поэтому маркер начинается с ближайшего объявленного порога, а
не с фактической версии появления клаузы.

## Чего нет: `DELETE … USING`

У SQLite нет клаузы `DELETE … USING` ни в одной версии, поэтому маркер
`SupportsDeleteUsing` не реализован вообще, и метод, который бы её выпускал,
не существует. Удаление через другое отношение выражается обычным SQL,
написанным вручную; библиотека такой запрос не синтезирует.
