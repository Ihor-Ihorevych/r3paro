//! The object table: every object scripts can reference, whether it lives in a package or was created at run
//! time. Package objects get an entry when first referenced and are filled in when first needed.

use crate::fx::FxHashMap as HashMap;

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
    /// Per object, the `epoch` of its last possible change (a property written, the entry replaced): lets a per-frame pass
    /// skip objects nobody touched since it last looked (see `changed_since`). Parallel to `entries`.
    stamps: Vec<u32>,
    /// Counts the passes that use `changed_since`; starts at 1 so a new object (stamped with it) is seen by the first pass.
    /// Writes by scripts to a property at this slot or above (a subclass's own variable) leave the stamp alone: what the
    /// passes look at are `Actor`'s properties (place, size, collision), below it. Set by the world (0: every write stamps).
    pub quiet_from: usize,
    pub epoch: u32,
}

impl ObjectTable {
    /// The id of a package object, allocating a stub entry the first time.
    pub fn for_key(&mut self, key: ObjKey, name: Name) -> ObjId {
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let id = ObjId(self.entries.len() as u32);
        self.entries.push(ObjEntry { key: Some(key.clone()), name, class: None, kind: ObjKind::Stub, deleted: false });
        self.stamps.push(self.epoch);
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
        self.stamps.push(self.epoch);
        id
    }

    /// The object of a struct that no package defines (a native-only class).
    pub fn add_struct(&mut self, name: Name, id: StructId) -> ObjId {
        let obj = ObjId(self.entries.len() as u32);
        self.entries.push(ObjEntry { key: None, name, class: None, kind: ObjKind::Struct(id), deleted: false });
        self.stamps.push(self.epoch);
        obj
    }

    pub fn get(&self, id: ObjId) -> &ObjEntry {
        &self.entries[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: ObjId) -> &mut ObjEntry {
        self.stamps[id.0 as usize] = self.epoch;
        &mut self.entries[id.0 as usize]
    }

    /// Starts a pass that skips unchanged objects: returns the epoch to hand to `changed_since` in the pass after this
    /// one. Anything written from now on (during this pass too) counts as changed for that next pass.
    pub fn begin_pass(&mut self) -> u32 {
        self.epoch += 1;
        self.epoch
    }

    /// Whether the object may have been written (a property set, its entry replaced or created) since the pass that
    /// returned `seen` from `begin_pass` (0: ever).
    #[inline]
    pub fn changed_since(&self, id: ObjId, seen: u32) -> bool {
        self.stamps[id.0 as usize] >= seen
    }

    pub fn values(&self, id: ObjId) -> Option<&Vec<Value>> {
        match &self.entries.get(id.0 as usize)?.kind {
            ObjKind::Instance { values, .. } => Some(values),
            _ => None,
        }
    }

    /// The values for a write that does not count as a change (see `quiet_from`).
    pub fn values_mut_quiet(&mut self, id: ObjId) -> Option<&mut Vec<Value>> {
        match &mut self.entries.get_mut(id.0 as usize)?.kind {
            ObjKind::Instance { values, .. } => Some(values),
            _ => None,
        }
    }

    /// Counts the object as changed.
    pub fn stamp(&mut self, id: ObjId) {
        if let Some(s) = self.stamps.get_mut(id.0 as usize) {
            *s = self.epoch;
        }
    }

    pub fn values_mut(&mut self, id: ObjId) -> Option<&mut Vec<Value>> {
        if let Some(s) = self.stamps.get_mut(id.0 as usize) {
            *s = self.epoch;
        }
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
