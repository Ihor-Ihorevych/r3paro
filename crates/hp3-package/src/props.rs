//! Tagged properties: the self-describing name/type/size records objects store their script properties in.

use crate::package::Package;
use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagType {
    Byte,
    Int,
    Bool,
    Float,
    Object,
    Name,
    String,
    Class,
    Array,
    Struct,
    Vector,
    Rotator,
    Str,
    Map,
    FixedArray,
}

impl TagType {
    fn from_u8(v: u8) -> Option<TagType> {
        Some(match v {
            1 => TagType::Byte,
            2 => TagType::Int,
            3 => TagType::Bool,
            4 => TagType::Float,
            5 => TagType::Object,
            6 => TagType::Name,
            7 => TagType::String,
            8 => TagType::Class,
            9 => TagType::Array,
            10 => TagType::Struct,
            11 => TagType::Vector,
            12 => TagType::Rotator,
            13 => TagType::Str,
            14 => TagType::Map,
            15 => TagType::FixedArray,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Tag<'a> {
    pub name: usize,
    pub ty: TagType,
    /// Struct type name for struct tags.
    pub struct_name: Option<usize>,
    pub array_index: u32,
    /// Value of a bool tag, which carries no data.
    pub bool_value: bool,
    pub data: &'a [u8],
}

impl Tag<'_> {
    pub fn as_u8(&self) -> Option<u8> {
        self.data.first().copied()
    }

    pub fn as_i32(&self) -> Option<i32> {
        Some(i32::from_le_bytes(self.data.get(..4)?.try_into().ok()?))
    }

    pub fn as_f32(&self) -> Option<f32> {
        Some(f32::from_bits(self.as_i32()? as u32))
    }

    /// Object or class reference.
    pub fn as_obj(&self) -> Option<crate::ObjRef> {
        Reader::new(self.data).ci().ok().map(crate::ObjRef)
    }

    /// Name index.
    pub fn as_name(&self) -> Option<usize> {
        Reader::new(self.data).ci().ok().and_then(|v| usize::try_from(v).ok())
    }

    pub fn as_str(&self) -> Option<String> {
        Reader::new(self.data).fstring().ok()
    }
}

/// Finds the first tag called `name` (array index 0 unless given).
pub fn find<'t, 'a>(pkg: &Package, tags: &'t [Tag<'a>], name: &str, index: u32) -> Option<&'t Tag<'a>> {
    tags.iter().find(|t| t.array_index == index && pkg.name(t.name).eq_ignore_ascii_case(name))
}

/// Reads one tag; `None` at the terminating `None` name.
pub fn read_tag<'a>(pkg: &Package, r: &mut Reader<'a>) -> Result<Option<Tag<'a>>> {
    let start = r.file_pos();
    let name = r.ci()?;
    if name < 0 || name as usize >= pkg.names.len() {
        return Err(Error::Format(format!("bad tag name index {name} at {start:#x}")));
    }
    let name = name as usize;
    if pkg.name(name).eq_ignore_ascii_case("None") {
        return Ok(None);
    }
    let info = r.u8()?;
    let ty = TagType::from_u8(info & 0x0F)
        .ok_or_else(|| Error::Format(format!("bad tag type {} for {} at {start:#x}", info & 0x0F, pkg.name(name))))?;
    let array_bit = info & 0x80 != 0;
    let struct_name = if ty == TagType::Struct {
        let n = r.ci()?;
        if n < 0 || n as usize >= pkg.names.len() {
            return Err(Error::Format(format!("bad struct name index {n} at {start:#x}")));
        }
        Some(n as usize)
    } else {
        None
    };
    let size = match (info >> 4) & 0x07 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 12,
        4 => 16,
        5 => r.u8()? as usize,
        6 => r.u16()? as usize,
        _ => r.u32()? as usize,
    };
    let mut array_index = 0;
    if array_bit && ty != TagType::Bool {
        let b = r.u8()?;
        array_index = if b & 0x80 == 0 {
            b as u32
        } else if b & 0xC0 == 0x80 {
            (((b & 0x7F) as u32) << 8) | r.u8()? as u32
        } else {
            let rest = r.bytes(3)?;
            (((b & 0x3F) as u32) << 24) | ((rest[0] as u32) << 16) | ((rest[1] as u32) << 8) | rest[2] as u32
        };
    }
    let (bool_value, data) = if ty == TagType::Bool { (array_bit, &[][..]) } else { (false, r.bytes(size)?) };
    Ok(Some(Tag { name, ty, struct_name, array_index, bool_value, data }))
}

/// Reads tags up to and including the terminating `None`.
pub fn read_tags<'a>(pkg: &Package, r: &mut Reader<'a>) -> Result<Vec<Tag<'a>>> {
    let mut tags = Vec::new();
    while let Some(tag) = read_tag(pkg, r)? {
        tags.push(tag);
        if tags.len() > 100_000 {
            return Err(Error::Format("runaway property list".into()));
        }
    }
    Ok(tags)
}
