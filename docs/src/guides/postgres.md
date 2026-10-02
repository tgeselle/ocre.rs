# Migrating from Postgres

D1 is SQLite. An app moving from Postgres (Rails, Phoenix, Django) brings its tables and rows over with one command, then adapts what SQLite does differently.

## Convert the dump

```sh
pg_dump --no-owner --no-acl myapp_prod > dump.sql   # plain SQL, schema and data
ocre db import-postgres dump.sql
```

The command writes two files and loads nothing:

- `migrations/NNNN_import_from_postgres.sql`: one `CREATE TABLE` per table, with the primary keys, unique and foreign keys that `pg_dump` adds afterwards (`ALTER TABLE ... ADD CONSTRAINT`) folded in, as SQLite requires, and the plain indexes;
- `db/import_from_postgres.sql`: the rows of each `COPY` block as `INSERT`s, 100 rows (and at most 90 KB) per statement, under D1's statement size limit.

It prints the rows per table and a note for every difference to check. Read them, edit the migration if needed, then load the data locally and try the app:

```sh
ocre migrate
ocre db load db/import_from_postgres.sql
ocre sql "SELECT count(*) FROM videos"
```

On Cloudflare: `ocre migrate --remote`, then `ocre db load db/import_from_postgres.sql --remote` (the file goes in one request: for dumps over a few MB, split it, or use `npx wrangler d1 execute <database> --remote --file db/import_from_postgres.sql`). The free plan's D1 database holds 500 MB.

## How types change

| Postgres | SQLite (D1) | Values | Note |
|---|---|---|---|
| `smallint`, `integer`, `bigint`, `serial`, `bigserial` | `INTEGER` | as is | a `serial` primary key becomes `INTEGER PRIMARY KEY AUTOINCREMENT` |
| `real`, `double precision` | `REAL` | as is | |
| `numeric`, `decimal`, `money` | `TEXT` | the exact digits | Ocre's `decimal` field type; a `REAL` would round them |
| `boolean` | `INTEGER` | `t`/`f` become `1`/`0` | models read them with `ocre::bool_from_sql` |
| `uuid` | `TEXT` | as is | new rows need an id from the app: `gen_random_uuid()` defaults are dropped |
| `timestamp with time zone` | `TEXT` | converted to UTC, `YYYY-MM-DD HH:MM:SS` (fractions kept) | the format of `datetime('now')`, which Ocre's `created_at` uses |
| `timestamp`, `date`, `time` | `TEXT` | as is | |
| `json`, `jsonb` | `TEXT` + `CHECK (json_valid(...))` | as is | query with `json_extract(col, '$.key')` or `col ->> '$.key'`; Ocre's `json` field type |
| an `ENUM` type | `TEXT` + `CHECK (col IN (...))` | as is | Ocre's `enum` field type |
| arrays (`text[]`...) | `TEXT` | a JSON array of strings | read them with `json_each` |
| `bytea` | `BLOB` | hex literals | prefer R2 for files ([File storage](files.md)) |
| `citext` | `TEXT COLLATE NOCASE` | as is | case-insensitive for ASCII letters only |
| `interval`, `tsvector`, ranges, geometry... | `TEXT` | as is | noted: rework them in the app |

Defaults: `now()` and `CURRENT_TIMESTAMP` become `(datetime('now'))`, `true`/`false` become `1`/`0`, literal defaults stay (their `::type` casts dropped); other function calls are dropped with a note.

Skipped, with a note: functions, triggers, views, row security policies, indexes on expressions or with a method SQLite lacks (`gin`, `gist`). Their logic moves into the app: a trigger's work goes in the model's callbacks (`after_create`...), a view becomes a query function, full-text search uses a `LIKE` or a SQLite FTS table you create in a migration. Sequences, extensions, ownership and grants have no D1 equivalent and are dropped silently.

## Then

- Generate models for the tables: `ocre g model` writes the model and a `create_` migration; when the table exists already (from the import), keep the model file and delete the new migration, or write the model by hand on the imported columns ([Models and migrations](models.md)).
- Public ids: tables keyed by `uuid` keep their ids as text. For new rows, the app sets one (`ocre::token::public_id()` gives a random, URL-safe id); or move to `id INTEGER PRIMARY KEY` with a `public_id:token` column ([Field types](../reference/field-types.md#public_id)).
- Times: Ecto's `utc_datetime` and Rails' `datetime` columns are `timestamp without time zone` holding UTC already; they are copied as they are.

The converter reads `pg_dump`'s plain format (the default); a custom-format dump (`-Fc`) converts with `pg_restore -f dump.sql dump.custom` first. Dumps made with `--inserts` keep their `INSERT` statements as they are, `public.` removed. Checked on a `pg_dump` of PostgreSQL 16's layout (enums, `uuid`, `jsonb`, arrays, `bytea`, `timestamptz`, a trigger and its function, foreign keys added by `ALTER TABLE`), loaded into `ocre dev`'s D1.
