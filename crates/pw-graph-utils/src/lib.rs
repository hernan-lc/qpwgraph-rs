//! Shared utilities and macros for the qpwgraph-rs workspace.

pub mod fs;
pub mod hex;

pub use fs::atomic_write;

/// Generate `as_str`, `parse`, and an `ALL` constant for a string-enum.
///
/// Each variant is mapped to a fixed string. `parse` is case-insensitive and
/// trims input. The macro automatically derives `Clone, Copy, Debug,
/// PartialEq, Eq`. Any additional derives or attributes can be passed at the
/// top level (e.g. `#[serde(rename_all = "lowercase")]`).
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
///
/// Attributes may also be attached to individual variants, which is how an
/// enum declares its default:
///
/// ```ignore
/// enum_str! {
///     #[derive(Default)]
///     pub enum Direction {
///         #[default]
///         Source = "source",
///         Sink = "sink",
///     }
/// }
///
/// assert_eq!(Direction::default(), Direction::Source);
/// ```
///
/// Prefer that over a hand-written `impl Default`, which is what
/// `clippy::derivable_impls` asks for.
#[macro_export]
macro_rules! enum_str {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant),+
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
