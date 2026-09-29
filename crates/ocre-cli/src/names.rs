//! Naming rules shared by generators.

use crate::output::CliError;

/// Names derived from a singular model name such as `BlogPost`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelNames {
    /// `BlogPost`
    pub model: String,
    /// `blog_post` (template loop variable)
    pub singular: String,
    /// `blog_posts` (table, module, URL segment, template directory)
    pub plural: String,
    /// `Blog post`
    pub human_singular: String,
    /// `Blog posts`
    pub human_plural: String,
}

impl ModelNames {
    pub fn parse(input: &str) -> Result<Self, CliError> {
        let words = split_words(input);
        let singular = words.join("_");
        if !is_identifier(&singular) {
            return Err(CliError::new(format!("invalid model name `{input}`"))
                .hint("use a singular name starting with a letter, e.g. `Post` or `BlogPost`"));
        }
        let (last, rest) = words.split_last().expect("identifier has at least one word");
        let plural_words: Vec<String> = rest.iter().cloned().chain([pluralize(last)]).collect();
        let plural = plural_words.join("_");
        Ok(Self {
            model: words.iter().map(|w| capitalize(w)).collect(),
            human_singular: humanize(&singular),
            human_plural: humanize(&plural),
            singular,
            plural,
        })
    }
}

/// Splits `BlogPost`, `blog_post`, `blog-post` or `Blog Post` into lowercase words.
pub fn split_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower_or_digit = false;
    for c in input.chars() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower_or_digit = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower_or_digit && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(c.to_ascii_lowercase());
        prev_lower_or_digit = c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// English plural for the regular cases generators need.
pub fn pluralize(word: &str) -> String {
    const IRREGULAR: &[(&str, &str)] = &[
        ("person", "people"),
        ("child", "children"),
        ("man", "men"),
        ("woman", "women"),
        ("mouse", "mice"),
        ("goose", "geese"),
        ("tooth", "teeth"),
        ("foot", "feet"),
    ];
    if let Some((_, plural)) = IRREGULAR.iter().find(|(singular, _)| *singular == word) {
        return (*plural).to_owned();
    }
    let ends_with_consonant_y = word.ends_with('y')
        && word.len() > 1
        && !matches!(word.as_bytes()[word.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u');
    if ends_with_consonant_y {
        format!("{}ies", &word[..word.len() - 1])
    } else if ["s", "x", "z", "ch", "sh"].iter().any(|suffix| word.ends_with(suffix)) {
        format!("{word}es")
    } else {
        format!("{word}s")
    }
}

/// `snake_case` identifier starting with a letter.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `published_at` -> `Published at`
pub fn humanize(snake: &str) -> String {
    capitalize(&snake.replace('_', " "))
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| first.to_ascii_uppercase().to_string() + chars.as_str())
}

#[cfg(test)]
mod tests;
