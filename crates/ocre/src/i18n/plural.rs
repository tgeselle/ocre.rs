//! CLDR plural categories for whole numbers, for the languages below. Other
//! languages use the English rule (`one` for 1, `other` otherwise). Source:
//! <https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html>.

/// A CLDR plural category, the last key segment of a plural translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl Category {
    pub(crate) const ALL: [Category; 6] =
        [Category::Zero, Category::One, Category::Two, Category::Few, Category::Many, Category::Other];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.name() == name)
    }
}

/// How a language picks a category for a whole number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    /// Japanese, Chinese, Korean...: `other` only.
    Other,
    /// English, German, Spanish, Italian, Dutch...: `one` for 1.
    One,
    /// French, Portuguese, Hindi: `one` for 0 and 1.
    ZeroOne,
    /// Russian, Ukrainian, Belarusian: `one` (1, 21...), `few` (2-4, 22-24...), `many`.
    EastSlavic,
    /// Polish: `one` for 1, `few` (2-4, 22-24...), `many`.
    Polish,
    /// Czech, Slovak: `one` for 1, `few` for 2-4.
    CzechSlovak,
    /// Arabic: `zero`, `one`, `two`, `few` (3-10), `many` (11-99).
    Arabic,
    /// Hebrew: `one`, `two`.
    Hebrew,
}

impl Rule {
    /// The rule of a locale code (`pt-BR` uses Portuguese's).
    pub(crate) fn for_locale(code: &str) -> Self {
        let language = code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase();
        match language.as_str() {
            "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" | "lo" | "my" => Self::Other,
            "fr" | "pt" | "hi" | "fa" | "bn" => Self::ZeroOne,
            "ru" | "uk" | "be" => Self::EastSlavic,
            "pl" => Self::Polish,
            "cs" | "sk" => Self::CzechSlovak,
            "ar" => Self::Arabic,
            "he" | "iw" => Self::Hebrew,
            _ => Self::One,
        }
    }

    /// The categories a complete translation of a plural key needs.
    pub(crate) fn categories(self) -> &'static [Category] {
        use Category::*;
        match self {
            Self::Other => &[Other],
            Self::One | Self::ZeroOne => &[One, Other],
            Self::EastSlavic | Self::Polish => &[One, Few, Many, Other],
            Self::CzechSlovak => &[One, Few, Other],
            Self::Arabic => &[Zero, One, Two, Few, Many, Other],
            Self::Hebrew => &[One, Two, Other],
        }
    }

    /// The category of `n` (negative numbers use their absolute value).
    pub(crate) fn category(self, n: i64) -> Category {
        let n = n.unsigned_abs();
        let (mod10, mod100) = (n % 10, n % 100);
        let few_slavic = (2..=4).contains(&mod10) && !(12..=14).contains(&mod100);
        match self {
            Self::Other => Category::Other,
            Self::One | Self::CzechSlovak | Self::Polish | Self::Hebrew if n == 1 => Category::One,
            Self::One => Category::Other,
            Self::ZeroOne if n <= 1 => Category::One,
            Self::ZeroOne => Category::Other,
            Self::EastSlavic if mod10 == 1 && mod100 != 11 => Category::One,
            Self::EastSlavic | Self::Polish if few_slavic => Category::Few,
            Self::EastSlavic | Self::Polish => Category::Many,
            Self::CzechSlovak if (2..=4).contains(&n) => Category::Few,
            Self::CzechSlovak => Category::Other,
            Self::Hebrew if n == 2 => Category::Two,
            Self::Hebrew => Category::Other,
            Self::Arabic => match (n, mod100) {
                (0, _) => Category::Zero,
                (1, _) => Category::One,
                (2, _) => Category::Two,
                (_, 3..=10) => Category::Few,
                (_, 11..=99) => Category::Many,
                _ => Category::Other,
            },
        }
    }
}

#[cfg(test)]
#[path = "../../tests/i18n/plural.rs"]
mod tests;
