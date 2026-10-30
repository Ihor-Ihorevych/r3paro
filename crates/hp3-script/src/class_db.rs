//! Classes, states, functions and structs loaded from packages, with a slot layout of their own and default
//! values built from the classes' default properties.
//!
//! The layout does not follow any native memory layout: each property gets `array_dim` consecutive slots in a
//! `Vec<Value>`, inherited properties first. Natives find properties by name.

use crate::fx::FxHashMap as HashMap;
use std::rc::Rc;

use hp3_package::bytecode::Token;
use hp3_package::object::{self, Body, OwnedTag, PropertyKind};
use hp3_package::props::{self, TagType};
use hp3_package::{ObjRef, Package, PackageSet, Reader};

use crate::names::{Name, Names};
use crate::objects::{ObjKind, ObjectTable};
use crate::value::{ObjId, ObjKey, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum PropType {
    Byte,
    Int,
    Bool,
    Float,
    Name,
    Str,
    Pointer,
    Object,
    Class,
    Array(Box<PropType>),
    FixedArray(Box<PropType>, usize),
    Map,
    Struct(StructId),
    Delegate,
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub name: Name,
    pub ty: PropType,
    pub dim: usize,
    pub flags: u32,
    pub slot: usize,
    pub owner: StructId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructKind {
    Class,
    State,
    Function,
    Struct,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub key: ObjKey,
    pub name: Name,
    pub kind: StructKind,
    pub super_: Option<StructId>,
    pub props: Vec<Prop>,
    by_name: HashMap<Name, usize>,
    pub slots: usize,
    pub defaults: Vec<Value>,
    /// The package and export it was loaded from; token operands refer into this package.
    pub pkg: Rc<Package>,
    pub export: usize,
    /// Its entry in the object table.
    pub object: ObjId,
    pub script: Rc<Vec<Token>>,
    pub script_size: u32,
    /// For functions.
    pub native_index: u16,
    pub function_flags: u32,
    pub operator_precedence: u8,
    /// For states: probe mask, ignore mask, label table offset, state flags.
    pub state_info: (u64, u64, u16, u32),
    /// Functions and states declared directly in this struct, by name.
    pub functions: HashMap<Name, StructId>,
    pub states: HashMap<Name, StructId>,
}

impl StructDef {
    pub fn prop(&self, name: Name) -> Option<&Prop> {
        self.by_name.get(&name).map(|&i| &self.props[i])
    }
}

#[derive(Default, Debug)]
pub struct LoadStats {
    pub structs: usize,
    pub classes: usize,
    pub props: usize,
    pub tags_applied: usize,
    pub unknown_tags: usize,
    pub mismatched_tags: usize,
    pub unresolved_objects: usize,
    pub problems: Vec<String>,
}

impl LoadStats {
    fn problem(&mut self, s: String) {
        if self.problems.len() < 40 {
            self.problems.push(s);
        }
    }
}

/// Structs whose tagged-property data is the raw members, in this order.
fn binary_members(name: &str) -> Option<&'static [&'static str]> {
    Some(match name.to_ascii_lowercase().as_str() {
        "vector" => &["X", "Y", "Z"],
        "rotator" => &["Pitch", "Yaw", "Roll"],
        "color" => &["B", "G", "R", "A"],
        "quat" => &["X", "Y", "Z", "W"],
        "guid" => &["A", "B", "C", "D"],
        "coords" => &["Origin", "XAxis", "YAxis", "ZAxis"],
        "box" => &["Min", "Max", "IsValid"],
        "intbox" => &["X1", "Y1", "X2", "Y2"],
        "floatbox" => &["X1", "Y1", "X2", "Y2"],
        _ => return None,
    })
}

pub struct ClassDb<'s> {
    pub set: &'s PackageSet,
    pub names: Names,
    pub structs: Vec<StructDef>,
    by_key: HashMap<(Rc<str>, usize), StructId>,
    package_names: HashMap<String, Rc<str>>,
    native_classes: HashMap<String, Option<StructId>>,
    pub objects: ObjectTable,
    pub stats: LoadStats,
}

impl<'s> ClassDb<'s> {
    pub fn new(set: &'s PackageSet) -> Self {
        ClassDb {
            set,
            names: Names::default(),
            structs: Vec::new(),
            by_key: HashMap::default(),
            package_names: HashMap::default(),
            native_classes: HashMap::default(),
            objects: ObjectTable::default(),
            stats: LoadStats::default(),
        }
    }

    pub fn get(&self, id: StructId) -> &StructDef {
        &self.structs[id.0 as usize]
    }

    pub fn key(&mut self, pkg: &Package, index: usize) -> ObjKey {
        let lower = pkg.name.to_ascii_lowercase();
        let rc = self.package_names.entry(lower.clone()).or_insert_with(|| Rc::from(lower.as_str())).clone();
        (rc, index)
    }

    fn name_of(&mut self, pkg: &Package, index: usize) -> Name {
        self.names.intern(pkg.name(index))
    }

    /// The object-table entry of a package export, allocated on first use.
    pub fn obj_for(&mut self, pkg: &Package, index: usize) -> ObjId {
        let key = self.key(pkg, index);
        let name = self.name_of(pkg, pkg.exports[index].name);
        self.objects.for_key(key, name)
    }

    /// The object an object reference of `pkg` points to.
    pub fn resolve_obj(&mut self, pkg: &Rc<Package>, r: ObjRef) -> Option<ObjId> {
        if r.is_none() {
            return None;
        }
        match self.set.resolve(pkg, r) {
            Some((p, i)) => Some(self.obj_for(&p, i)),
            // A native-only class (`class'Mesh'`): its synthetic class object.
            None if r.import().and_then(|i| pkg.imports.get(i)).is_some_and(|imp| pkg.name(imp.class_name).eq_ignore_ascii_case("Class"))
                && let Some(c) = self.native_class(pkg.ref_name(r)) =>
            {
                Some(self.get(c).object)
            }
            None => {
                self.stats.unresolved_objects += 1;
                let s = format!("{}: unresolved {}", pkg.name, pkg.ref_path(r));
                self.stats.problem(s);
                None
            }
        }
    }

    /// Finds a loaded class or struct by name.
    pub fn find(&self, name: &str) -> Option<StructId> {
        let n = self.names.find(name)?;
        self.structs
            .iter()
            .position(|s| s.name == n && matches!(s.kind, StructKind::Class | StructKind::Struct))
            .map(|i| StructId(i as u32))
    }

    /// Loads every class in every script package.
    pub fn load_all_classes(&mut self) {
        for path in self.set.files() {
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("u")) {
                continue;
            }
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            let Ok(pkg) = self.set.get(&stem) else { continue };
            for i in 0..pkg.exports.len() {
                if pkg.exports[i].class.is_none() {
                    self.load_struct(&pkg, i);
                }
            }
        }
    }

    /// Loads a class, state, function or struct export (and everything it inherits from).
    pub fn load_struct(&mut self, pkg: &Rc<Package>, index: usize) -> Option<StructId> {
        let key = self.key(pkg, index);
        if let Some(&id) = self.by_key.get(&key) {
            return Some(id);
        }
        let chain = self.set.export_chain(pkg, index);
        let o = object::read_export(pkg, index, &chain).ok()?;
        let mut function = (0u16, 0u32, 0u8);
        let mut state_info = (0, 0, 0, 0);
        let (structure, kind, class_defaults) = match &o.body {
            Body::Class { structure, defaults, state, .. } => {
                state_info = (state.probe_mask, state.ignore_mask, state.label_table_offset, state.state_flags);
                (structure.clone(), StructKind::Class, Some(defaults.clone()))
            }
            Body::State { structure, state } => {
                state_info = (state.probe_mask, state.ignore_mask, state.label_table_offset, state.state_flags);
                (structure.clone(), StructKind::State, None)
            }
            Body::Function { structure, function: f } => {
                function = (f.native_index, f.function_flags, f.operator_precedence);
                (structure.clone(), StructKind::Function, None)
            }
            Body::Struct { structure, .. } => (structure.clone(), StructKind::Struct, None),
            _ => return None,
        };
        let super_ = match self.set.resolve(pkg, structure.field.super_field) {
            Some((sp, si)) => self.load_struct(&sp, si),
            None => None,
        };
        let name = self.name_of(pkg, pkg.exports[index].name);
        let id = StructId(self.structs.len() as u32);
        // A function's super is the function it overrides; its parameters are its own.
        let (mut props, mut slots, mut defaults) = match super_.filter(|_| kind != StructKind::Function) {
            Some(s) => {
                let sd = self.get(s);
                (sd.props.clone(), sd.slots, sd.defaults.clone())
            }
            None => (Vec::new(), 0, Vec::new()),
        };
        // Reserve the id before loading children, which may refer back to this struct.
        self.by_key.insert(key.clone(), id);
        let object = self.objects.for_key(key.clone(), name);
        self.objects.get_mut(object).kind = ObjKind::Struct(id);
        self.structs.push(StructDef {
            key,
            name,
            kind,
            super_,
            props: Vec::new(),
            by_name: HashMap::default(),
            slots: 0,
            defaults: Vec::new(),
            pkg: pkg.clone(),
            export: index,
            object,
            script: Rc::new(structure.script.clone()),
            script_size: structure.script_size,
            native_index: function.0,
            function_flags: function.1,
            operator_precedence: function.2,
            state_info,
            functions: HashMap::default(),
            states: HashMap::default(),
        });
        let mut functions = HashMap::default();
        let mut states = HashMap::default();
        let mut own = Vec::new();
        let mut child = structure.children;
        let mut guard = 0;
        while let Some(ci) = child.export() {
            guard += 1;
            if guard > 10_000 || ci >= pkg.exports.len() {
                break;
            }
            let cchain = self.set.export_chain(pkg, ci);
            let Ok(co) = object::read_export(pkg, ci, &cchain) else { break };
            let next = match &co.body {
                Body::Property(p) => {
                    let ty = self.prop_type(pkg, &p.kind);
                    own.push((self.name_of(pkg, pkg.exports[ci].name), ty, p.array_dim.max(1) as usize, p.property_flags));
                    p.field.next
                }
                Body::Function { structure, .. } => {
                    let next = structure.field.next;
                    if let Some(f) = self.load_struct(pkg, ci) {
                        functions.insert(self.get(f).name, f);
                    }
                    next
                }
                Body::State { structure, .. } => {
                    let next = structure.field.next;
                    if let Some(st) = self.load_struct(pkg, ci) {
                        states.insert(self.get(st).name, st);
                    }
                    next
                }
                Body::Class { structure, .. } | Body::Struct { structure, .. } => structure.field.next,
                Body::Enum { field, .. } | Body::Const { field, .. } => field.next,
                _ => break,
            };
            child = next;
        }
        // Children are linked in declaration order, which is also parameter order for functions.
        for (pname, ty, dim, flags) in own {
            let default = self.default_value(&ty);
            for _ in 0..dim {
                defaults.push(default.clone());
            }
            props.push(Prop { name: pname, ty, dim, flags, slot: slots, owner: id });
            slots += dim;
            self.stats.props += 1;
        }
        let by_name = props.iter().enumerate().map(|(i, p)| (p.name, i)).collect();
        {
            let def = &mut self.structs[id.0 as usize];
            def.props = props;
            def.by_name = by_name;
            def.slots = slots;
            def.defaults = defaults;
            def.functions = functions;
            def.states = states;
        }
        if let Some(tags) = class_defaults {
            let mut values = std::mem::take(&mut self.structs[id.0 as usize].defaults);
            self.apply_tags(pkg, id, &tags, &mut values);
            self.structs[id.0 as usize].defaults = values;
            self.stats.classes += 1;
        }
        self.stats.structs += 1;
        Some(id)
    }

    fn prop_type(&mut self, pkg: &Rc<Package>, kind: &PropertyKind) -> PropType {
        match kind {
            PropertyKind::Byte { .. } => PropType::Byte,
            PropertyKind::Int => PropType::Int,
            PropertyKind::Bool => PropType::Bool,
            PropertyKind::Float => PropType::Float,
            PropertyKind::Name => PropType::Name,
            PropertyKind::Str | PropertyKind::String => PropType::Str,
            PropertyKind::Pointer => PropType::Pointer,
            PropertyKind::Object { .. } => PropType::Object,
            PropertyKind::Class { .. } => PropType::Class,
            PropertyKind::Map { .. } => PropType::Map,
            PropertyKind::Delegate { .. } => PropType::Delegate,
            PropertyKind::Struct { struct_ } => match self.set.resolve(pkg, *struct_) {
                Some((sp, si)) => match self.load_struct(&sp, si) {
                    Some(id) => PropType::Struct(id),
                    None => PropType::Int,
                },
                None => PropType::Int,
            },
            PropertyKind::Array { inner } | PropertyKind::FixedArray { inner, .. } => {
                let inner_ty = self
                    .set
                    .resolve(pkg, *inner)
                    .and_then(|(ip, ii)| {
                        let chain = self.set.export_chain(&ip, ii);
                        let o = object::read_export(&ip, ii, &chain).ok()?;
                        match &o.body {
                            Body::Property(p) => Some((ip.clone(), p.kind.clone())),
                            _ => None,
                        }
                    })
                    .map(|(ip, k)| self.prop_type(&ip, &k))
                    .unwrap_or(PropType::Int);
                match kind {
                    PropertyKind::FixedArray { count, .. } => PropType::FixedArray(Box::new(inner_ty), *count as usize),
                    _ => PropType::Array(Box::new(inner_ty)),
                }
            }
        }
    }

    pub fn default_value(&self, ty: &PropType) -> Value {
        match ty {
            PropType::Byte => Value::Byte(0),
            PropType::Int | PropType::Pointer => Value::Int(0),
            PropType::Bool => Value::Bool(false),
            PropType::Float => Value::Float(0.0),
            PropType::Name => Value::Name(Name::NONE),
            PropType::Str => Value::Str(String::new()),
            PropType::Object | PropType::Class => Value::Object(None),
            PropType::Array(_) | PropType::FixedArray(..) | PropType::Map => Value::Array(Vec::new()),
            PropType::Struct(id) => {
                // A struct still being loaded (one that contains itself indirectly) has no defaults yet.
                Value::Struct(self.structs.get(id.0 as usize).map(|s| s.defaults.clone()).unwrap_or_default().into())
            }
            PropType::Delegate => Value::Delegate(None, Name::NONE),
        }
    }

    /// The class of an export, loaded.
    pub fn class_of(&mut self, pkg: &Rc<Package>, index: usize) -> Option<StructId> {
        let class = pkg.exports.get(index)?.class;
        match self.set.resolve(pkg, class) {
            Some((cp, ci)) => self.load_struct(&cp, ci),
            None if !class.is_none() => self.native_class(pkg.ref_name(class)),
            None => None,
        }
    }

    /// A class implemented only in native code (`Mesh`, `SkeletalMesh`, `Sound`...), which no package defines:
    /// a class without properties whose parents come from the built-in hierarchy, so that casts and `IsA`
    /// work on its objects. A script class of that name is returned instead when one exists.
    pub fn native_class(&mut self, name: &str) -> Option<StructId> {
        let lower = name.to_ascii_lowercase();
        if let Some(c) = self.native_classes.get(&lower) {
            return *c;
        }
        let found = match self.find(name) {
            Some(id) => Some(id),
            None => match hp3_package::builtin_super(name) {
                Some(parent) => self.native_class(parent).map(|super_| {
                    let n = self.names.intern(name);
                    let id = StructId(self.structs.len() as u32);
                    let object = self.objects.add_struct(n, id);
                    let sd = self.get(super_);
                    let def = StructDef {
                        key: (Rc::from("<native>"), id.0 as usize),
                        name: n,
                        kind: StructKind::Class,
                        super_: Some(super_),
                        props: sd.props.clone(),
                        by_name: sd.by_name.clone(),
                        slots: sd.slots,
                        defaults: sd.defaults.clone(),
                        pkg: sd.pkg.clone(),
                        export: sd.export,
                        object,
                        script: Rc::new(Vec::new()),
                        script_size: 0,
                        native_index: 0,
                        function_flags: 0,
                        operator_precedence: 0,
                        state_info: (0, 0, 0, 0),
                        functions: HashMap::default(),
                        states: HashMap::default(),
                    };
                    self.structs.push(def);
                    id
                }),
                None => None,
            },
        };
        self.native_classes.insert(lower, found);
        found
    }

    /// The property values of an export: its class's defaults with the export's own tags applied.
    pub fn instance_values(&mut self, pkg: &Rc<Package>, index: usize) -> Option<(StructId, Vec<Value>)> {
        let class = self.class_of(pkg, index)?;
        let chain = self.set.export_chain(pkg, index);
        let o = object::read_export(pkg, index, &chain).ok()?;
        let tags: Vec<OwnedTag> = o.tags.into_iter().map(OwnedTag::from).collect();
        let mut values = self.get(class).defaults.clone();
        self.apply_tags(pkg, class, &tags, &mut values);
        Some((class, values))
    }

    /// Applies tagged properties of `pkg` to `values`, laid out as struct `id`.
    pub fn apply_tags(&mut self, pkg: &Rc<Package>, id: StructId, tags: &[OwnedTag], values: &mut [Value]) {
        for tag in tags {
            let name = self.name_of(pkg, tag.name);
            let Some(prop) = self.get(id).prop(name).cloned() else {
                self.stats.unknown_tags += 1;
                let s = format!("{}: no property {} in {}", pkg.name, self.names.get(name), self.names.get(self.get(id).name));
                self.stats.problem(s);
                continue;
            };
            let slot = prop.slot + tag.array_index as usize;
            if tag.array_index as usize >= prop.dim || slot >= values.len() {
                self.stats.mismatched_tags += 1;
                continue;
            }
            let value = if tag.ty == TagType::Bool {
                Some(Value::Bool(tag.bool_value))
            } else {
                let mut r = Reader::new(&tag.data);
                let v = self.decode(pkg, &prop.ty, &mut r);
                if v.is_none() || !r.at_end() {
                    self.stats.mismatched_tags += 1;
                    let s = format!(
                        "{}: {}.{} ({:?}, {} bytes) left {} bytes",
                        pkg.name,
                        self.names.get(self.get(id).name),
                        self.names.get(name),
                        prop.ty,
                        tag.data.len(),
                        r.remaining()
                    );
                    self.stats.problem(s);
                    None
                } else {
                    v
                }
            };
            if let Some(v) = value {
                values[slot] = v;
                self.stats.tags_applied += 1;
            }
        }
    }

    fn decode(&mut self, pkg: &Rc<Package>, ty: &PropType, r: &mut Reader<'_>) -> Option<Value> {
        Some(match ty {
            PropType::Byte => Value::Byte(r.u8().ok()?),
            PropType::Int | PropType::Pointer => Value::Int(r.i32().ok()?),
            PropType::Bool => Value::Bool(r.u8().ok()? != 0),
            PropType::Float => Value::Float(r.f32().ok()?),
            PropType::Name => {
                let n = r.ci().ok()?;
                Value::Name(self.name_of(pkg, usize::try_from(n).ok()?))
            }
            PropType::Str => Value::Str(r.fstring().ok()?),
            PropType::Object | PropType::Class => {
                let o = ObjRef(r.ci().ok()?);
                Value::Object(self.resolve_obj(pkg, o))
            }
            PropType::Delegate => {
                let o = ObjRef(r.ci().ok()?);
                let n = r.ci().ok()?;
                let obj = self.resolve_obj(pkg, o);
                Value::Delegate(obj, self.name_of(pkg, usize::try_from(n).ok()?))
            }
            PropType::Struct(id) => self.decode_struct(pkg, *id, r)?,
            PropType::Array(inner) => {
                let n = r.count().ok()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.decode(pkg, inner, r)?);
                }
                Value::Array(v)
            }
            PropType::FixedArray(..) | PropType::Map => {
                r.rest();
                Value::Array(Vec::new())
            }
        })
    }

    fn decode_struct(&mut self, pkg: &Rc<Package>, id: StructId, r: &mut Reader<'_>) -> Option<Value> {
        let def_name = self.names.get(self.get(id).name).to_string();
        let mut values = self.get(id).defaults.clone();
        if let Some(members) = binary_members(&def_name) {
            for m in members {
                let n = self.names.find(m)?;
                let prop = self.get(id).prop(n)?.clone();
                values[prop.slot] = self.decode(pkg, &prop.ty, r)?;
            }
        } else {
            let tags: Vec<OwnedTag> = props::read_tags(pkg, r).ok()?.into_iter().map(OwnedTag::from).collect();
            self.apply_tags(pkg, id, &tags, &mut values);
        }
        Some(Value::Struct(values.into()))
    }
}
