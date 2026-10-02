//! `ocre db import-postgres`: a `pg_dump` (plain SQL) turned into a D1
//! migration (the tables, in SQLite's types) and a data file (the rows of
//! its `COPY` blocks as `INSERT`s), with a note for every change of type or
//! anything left behind.

use std::{collections::BTreeMap, fmt::Write as _};

use crate::{
    CliResult,
    generate::{Edits, next_migration_path},
    output::CliError,
    project::Project,
};

/// Rows per `INSERT`: well under D1's statement size limit for most tables.
const ROWS_PER_INSERT: usize = 100;
/// Longest `INSERT`, in bytes (D1 refuses statements over 100 KB).
const MAX_INSERT_BYTES: usize = 90 * 1024;

/// What `ocre db import-postgres` makes of a dump.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Conversion {
    /// `CREATE TABLE` and `CREATE INDEX` statements for D1.
    pub schema: String,
    /// `INSERT` statements with the rows.
    pub data: String,
    /// Type changes, dropped defaults, skipped statements: what to check.
    pub notes: Vec<String>,
    /// Rows converted, per table.
    pub rows: BTreeMap<String, usize>,
}

/// How a column's values are written in the data file.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Integer,
    Real,
    Text,
    Boolean,
    /// `timestamp with time zone`: converted to UTC.
    TimestampTz,
    Bytes,
    /// A Postgres array: written as a JSON array of strings.
    Array,
}

#[derive(Debug)]
struct Column {
    name: String,
    sql: String,
    kind: Kind,
}

#[derive(Debug, Default)]
struct Table {
    columns: Vec<Column>,
    /// Constraints from the `CREATE TABLE` and from `ALTER TABLE ... ADD CONSTRAINT`.
    constraints: Vec<String>,
    /// Columns filled by `nextval(...)`: an integer primary key does it in SQLite.
    serial: Vec<String>,
}

/// Converts a plain-SQL `pg_dump` (`pg_dump --no-owner --no-acl db > dump.sql`).
pub(crate) fn convert(dump: &str) -> Result<Conversion, CliError> {
    let mut out = Conversion::default();
    let mut enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut tables: Vec<(String, Table)> = Vec::new();
    let mut indexes = Vec::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for statement in statements(dump) {
        let text = statement.sql.trim();
        let upper = text.to_ascii_uppercase();
        if let Some(copy) = statement.copy {
            let (table, columns) = copy_target(text)?;
            let Some((_, definition)) = tables.iter().find(|(name, _)| *name == table) else {
                out.notes.push(format!("{table}: data skipped, no CREATE TABLE before it"));
                continue;
            };
            let kinds: Vec<Kind> = columns
                .iter()
                .map(|column| definition.columns.iter().find(|c| &c.name == column).map_or(Kind::Text, |c| c.kind))
                .collect();
            let count = copy_rows(&mut out.data, &table, &columns, &kinds, &copy)?;
            *out.rows.entry(table).or_default() += count;
        } else if upper.starts_with("CREATE TYPE") && upper.contains("AS ENUM") {
            let name = unqualify(text["CREATE TYPE".len()..].split_whitespace().next().unwrap_or_default());
            let values = text[text.find('(').unwrap_or(0)..].trim_matches(|c| c == '(' || c == ')' || c == ';');
            enums.insert(name, values.split(',').map(|v| v.trim().trim_matches('\'').replace("''", "'")).collect());
        } else if upper.starts_with("CREATE TABLE") {
            let (name, table) = create_table(text, &enums, &mut out.notes)?;
            tables.push((name, table));
        } else if upper.starts_with("ALTER TABLE") {
            alter_table(text, &mut tables, &mut out.notes);
        } else if upper.starts_with("CREATE INDEX") || upper.starts_with("CREATE UNIQUE INDEX") {
            match index(text) {
                Some(sql) => indexes.push(sql),
                None => {
                    out.notes.push(format!("index skipped (expression or method SQLite lacks): {}", first_line(text)))
                }
            }
        } else if let Some(kind) = ignored(&upper) {
            if !kind.is_empty() {
                *skipped.entry(kind.to_owned()).or_default() += 1;
            }
        } else if upper.starts_with("INSERT INTO") {
            out.data.push_str(&text.replace("public.", ""));
            out.data.push_str(";\n");
            *out.rows.entry("(INSERT statements)".to_owned()).or_default() += 1;
        } else {
            out.notes.push(format!("statement skipped: {}", first_line(text)));
        }
    }
    for (kind, count) in skipped {
        out.notes.push(format!("{count} {kind} skipped: rewrite their logic in Rust (models' callbacks, jobs)"));
    }
    for (name, table) in &tables {
        writeln!(out.schema, "CREATE TABLE {name} (").expect("writing to a String");
        let mut lines: Vec<String> = table
            .columns
            .iter()
            .map(|column| {
                let serial_key = table.serial.contains(&column.name)
                    && table.constraints.iter().any(|c| c == &format!("PRIMARY KEY ({})", column.name));
                if serial_key {
                    format!("    {} INTEGER PRIMARY KEY AUTOINCREMENT", column.name)
                } else {
                    format!("    {} {}", column.name, column.sql)
                }
            })
            .collect();
        for constraint in &table.constraints {
            let single_serial_key = table.serial.iter().any(|column| constraint == &format!("PRIMARY KEY ({column})"));
            if !single_serial_key {
                lines.push(format!("    {constraint}"));
            }
        }
        writeln!(out.schema, "{}\n);\n", lines.join(",\n")).expect("writing to a String");
    }
    for sql in indexes {
        writeln!(out.schema, "{sql}").expect("writing to a String");
    }
    if tables.is_empty() {
        return Err(CliError::new("no CREATE TABLE in the dump")
            .hint("dump the schema too, in plain SQL: `pg_dump --no-owner --no-acl <database> > dump.sql`"));
    }
    Ok(out)
}

/// A statement of the dump, with the data lines of a `COPY ... FROM stdin`.
struct Statement {
    sql: String,
    copy: Option<Vec<String>>,
}

/// Splits the dump into statements: `;` outside quotes, dollar quotes and comments.
fn statements(dump: &str) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut lines = dump.lines();
    let (mut quote, mut dollar): (Option<char>, Option<String>) = (None, None);
    while let Some(line) = lines.next() {
        let mut chars = line.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match (&dollar, quote, c) {
                (Some(tag), _, '$') if line[i..].starts_with(tag.as_str()) => {
                    current.push_str(tag);
                    for _ in 1..tag.len() {
                        chars.next();
                    }
                    dollar = None;
                    continue;
                }
                (Some(_), _, _) => {}
                (None, Some(q), _) if c == q => quote = None,
                (None, Some(_), _) => {}
                (None, None, '\'' | '"') => quote = Some(c),
                (None, None, '$') => {
                    let tag: String =
                        line[i..].chars().skip(1).take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    if line[i + 1 + tag.len()..].starts_with('$') {
                        let tag = format!("${tag}$");
                        current.push_str(&tag);
                        for _ in 1..tag.len() {
                            chars.next();
                        }
                        dollar = Some(tag);
                        continue;
                    }
                }
                (None, None, '-') if line[i..].starts_with("--") => break,
                (None, None, ';') => {
                    let sql = std::mem::take(&mut current);
                    let copy = sql.trim_start().to_ascii_uppercase().starts_with("COPY ")
                        && sql.to_ascii_uppercase().contains("FROM STDIN");
                    let data =
                        copy.then(|| lines.by_ref().take_while(|line| *line != "\\.").map(str::to_owned).collect());
                    if !sql.trim().is_empty() {
                        out.push(Statement { sql, copy: data });
                    }
                    continue;
                }
                _ => {}
            }
            current.push(c);
        }
        current.push('\n');
    }
    out
}

/// Statements with nothing to carry over, by the kind reported (empty: not reported).
fn ignored(upper: &str) -> Option<&'static str> {
    const SILENT: [&str; 11] = [
        "SET ",
        "SELECT PG_CATALOG.",
        "CREATE SEQUENCE",
        "ALTER SEQUENCE",
        "CREATE SCHEMA",
        "CREATE EXTENSION",
        "COMMENT ON",
        "GRANT ",
        "REVOKE ",
        "SELECT SETVAL",
        "ALTER DEFAULT PRIVILEGES",
    ];
    if SILENT.iter().any(|prefix| upper.starts_with(prefix)) || upper.contains(" OWNER TO ") {
        return Some("");
    }
    [
        ("CREATE FUNCTION", "functions"),
        ("CREATE OR REPLACE FUNCTION", "functions"),
        ("CREATE TRIGGER", "triggers"),
        ("CREATE VIEW", "views"),
        ("CREATE OR REPLACE VIEW", "views"),
        ("CREATE MATERIALIZED VIEW", "materialized views"),
        ("CREATE POLICY", "row security policies"),
    ]
    .iter()
    .find(|(prefix, _)| upper.starts_with(prefix))
    .map(|(_, kind)| *kind)
}

/// `public.videos` -> `videos`; quotes removed.
fn unqualify(name: &str) -> String {
    let name = name.trim().trim_end_matches([';', '(']);
    name.rsplit('.').next().unwrap_or(name).trim_matches('"').to_owned()
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.len() > 100 { format!("{}...", &line[..line.floor_char_boundary(100)]) } else { line.to_owned() }
}

/// Splits on commas outside parentheses and quotes.
fn split_top_level(text: &str) -> Vec<String> {
    let (mut parts, mut current, mut depth, mut quote) = (Vec::new(), String::new(), 0, None);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current).trim().to_owned());
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    parts
}

fn create_table(
    text: &str,
    enums: &BTreeMap<String, Vec<String>>,
    notes: &mut Vec<String>,
) -> Result<(String, Table), CliError> {
    let open = text.find('(').ok_or_else(|| CliError::new(format!("cannot read `{}`", first_line(text))))?;
    let name = unqualify(text["CREATE TABLE".len()..open].trim().trim_start_matches("IF NOT EXISTS").trim());
    let body = &text[open + 1..text.rfind(')').unwrap_or(text.len())];
    let mut table = Table::default();
    for part in split_top_level(body) {
        let upper = part.to_ascii_uppercase();
        if ["CONSTRAINT", "PRIMARY KEY", "UNIQUE", "FOREIGN KEY", "CHECK"].iter().any(|k| upper.starts_with(k)) {
            table.constraints.push(constraint(&part));
            continue;
        }
        let mut words = part.splitn(2, char::is_whitespace);
        let column = words.next().unwrap_or_default().trim_matches('"').to_owned();
        let rest = words.next().unwrap_or_default().trim();
        let (sql, kind) = column_sql(&name, &column, rest, enums, notes);
        if rest.to_ascii_uppercase().contains("NEXTVAL(") {
            table.serial.push(column.clone());
        }
        table.columns.push(Column { name: column, sql, kind });
    }
    Ok((name, table))
}

/// A table constraint for SQLite: its name dropped, references unqualified.
fn constraint(text: &str) -> String {
    let text = text.trim().trim_end_matches(';');
    let body = if text.to_ascii_uppercase().starts_with("CONSTRAINT") {
        text.splitn(3, char::is_whitespace).nth(2).unwrap_or_default().trim()
    } else {
        text
    };
    body.replace("public.", "").replace(" NOT VALID", "").replace(" DEFERRABLE INITIALLY DEFERRED", "")
}

/// `ALTER TABLE ONLY public.t ADD CONSTRAINT ...` and `ALTER COLUMN ... SET DEFAULT nextval(...)`.
fn alter_table(text: &str, tables: &mut [(String, Table)], notes: &mut Vec<String>) {
    let rest = text["ALTER TABLE".len()..].trim().trim_start_matches("ONLY").trim();
    let (name, action) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = unqualify(name);
    let Some((_, table)) = tables.iter_mut().find(|(table, _)| *table == name) else { return };
    let action = action.trim();
    let upper = action.to_ascii_uppercase();
    if upper.starts_with("ADD CONSTRAINT") {
        table.constraints.push(constraint(&action["ADD ".len()..]));
    } else if upper.starts_with("ALTER COLUMN") && upper.contains("NEXTVAL(") {
        let column = action.split_whitespace().nth(2).unwrap_or_default().trim_matches('"').to_owned();
        table.serial.push(column);
    } else if !upper.contains("OWNER TO") {
        notes.push(format!("{name}: skipped `ALTER TABLE ... {}`", first_line(action)));
    }
}

/// The SQLite column definition of a Postgres one, and how its values are written.
fn column_sql(
    table: &str,
    column: &str,
    definition: &str,
    enums: &BTreeMap<String, Vec<String>>,
    notes: &mut Vec<String>,
) -> (String, Kind) {
    let upper = definition.to_ascii_uppercase();
    // The type runs until the first constraint keyword.
    let end = [
        " NOT NULL",
        " NULL",
        " DEFAULT ",
        " PRIMARY KEY",
        " UNIQUE",
        " REFERENCES ",
        " CHECK ",
        " COLLATE ",
        " GENERATED ",
    ]
    .iter()
    .filter_map(|keyword| upper.find(keyword))
    .min()
    .unwrap_or(definition.len());
    let pg = definition[..end].trim();
    let pg_lower = pg.to_ascii_lowercase();
    let base = pg_lower.split('(').next().unwrap_or_default().trim().trim_start_matches("public.").to_owned();
    let at = format!("{table}.{column}");
    let mut check = None;
    let (sqlite, kind) = if pg_lower.ends_with("[]") {
        notes.push(format!("{at}: {pg} -> TEXT holding a JSON array of strings"));
        ("TEXT", Kind::Array)
    } else {
        match base.as_str() {
            "smallint" | "integer" | "int" | "int2" | "int4" | "int8" | "bigint" | "serial" | "bigserial"
            | "smallserial" => ("INTEGER", Kind::Integer),
            "real" | "double precision" | "float4" | "float8" => ("REAL", Kind::Real),
            "numeric" | "decimal" | "money" => {
                notes.push(format!("{at}: {pg} -> TEXT, exact (Ocre's `decimal` type); a REAL would round it"));
                ("TEXT", Kind::Text)
            }
            "boolean" | "bool" => ("INTEGER", Kind::Boolean),
            "uuid" => {
                notes.push(format!(
                    "{at}: uuid -> TEXT; new rows need an id from the app (`ocre::token::public_id()`, or a UUID crate)"
                ));
                ("TEXT", Kind::Text)
            }
            "timestamp with time zone" | "timestamptz" => {
                notes.push(format!(
                    "{at}: {pg} -> TEXT `YYYY-MM-DD HH:MM:SS`, converted to UTC (`datetime('now')`'s format)"
                ));
                ("TEXT", Kind::TimestampTz)
            }
            "timestamp" | "timestamp without time zone" | "date" | "time" | "time without time zone" => {
                ("TEXT", Kind::Text)
            }
            "json" | "jsonb" => {
                check = Some(format!("json_valid({column})"));
                notes.push(format!(
                    "{at}: {pg} -> TEXT checked with json_valid; query it with SQLite's json_extract or `->>`"
                ));
                ("TEXT", Kind::Text)
            }
            "bytea" => ("BLOB", Kind::Bytes),
            "text" | "character varying" | "varchar" | "character" | "char" | "citext" | "inet" | "cidr"
            | "macaddr" | "name" => {
                if base == "citext" {
                    notes.push(format!("{at}: citext -> TEXT COLLATE NOCASE (ASCII case only)"));
                }
                ("TEXT", Kind::Text)
            }
            other => match enums.get(other) {
                Some(values) => {
                    let list =
                        values.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect::<Vec<_>>().join(", ");
                    check = Some(format!("{column} IN ({list})"));
                    notes.push(format!("{at}: enum {other} -> TEXT checked against its values (Ocre's `enum` type)"));
                    ("TEXT", Kind::Text)
                }
                None => {
                    notes.push(format!("{at}: {pg} has no SQLite equivalent -> TEXT; check the values"));
                    ("TEXT", Kind::Text)
                }
            },
        }
    };
    let mut sql = sqlite.to_owned();
    if base == "citext" {
        sql.push_str(" COLLATE NOCASE");
    }
    if upper.contains("NOT NULL") {
        sql.push_str(" NOT NULL");
    }
    if let Some(start) = upper.find(" DEFAULT ") {
        let value = definition[start + " DEFAULT ".len()..].trim();
        let value_upper = value.to_ascii_uppercase();
        let cut = [" NOT NULL", " NULL", " PRIMARY KEY", " UNIQUE", " REFERENCES ", " CHECK "]
            .iter()
            .filter_map(|keyword| value_upper.find(keyword))
            .min()
            .unwrap_or(value.len());
        let value = value[..cut].trim();
        let value_lower = value.to_ascii_lowercase();
        if value_lower.starts_with("nextval(") {
            // An INTEGER PRIMARY KEY numbers rows itself.
        } else if value_lower == "now()" || value_lower.starts_with("current_timestamp") {
            sql.push_str(" DEFAULT (datetime('now'))");
        } else if value_lower == "true" || value_lower == "false" {
            sql.push_str(if value_lower == "true" { " DEFAULT 1" } else { " DEFAULT 0" });
        } else if value.starts_with('\'') || value.parse::<f64>().is_ok() || value_lower == "null" {
            let literal = value.split("::").next().unwrap_or(value);
            write!(sql, " DEFAULT {literal}").expect("writing to a String");
        } else {
            notes.push(format!("{at}: default `{value}` dropped (no SQLite equivalent): set it when inserting"));
        }
    }
    if let Some(start) = upper.find(" REFERENCES ") {
        let reference = definition[start..].trim();
        sql.push(' ');
        sql.push_str(&reference.replace("public.", ""));
    } else {
        for keyword in [" PRIMARY KEY", " UNIQUE"] {
            if upper.contains(keyword) {
                sql.push_str(keyword);
            }
        }
    }
    if let Some(check) = check {
        write!(sql, " CHECK ({check})").expect("writing to a String");
    }
    (sql, kind)
}

/// `CREATE [UNIQUE] INDEX name ON [ONLY] public.t USING btree (a, b)` for SQLite; `None` for expressions.
fn index(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let unique = if upper.starts_with("CREATE UNIQUE") { "UNIQUE " } else { "" };
    let on = upper.find(" ON ")?;
    let name = text[..on].split_whitespace().last()?.trim_matches('"');
    let rest = text[on + 4..].trim().trim_start_matches("ONLY ").trim();
    let open = rest.find('(')?;
    let table = unqualify(rest[..open].split_whitespace().next()?);
    let method = rest[..open].to_ascii_lowercase();
    if method.contains("using") && !method.contains("btree") {
        return None;
    }
    let close = rest.rfind(')')?;
    let columns = &rest[open + 1..close];
    let plain = columns.split(',').all(|column| {
        let column = column.trim().trim_end_matches(" DESC").trim_end_matches(" ASC");
        !column.is_empty() && column.trim_matches('"').chars().all(|c| c.is_alphanumeric() || c == '_')
    });
    let filter = &rest[close + 1..];
    if !plain || filter.to_ascii_uppercase().contains("WHERE") {
        return None;
    }
    Some(format!("CREATE {unique}INDEX {name} ON {table} ({columns});"))
}

/// `COPY public.t (a, b) FROM stdin` -> (`t`, [`a`, `b`]).
fn copy_target(text: &str) -> Result<(String, Vec<String>), CliError> {
    let rest = text["COPY".len()..].trim();
    let open = rest.find('(').ok_or_else(|| CliError::new(format!("cannot read `{}`", first_line(text))))?;
    let close = rest.find(')').unwrap_or(rest.len());
    let columns = rest[open + 1..close].split(',').map(|c| c.trim().trim_matches('"').to_owned()).collect();
    Ok((unqualify(&rest[..open]), columns))
}

/// Appends the `COPY` rows as `INSERT`s; returns how many.
fn copy_rows(
    data: &mut String,
    table: &str,
    columns: &[String],
    kinds: &[Kind],
    lines: &[String],
) -> Result<usize, CliError> {
    let head = format!("INSERT INTO {table} ({}) VALUES\n", columns.join(", "));
    let mut batch: Vec<String> = Vec::new();
    let mut size = 0;
    let flush = |data: &mut String, batch: &mut Vec<String>| {
        if !batch.is_empty() {
            writeln!(data, "{head}{};", batch.join(",\n")).expect("writing to a String");
            batch.clear();
        }
    };
    for (n, line) in lines.iter().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != columns.len() {
            return Err(CliError::new(format!(
                "{table}: row {} has {} values for {} columns",
                n + 1,
                fields.len(),
                columns.len()
            ))
            .hint("the dump must come from `pg_dump` in plain format, its COPY data unchanged"));
        }
        let values: Vec<String> =
            fields.iter().zip(kinds).map(|(field, kind)| literal(field, *kind)).collect::<Result<_, _>>()?;
        let row = format!("({})", values.join(", "));
        if batch.len() == ROWS_PER_INSERT || size + row.len() > MAX_INSERT_BYTES {
            flush(data, &mut batch);
            size = 0;
        }
        size += row.len() + 2;
        batch.push(row);
    }
    flush(data, &mut batch);
    Ok(lines.len())
}

/// A `COPY` field as a SQLite literal.
fn literal(field: &str, kind: Kind) -> Result<String, CliError> {
    if field == "\\N" {
        return Ok("NULL".to_owned());
    }
    let text = unescape(field);
    Ok(match kind {
        Kind::Integer | Kind::Real if text.parse::<f64>().is_ok() => text,
        Kind::Boolean => if text == "t" { "1" } else { "0" }.to_owned(),
        Kind::TimestampTz => quote(&utc(&text).unwrap_or(text)),
        Kind::Bytes => match text.strip_prefix("\\x") {
            Some(hex) => format!("X'{hex}'"),
            None => quote(&text),
        },
        Kind::Array => quote(&array_json(&text)),
        _ => quote(&text),
    })
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Undoes COPY's backslash escapes.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// `2026-01-02 03:04:05.123+02` -> `2026-01-02 01:04:05.123` (UTC); `None` if it does not parse.
fn utc(text: &str) -> Option<String> {
    let split = text.rfind(['+', '-']).filter(|&i| i > 10)?;
    let (local, offset) = text.split_at(split);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let mut parts = offset[1..].split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next().map_or(Ok(0), str::parse).ok()?;
    let (date, time) = local.split_once(' ')?;
    let mut ymd = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (ymd.next()?.ok()?, ymd.next()?.ok()?, ymd.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').map_or((time, ""), |(clock, fraction)| (clock, fraction));
    let mut hms = clock.split(':').map(str::parse::<i64>);
    let (h, m, s) = (hms.next()?.ok()?, hms.next()?.ok()?, hms.next()?.ok()?);
    let seconds =
        days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s - sign * (hours * 3600 + minutes * 60);
    let (y, mo, d) = civil_from_days(seconds.div_euclid(86_400));
    let rest = seconds.rem_euclid(86_400);
    let fraction = if fraction.is_empty() { String::new() } else { format!(".{fraction}") };
    Some(format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02}{fraction}", rest / 3600, rest % 3600 / 60, rest % 60))
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `{a,"b c",NULL}` -> `["a","b c",null]` (one dimension; quoted items keep their backslash escapes' characters).
fn array_json(text: &str) -> String {
    let inner = text.trim().trim_start_matches('{').trim_end_matches('}');
    let mut items = Vec::new();
    let mut chars = inner.chars().peekable();
    while chars.peek().is_some() {
        let item = if chars.peek() == Some(&'"') {
            chars.next();
            let mut value = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
            serde_json::Value::String(value)
        } else {
            let value: String = chars.by_ref().take_while(|c| *c != ',').collect();
            if value == "NULL" { serde_json::Value::Null } else { serde_json::Value::String(value) }
        };
        // After a quoted item: its comma.
        if chars.peek() == Some(&',') {
            chars.next();
        }
        items.push(item);
    }
    serde_json::Value::Array(items).to_string()
}

/// `ocre db import-postgres <dump>`: writes the migration and the data file; loads nothing.
pub fn import(project: &Project, dump: &str, name: &str) -> CliResult {
    let text =
        std::fs::read_to_string(project.root.join(dump)).or_else(|_| std::fs::read_to_string(dump)).map_err(|err| {
            CliError::new(format!("cannot read {dump}: {err}")).hint("pass the path of a plain-SQL pg_dump")
        })?;
    let conversion = convert(&text)?;
    let mut edits = Edits::new(project);
    let migration = next_migration_path(&edits, name)?;
    let data = format!("db/{name}.sql");
    let header = format!("-- Converted from {dump} by `ocre db import-postgres`. Check the notes it printed.\n");
    edits.create(&migration, format!("{header}{}", conversion.schema))?;
    edits.create(&data, format!("{header}{}", conversion.data))?;
    let mut report = edits.apply("db import-postgres")?;
    report.ran = conversion.rows.iter().map(|(table, rows)| format!("{table}: {rows} rows")).collect();
    report.ran.extend(conversion.notes.iter().map(|note| format!("note: {note}")));
    report.next = vec![
        "read the notes, and edit the migration if needed".to_owned(),
        "ocre migrate".to_owned(),
        format!("ocre db load {data}"),
        format!("production: ocre migrate --remote, then ocre db load {data} --remote"),
    ];
    Ok(report)
}

#[cfg(test)]
#[path = "../tests/pg_import.rs"]
mod tests;
