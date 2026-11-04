//! Compact integer identifiers.

use core::fmt;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            /// Returns the identifier as an index into a table.
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl From<$name> for usize {
            #[inline]
            fn from(id: $name) -> usize {
                id.index()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

define_id! {
    /// Identifies a net within a problem.
    NetId
}

define_id! {
    /// Identifies a node of an explicit graph, or a cell of a grid.
    NodeId
}

define_id! {
    /// Identifies an edge of an explicit graph.
    EdgeId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips() {
        assert_eq!(NodeId(7).index(), 7);
        assert_eq!(usize::from(NetId(u32::MAX)), u32::MAX as usize);
    }

    #[test]
    fn debug_names_the_kind() {
        assert_eq!(format!("{:?}", EdgeId(3)), "EdgeId(3)");
    }
}
