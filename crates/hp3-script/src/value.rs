//! Script values. Objects store one value per property slot; structs nest their own slots.

use crate::names::Name;

/// Global identity of a package export: lower-case package name and export index.
pub type ObjKey = (std::rc::Rc<str>, usize);

/// Index into the object table: package objects and spawned objects alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId(pub u32);

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
    Object(Option<ObjId>),
    Struct(Box<[Value]>),
    Array(Vec<Value>),
    Delegate(Option<ObjId>, Name),
}

impl Value {
    pub fn as_int(&self) -> i32 {
        match self {
            Value::Int(v) => *v,
            Value::Byte(v) => *v as i32,
            Value::Bool(v) => *v as i32,
            Value::Float(v) => *v as i32,
            _ => 0,
        }
    }

    pub fn as_float(&self) -> f32 {
        match self {
            Value::Float(v) => *v,
            Value::Int(v) => *v as f32,
            Value::Byte(v) => *v as f32,
            _ => 0.0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Byte(v) => *v != 0,
            Value::Object(o) => o.is_some(),
            _ => false,
        }
    }

    pub fn as_byte(&self) -> u8 {
        match self {
            Value::Byte(v) => *v,
            other => other.as_int() as u8,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }

    pub fn as_name(&self) -> Name {
        match self {
            Value::Name(n) => *n,
            _ => Name::NONE,
        }
    }

    pub fn as_obj(&self) -> Option<ObjId> {
        match self {
            Value::Object(o) => *o,
            _ => None,
        }
    }

    /// Members of a struct value (vectors, rotators...).
    pub fn fields(&self) -> &[Value] {
        match self {
            Value::Struct(f) => f,
            _ => &[],
        }
    }
}
