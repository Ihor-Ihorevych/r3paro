//! Deserialization of exports: the common object header, tagged properties, and the native data of the classes
//! whose layout is known.

use crate::bytecode::{self, Token};
use crate::package::{ObjRef, Package};
use crate::props::{self, Tag};
use crate::reader::Reader;
use crate::{Error, Result};

/// Export flag: the object carries a script state frame.
pub const RF_HAS_STACK: u32 = 0x0200_0000;
/// Property flag: replicated, followed by a replication offset.
pub const CPF_NET: u32 = 0x0000_0020;
/// Function flag: replicated, followed by a replication offset.
pub const FUNC_NET: u32 = 0x0000_0040;

#[derive(Clone, Debug)]
pub struct StateFrame {
    pub node: ObjRef,
    pub state_node: ObjRef,
    pub probe_mask: u64,
    pub latent_action: u32,
    pub offset: Option<i32>,
}

pub fn read_state_frame(r: &mut Reader<'_>) -> Result<StateFrame> {
    let node = ObjRef(r.ci()?);
    let state_node = ObjRef(r.ci()?);
    let probe_mask = r.u64()?;
    let latent_action = r.u32()?;
    let offset = if node.is_none() { None } else { Some(r.ci()?) };
    Ok(StateFrame { node, state_node, probe_mask, latent_action, offset })
}

#[derive(Clone, Debug)]
pub struct FieldData {
    pub super_field: ObjRef,
    pub next: ObjRef,
}

#[derive(Clone, Debug)]
pub struct StructData {
    pub field: FieldData,
    pub script_text: ObjRef,
    pub children: ObjRef,
    pub friendly_name: usize,
    pub cpp_text: ObjRef,
    pub line: i32,
    pub text_pos: i32,
    pub script_size: u32,
    pub script: Vec<Token>,
}

#[derive(Clone, Debug)]
pub struct StateData {
    pub probe_mask: u64,
    pub ignore_mask: u64,
    pub label_table_offset: u16,
    pub state_flags: u32,
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub class: ObjRef,
    pub deep: u32,
    pub script_text_crc: u32,
}

#[derive(Clone, Debug)]
pub struct ClassData {
    pub class_flags: u32,
    pub guid: [u8; 16],
    pub dependencies: Vec<Dependency>,
    pub package_imports: Vec<usize>,
    pub within: ObjRef,
    pub config_name: usize,
    pub hide_categories: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct FunctionData {
    pub native_index: u16,
    pub operator_precedence: u8,
    pub function_flags: u32,
    pub rep_offset: Option<u16>,
}

#[derive(Clone, Debug)]
pub enum PropertyKind {
    Byte { enum_: ObjRef },
    Int,
    Bool,
    Float,
    Name,
    Str,
    String,
    Pointer,
    Object { class: ObjRef },
    Class { class: ObjRef, meta_class: ObjRef },
    Array { inner: ObjRef },
    FixedArray { inner: ObjRef, count: i32 },
    Map { key: ObjRef, value: ObjRef },
    Struct { struct_: ObjRef },
    Delegate { function: ObjRef },
}

#[derive(Clone, Debug)]
pub struct PropertyData {
    pub field: FieldData,
    pub array_dim: u16,
    pub element_size: u16,
    pub property_flags: u32,
    pub category: usize,
    pub rep_offset: Option<u16>,
    pub kind: PropertyKind,
}

#[derive(Clone, Debug)]
pub enum Body {
    Class { structure: StructData, state: StateData, class: ClassData, defaults: Vec<OwnedTag> },
    State { structure: StructData, state: StateData },
    Function { structure: StructData, function: FunctionData },
    Struct { structure: StructData, defaults: Vec<OwnedTag> },
    Enum { field: FieldData, names: Vec<usize> },
    Const { field: FieldData, value: String },
    Property(PropertyData),
    TextBuffer { pos: u32, top: u32, text: String },
    /// Tagged properties only.
    Plain,
    /// An actor saved with `bDeleteMe` set, followed by a second tag list.
    Deleted { extra: Vec<OwnedTag> },
    /// The class's native data is not decoded yet; `Object::tail` holds it.
    Unparsed,
}

/// A tag with its data copied out of the package.
#[derive(Clone, Debug)]
pub struct OwnedTag {
    pub name: usize,
    pub ty: props::TagType,
    pub struct_name: Option<usize>,
    pub array_index: u32,
    pub bool_value: bool,
    pub data: Vec<u8>,
}

impl From<Tag<'_>> for OwnedTag {
    fn from(t: Tag<'_>) -> OwnedTag {
        OwnedTag {
            name: t.name,
            ty: t.ty,
            struct_name: t.struct_name,
            array_index: t.array_index,
            bool_value: t.bool_value,
            data: t.data.to_vec(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Object<'a> {
    /// The most-derived class whose native layout decided how this export was read.
    pub native_class: &'static str,
    pub state_frame: Option<StateFrame>,
    pub tags: Vec<Tag<'a>>,
    pub body: Body,
    /// Bytes after everything decoded: undecoded native data for `Body::Unparsed`, an error otherwise.
    pub tail: &'a [u8],
}

/// Native classes this module reads, matched against the export's class chain from most derived.
const KNOWN: &[&str] = &[
    "Class",
    "State",
    "Function",
    "Struct",
    "Enum",
    "Const",
    "ByteProperty",
    "IntProperty",
    "BoolProperty",
    "FloatProperty",
    "NameProperty",
    "StrProperty",
    "StringProperty",
    "PointerProperty",
    "ClassProperty",
    "ObjectProperty",
    "ArrayProperty",
    "FixedArrayProperty",
    "MapProperty",
    "StructProperty",
    "DelegateProperty",
    "TextBuffer",
    "Package",
];

/// Native classes with data after their tags that is not decoded yet. Exports of other classes are expected
/// to hold tagged properties only.
const NATIVE_DATA: &[&str] = &[
    "Palette", "Texture", "Cubemap", "BitmapMaterial", "StaticMesh", "SkeletalMesh", "LodMesh", "VertMesh", "Mesh",
    "MeshAnimation", "Sound", "Music", "Font", "Level", "LevelBase", "Model", "Polys", "Primitive", "TerrainSector",
    "TerrainPrimitive", "ConvexVolume", "MovieTexture", "Movie", "ScriptedTexture", "ShadowBitmapMaterial",
    "ProceduralTexture", "FractalTexture", "FireTexture", "WaterTexture", "WaveTexture", "WetTexture", "IceTexture",
    "StaticMeshInstance", "KMeshProps", "TerrainInfo",
];

fn classify(chain: &[String]) -> (&'static str, bool) {
    for c in chain {
        if let Some(k) = KNOWN.iter().find(|k| k.eq_ignore_ascii_case(c)) {
            return (k, true);
        }
        if let Some(k) = NATIVE_DATA.iter().find(|k| k.eq_ignore_ascii_case(c)) {
            return (k, false);
        }
    }
    ("Object", true)
}

fn read_field(r: &mut Reader<'_>) -> Result<FieldData> {
    Ok(FieldData { super_field: ObjRef(r.ci()?), next: ObjRef(r.ci()?) })
}

fn read_struct(pkg: &Package, r: &mut Reader<'_>) -> Result<StructData> {
    let field = read_field(r)?;
    let script_text = ObjRef(r.ci()?);
    let children = ObjRef(r.ci()?);
    let friendly_name = r.ci()? as usize;
    let cpp_text = ObjRef(r.ci()?);
    let line = r.i32()?;
    let text_pos = r.i32()?;
    let script_size = r.u32()?;
    if script_size > 0x10_0000 {
        return Err(Error::Format(format!("implausible script size {script_size:#x}")));
    }
    let script = bytecode::decode_script(pkg, r, script_size)?;
    Ok(StructData { field, script_text, children, friendly_name, cpp_text, line, text_pos, script_size, script })
}

fn read_state(r: &mut Reader<'_>) -> Result<StateData> {
    Ok(StateData {
        probe_mask: r.u64()?,
        ignore_mask: r.u64()?,
        label_table_offset: r.u16()?,
        state_flags: r.u32()?,
    })
}

fn read_class(r: &mut Reader<'_>) -> Result<ClassData> {
    let class_flags = r.u32()?;
    let guid = r.guid()?;
    let dependencies =
        r.array(|r| Ok(Dependency { class: ObjRef(r.ci()?), deep: r.u32()?, script_text_crc: r.u32()? }))?;
    let package_imports = r.array(|r| Ok(r.ci()? as usize))?;
    let within = ObjRef(r.ci()?);
    let config_name = r.ci()? as usize;
    let hide_categories = r.array(|r| Ok(r.ci()? as usize))?;
    Ok(ClassData { class_flags, guid, dependencies, package_imports, within, config_name, hide_categories })
}

fn read_property(kind_name: &str, r: &mut Reader<'_>) -> Result<PropertyData> {
    let field = read_field(r)?;
    let array_dim = r.u16()?;
    let element_size = r.u16()?;
    let property_flags = r.u32()?;
    let category = r.ci()? as usize;
    let rep_offset = if property_flags & CPF_NET != 0 { Some(r.u16()?) } else { None };
    let kind = match kind_name {
        "ByteProperty" => PropertyKind::Byte { enum_: ObjRef(r.ci()?) },
        "IntProperty" => PropertyKind::Int,
        "BoolProperty" => PropertyKind::Bool,
        "FloatProperty" => PropertyKind::Float,
        "NameProperty" => PropertyKind::Name,
        "StrProperty" => PropertyKind::Str,
        "StringProperty" => PropertyKind::String,
        "PointerProperty" => PropertyKind::Pointer,
        "ObjectProperty" => PropertyKind::Object { class: ObjRef(r.ci()?) },
        "ClassProperty" => PropertyKind::Class { class: ObjRef(r.ci()?), meta_class: ObjRef(r.ci()?) },
        "ArrayProperty" => PropertyKind::Array { inner: ObjRef(r.ci()?) },
        "FixedArrayProperty" => PropertyKind::FixedArray { inner: ObjRef(r.ci()?), count: r.ci()? },
        "MapProperty" => PropertyKind::Map { key: ObjRef(r.ci()?), value: ObjRef(r.ci()?) },
        "StructProperty" => PropertyKind::Struct { struct_: ObjRef(r.ci()?) },
        "DelegateProperty" => PropertyKind::Delegate { function: ObjRef(r.ci()?) },
        other => return Err(Error::Format(format!("unknown property class {other}"))),
    };
    Ok(PropertyData { field, array_dim, element_size, property_flags, category, rep_offset, kind })
}

fn owned(tags: Vec<Tag<'_>>) -> Vec<OwnedTag> {
    tags.into_iter().map(OwnedTag::from).collect()
}

/// Reads export `index` of `pkg`, whose class chain (most derived first) is `chain`.
pub fn read_export<'a>(pkg: &'a Package, index: usize, chain: &[String]) -> Result<Object<'a>> {
    let exp = &pkg.exports[index];
    let mut r = Reader::with_base(pkg.export_data(index), exp.serial_offset);
    let (native_class, known) = classify(chain);
    let state_frame = if exp.flags & RF_HAS_STACK != 0 { Some(read_state_frame(&mut r)?) } else { None };
    let tags = if native_class == "Class" { Vec::new() } else { props::read_tags(pkg, &mut r)? };
    if !known {
        let tail = r.rest();
        return Ok(Object { native_class, state_frame, tags, body: Body::Unparsed, tail });
    }
    let body = match native_class {
        "Class" => {
            let structure = read_struct(pkg, &mut r)?;
            let state = read_state(&mut r)?;
            let class = read_class(&mut r)?;
            let defaults = owned(props::read_tags(pkg, &mut r)?);
            Body::Class { structure, state, class, defaults }
        }
        "State" => {
            let structure = read_struct(pkg, &mut r)?;
            let state = read_state(&mut r)?;
            Body::State { structure, state }
        }
        "Function" => {
            let structure = read_struct(pkg, &mut r)?;
            let native_index = r.u16()?;
            let operator_precedence = r.u8()?;
            let function_flags = r.u32()?;
            let rep_offset = if function_flags & FUNC_NET != 0 { Some(r.u16()?) } else { None };
            Body::Function {
                structure,
                function: FunctionData { native_index, operator_precedence, function_flags, rep_offset },
            }
        }
        "Struct" => {
            let structure = read_struct(pkg, &mut r)?;
            Body::Struct { structure, defaults: Vec::new() }
        }
        "Enum" => {
            let field = read_field(&mut r)?;
            let names = r.array(|r| Ok(r.ci()? as usize))?;
            Body::Enum { field, names }
        }
        "Const" => {
            let field = read_field(&mut r)?;
            Body::Const { field, value: r.fstring()? }
        }
        "TextBuffer" => Body::TextBuffer { pos: r.u32()?, top: r.u32()?, text: r.fstring()? },
        "Package" | "Object" => {
            // Actors deleted in the editor but still saved with a map carry a second tag list.
            let deleted = tags.iter().any(|t| t.bool_value && pkg.name(t.name).eq_ignore_ascii_case("bDeleteMe"));
            if deleted && !r.at_end() {
                let mut extra = r.clone();
                if let Ok(more) = props::read_tags(pkg, &mut extra)
                    && extra.at_end()
                {
                    r = extra;
                    let tail = r.rest();
                    return Ok(Object { native_class, state_frame, tags, body: Body::Deleted { extra: owned(more) }, tail });
                }
            }
            Body::Plain
        }
        prop => Body::Property(read_property(prop, &mut r)?),
    };
    let tail = r.rest();
    Ok(Object { native_class, state_frame, tags, body, tail })
}
