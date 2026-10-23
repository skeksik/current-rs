//! The list-diff shape every exported list uses (ADR 0004). UniFFI enums cannot be generic, so [`list_diff!`] declares one diff enum
//! per item type, converting from `current_app::VectorDiff`.

/// An index as the bindings carry it. A UI list never holds four billion rows; a larger index saturates rather than wrapping.
pub(crate) fn ffi_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Declares `pub enum $name` with the variants `Append`, `Insert`, `Set`, `Remove` and `Reset` over `$item`, and
/// `$name::from_core`, which converts a `VectorDiff<T>` for any `T` that `$item` converts from. Indices refer to the list after the
/// diffs before it in the same batch; UIs apply a batch in order.
macro_rules! list_diff {
    ($(#[$meta:meta])* $name:ident, $item:ty) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, uniffi::Enum)]
        pub enum $name {
            Append { values: Vec<$item> },
            Insert { index: u32, value: $item },
            Set { index: u32, value: $item },
            Remove { index: u32 },
            Reset { values: Vec<$item> },
        }

        impl $name {
            pub(crate) fn from_core<T>(diff: current_app::VectorDiff<T>) -> Self
            where
                $item: From<T>,
            {
                use current_app::VectorDiff;
                use $crate::diff::ffi_index;
                match diff {
                    VectorDiff::Append { values } => Self::Append {
                        values: values.into_iter().map(<$item>::from).collect(),
                    },
                    VectorDiff::Insert { index, value } => Self::Insert {
                        index: ffi_index(index),
                        value: value.into(),
                    },
                    VectorDiff::Set { index, value } => Self::Set {
                        index: ffi_index(index),
                        value: value.into(),
                    },
                    VectorDiff::Remove { index } => Self::Remove {
                        index: ffi_index(index),
                    },
                    VectorDiff::Reset { values } => Self::Reset {
                        values: values.into_iter().map(<$item>::from).collect(),
                    },
                }
            }
        }
    };
}

pub(crate) use list_diff;
