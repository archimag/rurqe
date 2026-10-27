# Движки, capability-гейты и декодирование

## `SupportsJsonAggregation`

Возможность вложенной проекции выражена маркерным трейтом в `crates/rurqe-core/src/dialect.rs`:

```rust
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support JSON aggregation of a row set",
    label = "JSON array and object aggregation is not supported by this dialect",
    note = "nested collections and objects require a dialect that implements `SupportsJsonAggregation` (such as PostgreSQL or SQLite 3.38+)"
)]
pub trait SupportsJsonAggregation: Dialect {}
```

Требование предъявляет сама проекция: `QueryD: SupportsJsonAggregation` в `SelectTargetInScope<Tables, At<0>, QueryD>` для `NestMany` и `NestOne`. Движок без этой возможности не получает SQL, который не сможет разобрать.

## Форма рендеринга и версии

| Диалект | `SUPPORTS_JSONB_ROW_AGG` | Форма |
|---|---|---|
| `Postgres<V>` | `true` | `jsonb_agg(to_jsonb(...))` / `to_jsonb(...)` |
| `Sqlite<Sqlite3_38>` | `false` | `json_group_array(json_object(...))` / `json_object(...)` |

JSON-функции SQLite были опциональным расширением сборки до 3.38.0, поэтому маркер реализует `Sqlite<Sqlite3_38>`, а `Sqlite<Sqlite3_35>` его не реализует и вложенную проекцию не принимает. Базовое значение `Dialect::SUPPORTS_JSONB_ROW_AGG` равно `false`: нативную агрегацию строки диалект объявляет явно.

## Диалектная привязка

```rust
InnerD: Dialect + DialectInScope<QueryD>
```

Вложенное утверждение принадлежит тому же диалекту, что и объемлющий запрос (либо диалект-нейтрально), поэтому коллекция PostgreSQL не вкладывается в утверждение SQLite.

## Декодирование

PostgreSQL отдаёт документ вложенной строки как JSON-значение (`SqlVal::Json`), SQLite — как текст (`SqlVal::Text`). Обе формы принимает `json_object`; поля читает `nested_field`, декодируя каждое тем же контрактом `FromSqlValue`, что и колонку того же типа. Коллекция собирается через `SqlArrayElement`: `Vec<Row>` и `Option<Row>` декодируются из документа без ручного разбора JSON.

| Тип значения | Представление в документе |
|---|---|
| `i32`, `i64` | число |
| `f64` | число; `NaN` и бесконечности — `null` |
| `bool` | булево |
| `String` | строка |
| `Vec<u8>` | канонический hex-текст, который `FromSqlValue for Vec<u8>` читает обратно |
| `Decimal` (фича `decimal`) | число, если все цифры сохраняются, иначе текст |
| `Uuid`, `NaiveDateTime`, `DateTime<Utc>`, `NaiveDate` | текст `Display`, который `FromSqlValue` парсит обратно |
| вложенный JSON | как есть |

## Обратное направление

`multiset_object(&[(&str, SqlVal)])` собирает документ строки из значений полей, `sql_val_to_json` переводит SQL-значение в JSON. Этим документ строки привязывается параметром и читается обратно по тем же ключам.

## Фича `serde_json`

Без фичи `serde_json` документа нет: `RowObject` вырождается в пустую структуру, `json_object` возвращает `RowDecodeError::InvalidValue` с деталью о недостающей фиче, `multiset_object` — `SqlVal::EncodingError`. Функции при этом остаются вызываемыми — именно это позволяет раскрытиям `dto!` и `select!` компилироваться в любой сборке, а причину сообщить в месте декодирования.

## Итог требований

| Требование | Где проверяется |
|---|---|
| Диалект вложенного запроса совпадает с диалектом запроса | `DialectInScope<QueryD>`, компиляция |
| Диалект умеет JSON-агрегацию строки | `SupportsJsonAggregation`, компиляция |
| Тип строки объявляет ключи документа | `MultisetRow::FIELDS`, компиляция |
| Вложенный запрос проецирует строки | `HasProjection`, компиляция |
| Параметры сырых фрагментов совпадают с плейсхолдерами | `RawSqlError`, рендеринг |

## Связанные главы

- [Вложенные результаты: nest_many и nest_one](nested.md)
- [MULTISET: как вложенность превращается в SQL](multiset.md)
- [JSON и вложенные коллекции (SQLite)](../dialects/sqlite/json.md)
