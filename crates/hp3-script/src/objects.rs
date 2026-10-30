//! The object table: every object scripts can reference, whether it lives in a package or was created at run
//! time. Package objects get an entry when first referenced and are filled in when first needed.

use std::collections::HashMap;

use crate::class_db::StructId;
use crate::names::Name;
use crate::value::{ObjId, ObjKey, Value};

#[derive(Clone, Debug)]
pub enum ObjKind {
    /// Referenced but not loaded yet.
    Stub,
    /// A stub that could not be loaded (an actor deleted in the editor, an object of a class without script).
    Missing,
    /// A class, state, function or struct.
    Struct(StructId),
    /// An instance of a script class.
    Instance { values: Vec<Value>, state: Option<StructId> },
    /// A package object of a native-only class (texture, mesh, sound...), kept as a reference.
    Native,
}

#[derive(Clone, Debug)]
pub struct ObjEntry {
    pub key: Option<ObjKey>,
    pub name: Name,
    pub class: Option<StructId>,
    pub kind: ObjKind,
    pub deleted: bool,
}

#[derive(Default)]
pub struct ObjectTable {
    pub entries: Vec<ObjEntry>,
    by_key: HashMap<ObjKey, ObjId>,
}

impl ObjectTable {
    /// The id of a package object, allocating a stub entry the first time.
    pub fn for_key(&mut self, key: ObjKey, name: Name) -> ObjId {
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let id = ObjId(self.entries.len() as u32);
        self.entries.push(ObjEntry { key: Some(key.clone()), name, class: None, kind: ObjKind::Stub, deleted: false });
        self.by_key.insert(key, id);
        id
    }

    pub fn add(&mut self, name: Name, class: StructId, values: Vec<Value>) -> ObjId {
        let id = ObjId(self.entries.len() as u32);
        self.entries.push(ObjEntry {
            key: None,
            name,
            class: Some(class),
            kind: ObjKind::Instance { values, state: None },
            deleted: false,
        });
        id
    }

    /// The object of a struct that no package defines (a native-only class).
    pub fn add_struct(&mut self, name: Name, id: StructId) -> ObjId {
        let obj = ObjId(self.entries.len() as u32);
        self.entries.push(ObjEntry { key: None, name, class: None, kind: ObjKind::Struct(id), deleted: false });
        obj
    }

    pub fn get(&self, id: ObjId) -> &ObjEntry {
        &self.entries[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: ObjId) -> &mut ObjEntry {
        &mut self.entries[id.0 as usize]
    }

    pub fn values(&self, id: ObjId) -> Option<&Vec<Value>> {
        match &self.entries.get(id.0 as usize)?.kind {
            ObjKind::Instance { values, .. } => Some(values),
            _ => None,
        }
    }

    pub fn values_mut(&mut self, id: ObjId) -> Option<&mut Vec<Value>> {
        match &mut self.entries.get_mut(id.0 as usize)?.kind {
            ObjKind::Instance { values, .. } => Some(values),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
