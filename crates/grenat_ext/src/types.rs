//! Rust types and the Grenat types they stand for.

use std::collections::{BTreeMap, HashMap};

use crate::manifest::Struct;

/// A Rust type that crosses to Grenat. Implemented for `String`, integers,
/// floats, `bool`, `()`, `Vec<T>`, `Option<T>`, `Box<T>`, maps from
/// `String`, and derived (`#[derive(GrenatType)]`) for structs.
pub trait GrenatType {
    /// The Grenat type: `Int`, `Array(String)`, `Cell?`, `Hash(String, Float)`.
    fn grenat_type() -> String;

    /// Adds the structs this type mentions (itself included) to `out`, once each.
    fn structs(_out: &mut Vec<Struct>) {}
}

macro_rules! scalar {
    ($grenat:literal: $($rust:ty),*) => {
        $(impl GrenatType for $rust {
            fn grenat_type() -> String {
                $grenat.to_string()
            }
        })*
    };
}

scalar!("String": String);
scalar!("Int": i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
scalar!("Float": f32, f64);
scalar!("Bool": bool);
scalar!("Nil": ());

impl<T: GrenatType> GrenatType for Vec<T> {
    fn grenat_type() -> String {
        format!("Array({})", T::grenat_type())
    }

    fn structs(out: &mut Vec<Struct>) {
        T::structs(out);
    }
}

impl<T: GrenatType> GrenatType for Option<T> {
    fn grenat_type() -> String {
        let inner = T::grenat_type();
        // `nil` is already among the values of `T?`
        if inner.ends_with('?') || inner == "Nil" { inner } else { format!("{inner}?") }
    }

    fn structs(out: &mut Vec<Struct>) {
        T::structs(out);
    }
}

impl<T: GrenatType> GrenatType for Box<T> {
    fn grenat_type() -> String {
        T::grenat_type()
    }

    fn structs(out: &mut Vec<Struct>) {
        T::structs(out);
    }
}

impl<T: GrenatType, S> GrenatType for HashMap<String, T, S> {
    fn grenat_type() -> String {
        format!("Hash(String, {})", T::grenat_type())
    }

    fn structs(out: &mut Vec<Struct>) {
        T::structs(out);
    }
}

impl<T: GrenatType> GrenatType for BTreeMap<String, T> {
    fn grenat_type() -> String {
        format!("Hash(String, {})", T::grenat_type())
    }

    fn structs(out: &mut Vec<Struct>) {
        T::structs(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_types_have_grenat_names() {
        assert_eq!(String::grenat_type(), "String");
        assert_eq!(u8::grenat_type(), "Int");
        assert_eq!(f32::grenat_type(), "Float");
        assert_eq!(<()>::grenat_type(), "Nil");
        assert_eq!(<Vec<Vec<String>>>::grenat_type(), "Array(Array(String))");
        assert_eq!(<Option<i64>>::grenat_type(), "Int?");
        assert_eq!(<Option<Option<i64>>>::grenat_type(), "Int?");
        assert_eq!(<HashMap<String, Vec<f64>>>::grenat_type(), "Hash(String, Array(Float))");
        assert_eq!(<BTreeMap<String, bool>>::grenat_type(), "Hash(String, Bool)");
        assert_eq!(<Box<bool>>::grenat_type(), "Bool");
        let mut structs = Vec::new();
        <Vec<Option<String>>>::structs(&mut structs);
        assert!(structs.is_empty());
    }
}
