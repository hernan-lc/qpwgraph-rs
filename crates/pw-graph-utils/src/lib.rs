//! Shared utilities and macros for the qpwgraph-rs workspace.

pub mod hex;

/// Generate `as_str`, `parse`, and an `ALL` constant for a string-enum.
///
/// Each variant is mapped to a fixed string. `parse` is case-insensitive and
/// trims input. An optional catch-all string can be provided with
/// `_default` to set which variant `parse` returns for unknown inputs.
///
/// # Example
///
/// ```ignore
/// enum_str! {
///     pub enum Direction {
///         Source = "source",
///         Sink = "sink",
///     }
/// }
///
/// assert_eq!(Direction::Source.as_str(), "source");
/// assert_eq!(Direction::parse("SOURCE"), Direction::Source);
/// ```
#[macro_export]
macro_rules! enum_str {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($variant),+
        }

        impl $name {
            /// All variants of this enum, in declaration order.
            $vis const ALL: &'static [Self] = &[$($name::$variant),+];

            /// The string representation of this variant.
            $vis fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $value),+
                }
            }

            /// Parse a variant from its string representation. Matching is
            /// case-insensitive and surrounding whitespace is trimmed.
            /// Unknown values fall back to the first variant.
            $vis fn parse(value: &str) -> Self {
                match value.trim().to_ascii_lowercase().as_str() {
                    $($value => $name::$variant),+,
                    _ => <$name>::ALL[0],
                }
            }
        }
    };
}

/// Generate a constant constructor that returns `Self` with the given value.
/// Used to keep "both"/"only" constructors on generated enums.
#[macro_export]
macro_rules! enum_default {
    ($name:ident => $variant:ident) => {
        impl Default for $name {
            fn default() -> Self {
                Self::$variant
            }
        }
    };
}
