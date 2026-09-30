//! Model and attribute names and validation messages in the locale of an
//! [`I18n`]: Rails' `model_name.human`, `human_attribute_name` and
//! translated `errors.full_messages`.

use super::{Count, I18n, builtin, interpolate};
use crate::{FieldError, names::humanize};

impl I18n {
    /// The name of a model for `count` items, from `models.<model>` (Rails' `Post.model_name.human(count:)`).
    ///
    /// `model` is the snake_case name (`blog_post`). `models.<model>` is a
    /// text or plural forms (`one`/`other`...). Without a translation, the
    /// humanized name (`Blog post`) for every count: add an `other` form for
    /// plurals, since Ocre has no runtime inflector.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("fr", "fr:\n  models:\n    post:\n      one: Article\n      other: Articles\n")])
    /// });
    /// let fr = LOCALES.locale("fr");
    /// assert_eq!(fr.model_name("post", 1), "Article");
    /// assert_eq!(fr.model_name("post", 3), "Articles");
    /// assert_eq!(fr.model_name("blog_post", 1), "Blog post");
    /// ```
    pub fn model_name(&self, model: &str, count: impl Count) -> String {
        let key = format!("models.{model}");
        self.lookup(&[&key], Some(count.to_count()), false).map_or_else(|| humanize(model), str::to_owned)
    }

    /// The name of a model's field, from `attributes.<model>.<field>`, then `attributes.<field>` (Rails' `human_attribute_name`).
    ///
    /// Without a translation, the humanized field (`published_at` gives
    /// `Published at`, `author_id` gives `Author`), as
    /// [`FieldError::full_message`] writes it. Use it for form labels and
    /// table headers.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[(
    ///         "fr",
    ///         "fr:\n  attributes:\n    created_at: Créé le\n    post:\n      title: Titre\n",
    ///     )])
    /// });
    /// let fr = LOCALES.locale("fr");
    /// assert_eq!(fr.attribute("post", "title"), "Titre");
    /// assert_eq!(fr.attribute("post", "created_at"), "Créé le");
    /// assert_eq!(fr.attribute("post", "author_id"), "Author");
    /// ```
    pub fn attribute(&self, model: &str, field: &str) -> String {
        let keys = [format!("attributes.{model}.{field}"), format!("attributes.{field}")];
        self.lookup(&[&keys[0], &keys[1]], None, false).map_or_else(|| humanize(field), str::to_owned)
    }

    /// The message of a validation error in the locale, without the field name (Rails' `errors.messages`).
    ///
    /// Each [`Validator`](crate::Validator) check records Rails' key
    /// ([`FieldError::key`]: `blank`, `too_long`, `inclusion`...), looked up
    /// in this order, the first found winning:
    /// `errors.models.<model>.attributes.<field>.<key>`,
    /// `errors.models.<model>.<key>`, `errors.attributes.<field>.<key>`,
    /// `errors.messages.<key>`; first in the locale's file, then in the
    /// built-in translations of its language (English, French, German,
    /// Spanish, Italian, Portuguese, Dutch), then in the default locale.
    /// `%{count}` is the check's bound, `%{attribute}` the translated field
    /// (the confirmed one for `confirmation`) and `%{model}` the model name.
    /// An error without a key (a custom [`check`](crate::Validator::check)
    /// or [`message`](crate::Validator::message)) keeps its message, unless
    /// that message is one of the English built-in ones (`has already been
    /// taken` is `taken`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// use ocre::{Error, Validator};
    ///
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[
    ///         ("en", "en:\n"),
    ///         ("fr", "fr:\n  errors:\n    models:\n      post:\n        attributes:\n          title:\n            blank: \"donnez un titre\"\n"),
    ///     ])
    /// });
    /// let mut v = Validator::new();
    /// v.required("title", "").required("body", "").max_length("slug", "far-too-long", 5);
    /// let Err(Error::Invalid(errors)) = v.finish() else { unreachable!() };
    /// let fr = LOCALES.locale("fr");
    /// let messages: Vec<String> = errors.iter().map(|error| fr.error_message("post", error)).collect();
    /// assert_eq!(messages, ["donnez un titre", "doit être rempli(e)", "est trop long (pas plus de 5 caractères)"]);
    /// ```
    pub fn error_message(&self, model: &str, error: &FieldError) -> String {
        let Some(key) = error.key().or_else(|| builtin::english_key(&error.message)) else {
            return error.message.clone();
        };
        let field = error.field.as_str();
        let keys = [
            format!("errors.models.{model}.attributes.{field}.{key}"),
            format!("errors.models.{model}.{key}"),
            format!("errors.attributes.{field}.{key}"),
            format!("errors.messages.{key}"),
        ];
        let keys = [keys[0].as_str(), keys[1].as_str(), keys[2].as_str(), keys[3].as_str()];
        let detail = error.detail().unwrap_or_default();
        let (attribute, count) = match key {
            "confirmation" => (self.attribute(model, detail), None),
            _ => (self.attribute(model, field), detail.parse::<i64>().ok()),
        };
        let text: &str = self.lookup(&keys, count, false).unwrap_or(&error.message);
        let args = [("attribute", attribute), ("model", self.model_name(model, 1)), ("count", detail.to_owned())];
        let mut out = String::new();
        interpolate(&mut out, text, &args, count, false).expect("writing to a String");
        out
    }

    /// The translated field name and message of a validation error, as `errors.format` lays them out (Rails' `full_message`).
    ///
    /// `errors.format` defaults to `%{attribute} %{message}`; the attribute
    /// is [`attribute`](Self::attribute) and the message
    /// [`error_message`](Self::error_message).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("de", "de:\n  attributes:\n    post:\n      title: Titel\n")])
    /// });
    /// let mut v = ocre::Validator::new();
    /// v.required("title", "");
    /// let de = LOCALES.locale("de");
    /// assert_eq!(de.full_message("post", &v.errors()[0]), "Titel muss ausgefüllt werden");
    /// ```
    pub fn full_message(&self, model: &str, error: &FieldError) -> String {
        let layout = self.lookup(&["errors.format"], None, false).unwrap_or("%{attribute} %{message}");
        let args = [("attribute", self.attribute(model, &error.field)), ("message", self.error_message(model, error))];
        let mut out = String::new();
        interpolate(&mut out, layout, &args, None, false).expect("writing to a String");
        out
    }
}

#[cfg(test)]
#[path = "../../tests/i18n/model.rs"]
mod tests;
