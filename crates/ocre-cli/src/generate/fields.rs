//! Field language shared by `model`, `scaffold` and `api`:
//! `name:type`, with `?` for optional (NULL allowed) and `^` for unique,
//! e.g. `title:string^ summary:text? author:references`.

use crate::{
    names::{ModelNames, humanize, is_identifier},
    output::CliError,
};

/// Names that would clash with generated columns, Rust keywords or SQL keywords.
const RESERVED: &[&str] = &[
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

pub(super) const TYPES: &str = "string, text, integer, float, boolean, date, datetime, references";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FieldType {
    String,
    Text,
    Integer,
    Float,
    Boolean,
    Date,
    DateTime,
    References,
}

impl FieldType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "text" => Self::Text,
            "integer" => Self::Integer,
            "float" => Self::Float,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "datetime" => Self::DateTime,
            "references" => Self::References,
            _ => return None,
        })
    }

    /// Stored as text in SQLite and in Rust.
    pub(super) fn is_textual(self) -> bool {
        matches!(self, Self::String | Self::Text | Self::Date | Self::DateTime)
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
}

impl Field {
    pub(super) fn parse(spec: &str) -> Result<Self, CliError> {
        let (name, ty) = spec.split_once(':').ok_or_else(|| {
            CliError::new(format!("field `{spec}` has no type"))
                .hint("write fields as `name:type`, e.g. `title:string`")
        })?;
        let modifiers = ty.len() - ty.trim_end_matches(['?', '^']).len();
        let (ty_name, suffix) = ty.split_at(ty.len() - modifiers);
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
        let (name, target) = if ty == FieldType::References {
            (format!("{name}_id"), Some(ModelNames::parse(name)?))
        } else {
            (name.to_owned(), None)
        };
        Ok(Self { name, ty, optional, unique, target })
    }

    /// `Published at`; `Author` for `author_id`.
    pub(super) fn label(&self) -> String {
        humanize(self.name.strip_suffix("_id").filter(|_| self.target.is_some()).unwrap_or(&self.name))
    }

    /// Rust type of the value, without `Option`.
    pub(super) fn rust_type(&self) -> &'static str {
        match self.ty {
            FieldType::String | FieldType::Text | FieldType::Date | FieldType::DateTime => "String",
            FieldType::Integer | FieldType::References => "i64",
            FieldType::Float => "f64",
            FieldType::Boolean => "bool",
        }
    }

    /// Rust type of the column: `Option<T>` when optional.
    pub(super) fn column_type(&self) -> String {
        if self.optional { format!("Option<{}>", self.rust_type()) } else { self.rust_type().to_owned() }
    }

    /// Column definition inside `CREATE TABLE`.
    pub(super) fn sql_column(&self) -> String {
        let sql_type = match self.ty {
            FieldType::String | FieldType::Text | FieldType::Date | FieldType::DateTime => "TEXT",
            FieldType::Integer | FieldType::Boolean | FieldType::References => "INTEGER",
            FieldType::Float => "REAL",
        };
        let null = if self.optional { "" } else { " NOT NULL" };
        let default = if self.ty == FieldType::Boolean { " DEFAULT 0" } else { "" };
        let reference = match &self.target {
            Some(target) => format!(" REFERENCES {}(id) ON DELETE CASCADE", target.plural),
            None => String::new(),
        };
        format!("{} {sql_type}{null}{default}{reference}", self.name)
    }

    /// Checks on a value of the Rust type, as `v.<check>` statements for the
    /// given expression (`self.title`, or a variable bound from an `Option`).
    pub(super) fn checks(&self, value: &str) -> Vec<String> {
        let name = &self.name;
        match self.ty {
            FieldType::String | FieldType::Text if !self.optional => vec![format!("v.required(\"{name}\", {value});")],
            FieldType::Date => vec![format!("v.date(\"{name}\", {value});")],
            FieldType::DateTime => vec![format!("v.datetime(\"{name}\", {value});")],
            FieldType::Integer => vec![format!("v.safe_integer(\"{name}\", {value});")],
            _ => vec![],
        }
    }
}

pub(super) fn parse_fields(specs: &[String]) -> Result<Vec<Field>, CliError> {
    let fields = specs.iter().map(|spec| Field::parse(spec)).collect::<Result<Vec<_>, _>>()?;
    for (i, field) in fields.iter().enumerate() {
        if fields[..i].iter().any(|f| f.name == field.name) {
            return Err(CliError::new(format!("field `{}` is listed twice", field.name)));
        }
    }
    Ok(fields)
}

#[cfg(test)]
mod tests;
