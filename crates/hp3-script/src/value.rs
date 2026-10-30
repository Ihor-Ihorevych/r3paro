//! Script values. Objects store one value per property slot; structs nest their own slots.

use crate::names::Name;

/// Global identity of a package export: lower-case package name and export index.
pub type ObjKey = (std::rc::Rc<str>, usize);

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Byte(u8),
    Int(i32),
    Bool(bool),
    Float(f32),
    Name(Name),
    Str(String),
    Object(Option<ObjKey>),
    Struct(Box<[Value]>),
    Array(Vec<Value>),
    Delegate(Option<ObjKey>, Name),
}
