# JSON и вложенные коллекции

## Вложенная коллекция без `jsonb_agg`

`nest_many` / `nest_one` рендерят вложенный набор строк как JSON-агрегат
движка. У SQLite нет агрегации строки в JSON (`SUPPORTS_JSONB_ROW_AGG =
false`), поэтому документ собирается поле за полем через `json_object` и
склеивается `json_group_array`:

```sql
COALESCE((SELECT json_group_array(json_object('sku', "__rurqe_sub"."sku",
                                            'qty', "__rurqe_sub"."qty"))
          FROM (SELECT "sku" AS "sku", "qty" AS "qty" FROM "lines")
               AS "__rurqe_sub"), '[]')
```

Каждое проецируемое выражение вложенного запроса получает имя поля, которое
оно питает, поэтому документ строки несёт ровно те ключи, что объявляет тип
строки, а клаузы вложенного запроса (фильтр, порядок, лимит) остаются в силе.

Для `nest_one` форма отличается двумя деталями: агрегат не оборачивается в
`COALESCE`, а лимит вложенного запроса становится `1` (или лимитом
компаундного запроса, если внутри есть `UNION`), поэтому отсутствие ребёнка
декодируется как `None`, а не как пустой список:

```sql
(SELECT json_object('id', "__rurqe_sub"."id")
 FROM (SELECT "id" AS "id" FROM "profile" LIMIT 1) AS "__rurqe_sub")
```

## Порог версии — 3.38.0

JSON-функции были опциональным расширением сборки до 3.38.0, поэтому вложенная
проекция гейтится маркером `SupportsJsonAggregation`, реализованным только для
`Sqlite<Sqlite3_38>` и требованиями `SelectTargetInScope`:

```rust
impl<Tables, InnerD, QueryD, S, Row> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestMany<SelectQuery<InnerD, S, Row, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    /* … */
```

То есть statement, проецирующий вложенную коллекцию, обязан строиться для
диалекта с агрегацией; для `Sqlite<Sqlite3_35>` проекция не компилируется.

## Декодирование документа

SQLite отдаёт документ вложенной строки как текст, PostgreSQL — как JSON-значение.
Обе формы принимает `json_object`, а поля читает `nested_field`, поэтому сгенерированный
`dto!`/`select!` код одинаков для обоих движков.

## Массив как параметр

Столбца-массива в SQLite нет, поэтому `SqlVal::Array` привязывается драйвером
как JSON-документ элементов (`[1,2]`, `["root","guest"]`). Это та же форма,
которую `Vec<T>::from_sql_value` разбирает обратно в элементы, и та же, в
которой приходит агрегированный набор строк. Привязка PostgreSQL-ной записи
`{...}` не годилась бы: SQLite не хранит её как документ и не декодирует
обратно в коллекцию. Не-финитные числа (`NaN`, бесконечности) записываются в
документ как `null`, потому что JSON не несёт для них нотации.
