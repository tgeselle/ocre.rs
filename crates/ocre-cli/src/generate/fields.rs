//! Field language shared by `model`, `scaffold` and `api`:
//! `name:type`, with `?` for optional (NULL allowed) and `^` for unique,
//! e.g. `title:string^ summary:text? author:references avatar:attachment? settings:json`.
//! `author:references:writer_id` names the foreign key column.

use crate::{
    names::{ModelNames, humanize, is_identifier},
    output::CliError,
};

/// Names that would clash with generated columns, Rust keywords or SQL keywords.
pub(super) const RESERVED: &[&str] = &[
    "id",
    "created_at",
    "updated_at",
    "as",
    "async",
    "await",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "yield",
    "and",
    "asc",
    "by",
    "case",
    "check",
    "default",
    "desc",
    "from",
    "group",
    "index",
    "join",
    "key",
    "limit",
    "not",
    "null",
    "offset",
    "or",
    "order",
    "primary",
    "references",
    "select",
    "table",
    "unique",
    "values",
];

pub(super) const TYPES: &str = "string, text, integer (int, small_int, big_int), float (double), decimal, boolean (bool), date, time, datetime (date_time), uuid, references, attachment, json (jsonb), enum:<value>,<value>...";

/// Content types an attachment accepts until the app edits its `Rules`:
/// common images, PDF and plain text, all safe to display inline.
pub(super) const ATTACHMENT_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf", "text/plain"];

/// Columns stored for each attachment, after its name (`avatar_key`, ...).
pub(super) const ATTACHMENT_COLUMNS: [(&str, &str, &str); 4] = [
    ("key", "String", "TEXT"),
    ("filename", "String", "TEXT"),
    ("content_type", "String", "TEXT"),
    ("size", "i64", "INTEGER"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FieldType {
    String,
    Text,
    Integer,
    Float,
    Boolean,
    Date,
    /// `HH:MM[:SS]`, stored as text.
    Time,
    DateTime,
    /// Exact number (money) as text such as `19.99`: a `REAL` would round it.
    Decimal,
    /// Hyphenated UUID text.
    Uuid,
    References,
    /// A file in R2: four columns (`<name>_key`, `_filename`, `_content_type`, `_size`).
    Attachment,
    /// Any JSON value (`serde_json::Value`), stored as its text in a `TEXT`
    /// column checked with `json_valid`.
    Json,
    /// One of a fixed list of values (`status:enum:draft,published`): a Rust
    /// enum in the model, stored as its snake_case text with a `CHECK`.
    Enum,
}

impl FieldType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "text" => Self::Text,
            // SQLite integers are 64-bit whatever the declared size.
            "integer" | "int" | "small_int" | "big_int" => Self::Integer,
            "float" | "double" => Self::Float,
            "decimal" => Self::Decimal,
            "boolean" | "bool" => Self::Boolean,
            "date" => Self::Date,
            "time" => Self::Time,
            "datetime" | "date_time" => Self::DateTime,
            "uuid" => Self::Uuid,
            "references" => Self::References,
            "attachment" => Self::Attachment,
            "json" | "jsonb" => Self::Json,
            "enum" => Self::Enum,
            _ => return None,
        })
    }

    /// Stored as text in SQLite and in Rust.
    pub(super) fn is_textual(self) -> bool {
        matches!(
            self,
            Self::String | Self::Text | Self::Date | Self::Time | Self::DateTime | Self::Decimal | Self::Uuid
        )
    }

    /// Parsed from form text as a number.
    pub(super) fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Float | Self::References)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Field {
    /// Column and Rust field name (`author_id` for `author:references`).
    pub name: String,
    pub ty: FieldType,
    pub optional: bool,
    pub unique: bool,
    /// Referenced model for `references` fields.
    pub target: Option<ModelNames>,
    /// Rust enum of `enum` fields: `Status` with values `draft`, `published`.
    pub enumeration: Option<Enumeration>,
}

/// The Rust enum generated for `status:enum:draft,published`, stored as TEXT.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Enumeration {
    /// `Status` for the field `status`.
    pub type_name: String,
    /// The stored values, in declaration order: `draft`, `published`.
    pub values: Vec<String>,
}

impl Enumeration {
    fn parse(name: &str, values: &str) -> Result<Self, CliError> {
        let values: Vec<String> = values.split(',').map(str::to_owned).collect();
        let valid = values.iter().all(|value| is_identifier(value))
            && values.iter().enumerate().all(|(i, v)| !values[..i].contains(v));
        if !valid {
            return Err(CliError::new(format!("invalid values `{}` for enum `{name}`", values.join(",")))
                .hint("list distinct snake_case values after the type, e.g. `status:enum:draft,published`"));
        }
        Ok(Self { type_name: pascal_case(name), values })
    }

    /// `Draft` for `draft`: the Rust variant of a value.
    pub(super) fn variant(value: &str) -> String {
        pascal_case(value)
    }
}

/// `published_at` -> `PublishedAt`.
fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| first.to_ascii_uppercase().to_string() + chars.as_str())
        })
        .collect()
}

impl Field {
    pub(super) fn parse(spec: &str) -> Result<Self, CliError> {
        let (name, ty) = spec.split_once(':').ok_or_else(|| {
            CliError::new(format!("field `{spec}` has no type"))
                .hint("write fields as `name:type`, e.g. `title:string`")
        })?;
        // `?` and `^` go after the type or after its argument
        // (`author:references?:writer_id` or `author:references:writer_id?`).
        let (ty_part, argument) = match ty.split_once(':') {
            Some((ty_part, argument)) => (ty_part, Some(argument)),
            None => (ty, None),
        };
        let strip = |part: &str| part.trim_end_matches(['?', '^']).len();
        let suffix: String = [ty_part, argument.unwrap_or_default()].iter().map(|part| &part[strip(part)..]).collect();
        let ty_name = &ty_part[..strip(ty_part)];
        let argument = argument.map(|argument| &argument[..strip(argument)]);
        let (optional, unique) = (suffix.contains('?'), suffix.contains('^'));
        if !is_identifier(name) {
            return Err(CliError::new(format!("invalid field name `{name}`"))
                .hint("use snake_case starting with a letter, e.g. `published_at`"));
        }
        if RESERVED.contains(&name) {
            return Err(CliError::new(format!("field name `{name}` is reserved")).hint(
                "`id`, `created_at` and `updated_at` are generated; Rust and SQL keywords are not allowed. Pick another name, e.g. `kind` for `type`",
            ));
        }
        let ty = FieldType::parse(ty_name).ok_or_else(|| {
            CliError::new(format!("unknown field type `{ty_name}` for `{name}`"))
                .hint(format!("types: {TYPES}; add `?` for optional, `^` for unique"))
        })?;
        if ty == FieldType::Boolean && optional {
            return Err(CliError::new(format!("boolean field `{name}` cannot be optional"))
                .hint("booleans are true or false (a checkbox); drop the `?`"));
        }
        if ty == FieldType::Attachment && unique {
            return Err(CliError::new(format!("attachment `{name}` cannot be unique"))
                .hint("every stored file gets its own random key already; drop the `^`"));
        }
        if ty == FieldType::Json && unique {
            return Err(CliError::new(format!("json field `{name}` cannot be unique"))
                .hint("a unique index compares JSON text, where key order and spacing differ; drop the `^`"));
        }
        if ty == FieldType::Attachment && ["edit", "delete", "new"].contains(&name) {
            return Err(CliError::new(format!("attachment name `{name}` clashes with a scaffold route"))
                .hint(format!("`/<plural>/{{id}}/{name}` is taken; pick another name, e.g. `{name}_file`")));
        }
        if ty == FieldType::Enum && unique {
            return Err(CliError::new(format!("enum `{name}` cannot be unique"))
                .hint("a few values cannot be unique across many rows; drop the `^`"));
        }
        let (name, target, enumeration) = match (ty, argument) {
            (FieldType::References, None) => (format!("{name}_id"), Some(ModelNames::parse(name)?), None),
            (FieldType::References, Some(column)) => {
                if !is_identifier(column) || !column.ends_with("_id") || RESERVED.contains(&column) {
                    return Err(CliError::new(format!("invalid foreign key column `{column}` for `{name}`"))
                        .hint("name the column in snake_case ending in `_id`, e.g. `author:references:writer_id`"));
                }
                (column.to_owned(), Some(ModelNames::parse(name)?), None)
            }
            (FieldType::Enum, Some(values)) => (name.to_owned(), None, Some(Enumeration::parse(name, values)?)),
            (FieldType::Enum, None) => {
                return Err(CliError::new(format!("enum `{name}` has no values"))
                    .hint(format!("list them after the type, e.g. `{name}:enum:draft,published`")));
            }
            (_, Some(argument)) => {
                return Err(CliError::new(format!("type `{ty_name}` of `{name}` takes no `:{argument}`")).hint(
                    "only `references` (the foreign key column, e.g. `author:references:writer_id`) and `enum` \
                     (its values, e.g. `status:enum:draft,published`) take an argument",
                ));
            }
            (_, None) => (name.to_owned(), None, None),
        };
        Ok(Self { name, ty, optional, unique, target, enumeration })
    }

    /// `Published at`; `Author` for `author_id`.
    pub(super) fn label(&self) -> String {
        humanize(self.name.strip_suffix("_id").filter(|_| self.target.is_some()).unwrap_or(&self.name))
    }

    /// askama expression printing this field of `record` (`{{ post.title }}`);
    /// optional values print nothing when empty, attachments their file name.
    pub(super) fn display(&self, record: &str) -> String {
        let name = &self.name;
        match (self.is_attachment(), self.optional) {
            (true, true) => {
                format!("{{% if let Some(file) = {record}.{name}() %}}{{{{ file.filename }}}}{{% endif %}}")
            }
            (true, false) => format!("{{{{ {record}.{name}_filename }}}}"),
            (false, true) => format!("{{% if let Some(value) = {record}.{name} %}}{{{{ value }}}}{{% endif %}}"),
            (false, false) => format!("{{{{ {record}.{name} }}}}"),
        }
    }

    /// Rust type of the value, without `Option`. For attachments, the file
    /// received before it is stored (`ocre::storage::Upload`).
    pub(super) fn rust_type(&self) -> &str {
        match self.ty {
            FieldType::String
            | FieldType::Text
            | FieldType::Date
            | FieldType::Time
            | FieldType::DateTime
            | FieldType::Decimal
            | FieldType::Uuid => "String",
            FieldType::Integer | FieldType::References => "i64",
            FieldType::Float => "f64",
            FieldType::Boolean => "bool",
            FieldType::Attachment => "Upload",
            FieldType::Json => "ocre::serde_json::Value",
            FieldType::Enum => &self.enumeration.as_ref().expect("enum fields have values").type_name,
        }
    }

    /// Rust type of the column: `Option<T>` when optional.
    pub(super) fn column_type(&self) -> String {
        if self.optional { format!("Option<{}>", self.rust_type()) } else { self.rust_type().to_owned() }
    }

    pub(super) fn is_attachment(&self) -> bool {
        self.ty == FieldType::Attachment
    }

    /// `AVATAR`: the `ocre::storage::Rules` constant of an attachment.
    pub(super) fn rules_const(&self) -> String {
        self.name.to_ascii_uppercase()
    }

    /// Table columns with their Rust types: one per field, four per
    /// attachment (`avatar_key`, `avatar_filename`, `avatar_content_type`, `avatar_size`).
    pub(super) fn columns(&self) -> Vec<(String, String)> {
        if !self.is_attachment() {
            return vec![(self.name.clone(), self.column_type())];
        }
        ATTACHMENT_COLUMNS
            .iter()
            .map(|(suffix, ty, _)| {
                let ty = if self.optional { format!("Option<{ty}>") } else { (*ty).to_owned() };
                (format!("{}_{suffix}", self.name), ty)
            })
            .collect()
    }

    /// Column definitions inside `CREATE TABLE` (or `ADD COLUMN`).
    pub(super) fn sql_columns(&self) -> Vec<String> {
        let null = if self.optional { "" } else { " NOT NULL" };
        if self.is_attachment() {
            return ATTACHMENT_COLUMNS
                .iter()
                .map(|(suffix, _, sql)| format!("{}_{suffix} {sql}{null}", self.name))
                .collect();
        }
        let sql_type = match self.ty {
            FieldType::Integer | FieldType::Boolean | FieldType::References => "INTEGER",
            FieldType::Float => "REAL",
            _ => "TEXT",
        };
        let default = if self.ty == FieldType::Boolean { " DEFAULT 0" } else { "" };
        // Required references are deleted with their parent (Rails' `dependent: :destroy`
        // done by SQLite); optional ones are set to NULL (`dependent: :nullify`).
        let reference = match &self.target {
            Some(target) if self.optional => format!(" REFERENCES {}(id) ON DELETE SET NULL", target.plural),
            Some(target) => format!(" REFERENCES {}(id) ON DELETE CASCADE", target.plural),
            None => String::new(),
        };
        let check = match (self.ty, &self.enumeration) {
            (FieldType::Json, _) => format!(" CHECK (json_valid({}))", self.name),
            (_, Some(enumeration)) => {
                let values = enumeration.values.iter().map(|v| format!("'{v}'")).collect::<Vec<_>>().join(", ");
                format!(" CHECK ({} IN ({values}))", self.name)
            }
            _ => String::new(),
        };
        vec![format!("{} {sql_type}{null}{default}{reference}{check}", self.name)]
    }

    /// Checks on a value of the Rust type, as `v.<check>` statements for the
    /// given expression (`self.title`, or a variable bound from an `Option`).
    pub(super) fn checks(&self, value: &str) -> Vec<String> {
        let name = &self.name;
        match self.ty {
            FieldType::String | FieldType::Text if !self.optional => vec![format!("v.required(\"{name}\", {value});")],
            FieldType::Date => vec![format!("v.date(\"{name}\", {value});")],
            FieldType::DateTime => vec![format!("v.datetime(\"{name}\", {value});")],
            FieldType::Time => vec![format!("v.time(\"{name}\", {value});")],
            FieldType::Decimal => vec![format!("v.decimal(\"{name}\", {value});")],
            FieldType::Uuid => vec![format!("v.uuid(\"{name}\", {value});")],
            FieldType::Integer => vec![format!("v.safe_integer(\"{name}\", {value});")],
            FieldType::Attachment => vec![format!("v.file(\"{name}\", {value}, &{});", self.rules_const())],
            _ => vec![],
        }
    }
}

pub(super) fn parse_fields(specs: &[String]) -> Result<Vec<Field>, CliError> {
    let fields = specs.iter().map(|spec| Field::parse(spec)).collect::<Result<Vec<_>, _>>()?;
    let mut columns: Vec<String> = Vec::new();
    for field in &fields {
        for (column, _) in field.columns() {
            if columns.contains(&column) {
                return Err(CliError::new(format!("field `{column}` is listed twice")).hint(
                    "names must differ, and `<name>:attachment` also takes `<name>_key`, `<name>_filename`, `<name>_content_type` and `<name>_size`",
                ));
            }
            columns.push(column);
        }
    }
    Ok(fields)
}

#[cfg(test)]
#[path = "../../tests/generate/fields.rs"]
mod tests;
