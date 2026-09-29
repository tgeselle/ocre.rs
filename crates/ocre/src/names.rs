//! Naming helpers shared by messages.

/// `published_at` -> `Published at`; like Rails, a foreign key names its
/// association: `author_id` -> `Author`.
pub(crate) fn humanize(snake: &str) -> String {
    let text = snake.strip_suffix("_id").filter(|rest| !rest.is_empty()).unwrap_or(snake).replace('_', " ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().collect::<String>() + chars.as_str())
}

#[cfg(test)]
mod tests;
