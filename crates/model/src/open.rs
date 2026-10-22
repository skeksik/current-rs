//! Open values: strings a server defines, with constants for the values Current knows.
//!
//! A server may send a value no release of Current has seen. It must pass through unchanged (same bytes, same case) so that it can be
//! shown, compared and sent back. Constants name the values Current gives meaning to; anything else is still a valid value.

/// Declares an open value type: a newtype over a string with known values as associated constants.
macro_rules! open_value {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$cmeta:meta])* $konst:ident = $value:literal, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(std::borrow::Cow<'static, str>);

        impl $name {
            $( $(#[$cmeta])* pub const $konst: Self = Self(std::borrow::Cow::Borrowed($value)); )*

            /// Every value this release of Current knows.
            pub const KNOWN: &'static [Self] = &[$(Self::$konst),*];

            /// Wraps a value as received, without changing it.
            pub fn new(value: impl Into<String>) -> Self {
                Self(std::borrow::Cow::Owned(value.into()))
            }

            /// The value exactly as received or declared.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Whether this release of Current gives the value a meaning.
            pub fn is_known(&self) -> bool {
                Self::KNOWN.contains(self)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0.into_owned()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub(crate) use open_value;

#[cfg(test)]
mod tests {
    open_value! {
        /// A test value.
        Sample {
            ONE = "one",
        }
    }

    #[test]
    fn known_values_compare_equal_to_received_ones() {
        assert_eq!(Sample::new("one"), Sample::ONE);
        assert!(Sample::from("one").is_known());
    }

    #[test]
    fn unknown_values_round_trip_unchanged() {
        let received = "Some-Value_é ";
        let value = Sample::from(received);
        assert!(!value.is_known());
        assert_eq!(value.as_str(), received);
        assert_eq!(String::from(value.clone()), received);
        assert_eq!(value.to_string(), received);
    }

    #[test]
    fn values_are_never_normalised() {
        assert_ne!(Sample::new("ONE"), Sample::ONE);
        assert!(!Sample::new("ONE").is_known());
    }
}
