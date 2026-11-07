//! The interpreter: runs compiled functions against the object table.

use crate::fx::FxHashMap as HashMap;
use std::rc::Rc;

use crate::class_db::{ClassDb, PropType, StructId, StructKind};
use crate::compile::{Code, Compiler, Expr};
use crate::names::Name;
use crate::natives::{self, NativeFn};
use crate::objects::ObjKind;
use crate::value::{ObjId, Value};

pub const CPF_OPTIONAL_PARM: u32 = 0x0000_0010;
pub const CPF_PARM: u32 = 0x0000_0080;
pub const CPF_OUT_PARM: u32 = 0x0000_0100;
pub const CPF_RETURN_PARM: u32 = 0x0000_0400;
pub const FUNC_NATIVE: u32 = 0x0000_0400;

#[derive(Debug)]
pub enum VmError {
    /// A native or opcode the VM does not implement yet.
    Unimplemented(String),
    Runtime(String),
    /// Too many nested calls.
    Recursion,
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::Unimplemented(s) => write!(f, "unimplemented: {s}"),
            VmError::Runtime(s) => write!(f, "{s}"),
            VmError::Recursion => write!(f, "infinite script recursion"),
        }
    }
}

impl std::error::Error for VmError {}

pub type VmResult<T> = Result<T, VmError>;

pub struct Frame {
    pub func: StructId,
    pub this: Option<ObjId>,
    pub locals: Vec<Value>,
    switch_value: Option<Value>,
    iters: Vec<IterState>,
}

impl Frame {
    pub fn new(func: StructId, this: Option<ObjId>, locals: Vec<Value>) -> Frame {
        Frame { func, this, locals, switch_value: None, iters: Vec::new() }
    }
}

/// What calls need to know about a function, computed once.
struct FuncInfo {
    /// Parameters in order: slot, property flags, type.
    params: Vec<(usize, u32, PropType)>,
    ret: Option<usize>,
    native: bool,
    /// Whether the function is a state (its code runs as state code).
    is_state: bool,
}

/// A running `foreach`: the values each iteration writes to the out arguments.
struct IterState {
    items: Vec<Vec<Value>>,
    index: usize,
    places: Vec<(usize, Place)>,
    /// First statement of the loop body.
    body: usize,
}

pub enum RunEnd {
    Finished,
    /// State code waiting on a latent action; resumes at this statement.
    Suspended(usize),
}

/// What suspended state code is waiting for.
#[derive(Clone, Debug, PartialEq)]
pub enum Latent {
    Sleep(f32),
    /// Waiting on something the world decides (animation end, movement); cleared with `finish_latent`.
    Other(&'static str),
}

/// An iterator native: one entry per iteration, each holding the values of all arguments.
pub type IterFn = fn(&mut Vm<'_>, Option<ObjId>, &[Value]) -> VmResult<Vec<Vec<Value>>>;

#[derive(Clone, Debug)]
enum Root {
    Local,
    Instance(ObjId),
    Default(StructId),
    Temp(Value),
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Slot(usize),
    Index(usize),
}

#[derive(Clone, Debug)]
pub struct Place {
    root: Root,
    path: smallvec::SmallVec<[Step; 4]>,
}

enum Flow {
    Next,
    Jump(usize),
    Return,
    Stop,
}

pub struct Vm<'s> {
    pub db: ClassDb<'s>,
    code: HashMap<StructId, Rc<Code>>,
    native_decls: HashMap<u16, StructId>,
    natives: HashMap<String, NativeFn>,
    /// Script log lines (`Log`, warnings, accessed-none reports).
    pub log: Vec<String>,
    depth: usize,
    /// Number of statements executed, for runaway detection and statistics.
    pub steps: u64,
    /// State of the deterministic random number generator.
    pub rng: u32,
    /// `Object.AndAnd_BoolBool` and `Object.OrOr_BoolBool`, whose right operand is evaluated only when needed.
    short_circuit: [Option<StructId>; 2],
    iterators: HashMap<String, IterFn>,
    /// Objects whose state code is waiting, and on what.
    pub latent: HashMap<ObjId, Latent>,
    /// Where each object's state code resumes.
    /// Where an object's state code continues: the state that owns that code (a state inherits the code of the
    /// state of the same name in its parent class) and the statement.
    pub state_pc: HashMap<ObjId, (StructId, usize)>,
    /// Objects that changed state while their state code ran.
    pub(crate) state_changed: crate::fx::FxHashSet<ObjId>,
    /// Objects whose EndState is running, and how many times each object has had its state set (so a GotoState
    /// can tell that the EndState it sent moved the object on).
    pub(crate) in_end_state: crate::fx::FxHashSet<ObjId>,
    /// Events a script switched off for an object (`Disable('Tick')`): the engine does not raise them until the script
    /// switches them on again or the object enters a state (which starts with every event on).
    pub(crate) disabled_events: HashMap<ObjId, Vec<Name>>,
    pub(crate) state_sets: HashMap<ObjId, u32>,
    /// Statement being executed, for iterators to find their loop body.
    current_pc: usize,
    /// Script offset of the statement being run (for messages).
    current_offset: u32,
    /// World data of the crate that embeds the VM.
    pub ext: Option<Box<dyn std::any::Any>>,
    /// Resolved natives, indexed by function struct id (outer `None`: not looked up yet).
    native_cache: Vec<Option<Option<NativeFn>>>,
    /// Script functions replaced by a native, by lower-case `Class.Function`, and the lookup of them by function struct id.
    script_overrides: HashMap<String, NativeFn>,
    override_cache: Vec<Option<Option<NativeFn>>>,
    trivial: HashMap<StructId, bool>,
    /// Count statements per function when set.
    pub profile: bool,
    /// Log every state change.
    pub trace_states: bool,
    /// Print calls of functions whose path contains this (lower-case), from `HP3_TRACE_CALLS`.
    pub trace_calls: Option<String>,
    /// `Engine.Actor`, when loaded: actors are never loaded lazily.
    actor_class: Option<StructId>,
    pub profile_counts: HashMap<StructId, u64>,
    /// Calls and time spent per native function, when profiling.
    pub profile_natives: HashMap<StructId, (u64, std::time::Duration)>,
    /// Per-function call information, indexed by struct id.
    func_info: Vec<Option<Rc<FuncInfo>>>,
    function_cache: HashMap<(StructId, Name), Option<StructId>>,
    /// Skip unimplemented natives instead of failing, counting them in `missing`.
    pub lenient: bool,
    pub missing: std::collections::BTreeMap<String, u64>,
    /// The state each object was in before its last state change.
    pub last_state: HashMap<ObjId, Option<StructId>>,
}

impl<'s> Vm<'s> {
    /// Builds a VM over a class database whose classes are already loaded.
    pub fn new(db: ClassDb<'s>) -> Self {
        let mut native_decls = HashMap::default();
        for (i, s) in db.structs.iter().enumerate() {
            if s.kind == StructKind::Function && s.function_flags & FUNC_NATIVE != 0 && s.native_index != 0 {
                native_decls.entry(s.native_index).or_insert(StructId(i as u32));
            }
        }
        let actor_class = db.find("Actor");
        Vm {
            actor_class,
            db,
            code: HashMap::default(),
            native_decls,
            natives: natives::registry(),
            log: Vec::new(),
            depth: 0,
            steps: 0,
            rng: 0x1234_5678,
            short_circuit: [None, None],
            iterators: HashMap::default(),
            latent: HashMap::default(),
            state_pc: HashMap::default(),
            state_changed: Default::default(),
            in_end_state: Default::default(),
            disabled_events: HashMap::default(),
            state_sets: HashMap::default(),
            current_pc: 0,
            current_offset: 0,
            ext: None,
            last_state: HashMap::default(),
            native_cache: Vec::new(),
            script_overrides: HashMap::default(),
            override_cache: Vec::new(),
            trivial: HashMap::default(),
            profile: false,
            trace_states: false,
            profile_counts: HashMap::default(),
            profile_natives: HashMap::default(),
            func_info: Vec::new(),
            trace_calls: std::env::var("HP3_TRACE_CALLS").ok().map(|s| s.to_ascii_lowercase()),
            function_cache: HashMap::default(),
            lenient: false,
            missing: Default::default(),
        }
        .with_short_circuit()
        .with_states()
    }

    fn with_states(mut self) -> Self {
        crate::states::register(&mut self);
        self
    }

    fn with_short_circuit(mut self) -> Self {
        let find = |vm: &Self, name: &str| {
            vm.db.structs.iter().position(|s| {
                s.kind == StructKind::Function && vm.db.names.get(s.name).eq_ignore_ascii_case(name)
            })
        };
        self.short_circuit = [find(&self, "AndAnd_BoolBool"), find(&self, "OrOr_BoolBool")].map(|i| i.map(|i| StructId(i as u32)));
        self
    }

    pub fn warn(&mut self, s: String) {
        if self.log.len() < 200_000 {
            self.log.push(s);
        }
    }

    /// Whether a script function does nothing (an empty event body): only returns and no-ops.
    pub fn is_trivial(&mut self, f: StructId) -> bool {
        if let Some(&t) = self.trivial.get(&f) {
            return t;
        }
        let t = self.db.get(f).function_flags & FUNC_NATIVE == 0 && {
            let code = self.code(f);
            code.statements.iter().all(|s| {
                matches!(s, Expr::Nothing | Expr::ReturnNothing | Expr::EndParms)
                    || matches!(s, Expr::Return(v) if matches!(**v, Expr::Nothing))
            })
        };
        self.trivial.insert(f, t);
        t
    }

    /// Adds or replaces a native, by `Class.Function`.
    pub fn register(&mut self, path: &str, f: NativeFn) {
        self.native_cache.clear();
        self.natives.insert(path.to_ascii_lowercase(), f);
    }

    /// Replaces a script function (not a native) with a native that does the same, by `Class.Function`: for hot functions.
    pub fn register_override(&mut self, path: &str, f: NativeFn) {
        self.override_cache.clear();
        self.script_overrides.insert(path.to_ascii_lowercase(), f);
    }

    /// Adds an iterator native, by `Class.Function`.
    pub fn register_iterator(&mut self, path: &str, f: IterFn) {
        self.iterators.insert(path.to_ascii_lowercase(), f);
    }

    /// Whether a native function has an implementation.
    pub fn has_native(&self, f: StructId) -> bool {
        let p = self.function_path(f).to_ascii_lowercase();
        self.natives.contains_key(&p) || self.iterators.contains_key(&p)
    }

    pub fn code(&mut self, id: StructId) -> Rc<Code> {
        if let Some(c) = self.code.get(&id) {
            return c.clone();
        }
        let script = self.db.script_of(id);
        let code = Rc::new(Compiler::new(&mut self.db, id, &self.native_decls).compile(&script));
        self.code.insert(id, code.clone());
        code
    }

    /// `Class.Function` of a function struct: the outer class name and its own name.
    pub fn function_path(&self, f: StructId) -> String {
        let def = self.db.get(f);
        let outer = def.pkg.exports[def.export].outer;
        let class = def.pkg.ref_name(outer);
        format!("{class}.{}", self.db.names.get(def.name))
    }

    /// Whether an object exists for scripts: references to objects that were never loaded (such as actors
    /// deleted in the editor but still referenced by others) behave as `None`.
    pub fn is_loaded(&self, o: ObjId) -> bool {
        !matches!(self.db.objects.get(o).kind, ObjKind::Stub | ObjKind::Missing)
    }

    /// Loads a package object the first time scripts use it (meshes, materials, sounds, notifies...), with its
    /// tagged properties over its class defaults. Actors are not loaded here: they exist only once the world
    /// spawns them from its map, so a stub actor is one deleted in the editor. Returns whether it exists.
    pub fn ensure_loaded(&mut self, o: ObjId) -> bool {
        match self.db.objects.get(o).kind {
            ObjKind::Stub => {}
            ObjKind::Missing => return false,
            _ => return true,
        }
        let actor = self.actor_class;
        let loaded = self.db.objects.get(o).key.clone().and_then(|(pkg, index)| {
            let p = self.db.set.get(&pkg).ok()?;
            let class = self.db.class_of(&p, index)?;
            if actor.is_some_and(|a| self.is_child_of(class, a)) {
                return None;
            }
            let values = self.db.instance_values(&p, index)?;
            let outer = p.exports[index].outer;
            let outer = if outer.is_none() { None } else { self.db.resolve_obj(&p, outer) };
            Some((values, outer))
        });
        match loaded {
            Some(((class, values), outer)) => {
                let e = self.db.objects.get_mut(o);
                e.class = Some(class);
                e.kind = ObjKind::Instance { values, state: None };
                self.set_intrinsics(o, outer);
                true
            }
            None => {
                self.db.objects.get_mut(o).kind = ObjKind::Missing;
                false
            }
        }
    }

    /// The class of an object (the class itself for class objects).
    pub fn class_of(&self, obj: ObjId) -> Option<StructId> {
        let e = self.db.objects.get(obj);
        e.class.or(match e.kind {
            ObjKind::Struct(s) => Some(s),
            _ => None,
        })
    }

    pub fn is_child_of(&self, mut class: StructId, ancestor: StructId) -> bool {
        loop {
            if class == ancestor {
                return true;
            }
            match self.db.get(class).super_ {
                Some(s) => class = s,
                None => return false,
            }
        }
    }

    /// Finds a function by name for an object: its current state first, then its class chain.
    pub fn find_function(&mut self, obj: Option<ObjId>, class: Option<StructId>, name: Name) -> Option<StructId> {
        if let Some(o) = obj
            && let ObjKind::Instance { state: Some(st), .. } = &self.db.objects.get(o).kind
            && let Some(f) = self.lookup_in_chain(*st, name)
        {
            return Some(f);
        }
        let c = class.or_else(|| obj.and_then(|o| self.class_of(o)))?;
        self.lookup_in_chain(c, name)
    }

    /// A function declared in `start` or its super chain, cached per (struct, name).
    fn lookup_in_chain(&mut self, start: StructId, name: Name) -> Option<StructId> {
        if let Some(r) = self.function_cache.get(&(start, name)) {
            return *r;
        }
        let mut c = Some(start);
        let mut found = None;
        while let Some(cid) = c {
            if let Some(&f) = self.db.get(cid).functions.get(&name) {
                found = Some(f);
                break;
            }
            c = self.db.get(cid).super_;
        }
        self.function_cache.insert((start, name), found);
        found
    }

    /// Creates an instance of a class with its default values.
    pub fn new_object(&mut self, class: StructId, name: Name) -> ObjId {
        self.new_object_in(class, name, None)
    }

    /// Creates an instance of a class with its default values inside `outer`.
    pub fn new_object_in(&mut self, class: StructId, name: Name, outer: Option<ObjId>) -> ObjId {
        let values = self.db.get(class).defaults.clone();
        let id = self.db.objects.add(name, class, values);
        self.set_intrinsics(id, outer);
        id
    }

    /// Fills the properties every object has in script (`Outer`, `Name`, `Class`) from the object table.
    /// What a script reads from a class object (`SomePickupClass.Name`): a class has no property values of its own,
    /// only the name, outer and class every object has.
    fn class_object_property(&self, o: ObjId, slot: usize) -> Value {
        let ObjKind::Struct(sid) = self.db.objects.get(o).kind else { return Value::Empty };
        let Some(object) = self.db.find("Object") else { return Value::Empty };
        let slot_of = |name: &str| self.db.names.find(name).and_then(|n| self.db.get(object).prop(n)).map(|p| p.slot);
        if slot_of("Name") == Some(slot) {
            Value::Name(self.db.get(sid).name)
        } else {
            Value::Empty
        }
    }

    pub fn set_intrinsics(&mut self, obj: ObjId, outer: Option<ObjId>) {
        let Some(class) = self.db.objects.get(obj).class else { return };
        let name = self.db.objects.get(obj).name;
        let class_obj = self.db.get(class).object;
        for (prop, value) in [("Outer", Value::Object(outer)), ("Name", Value::Name(name)), ("Class", Value::Object(Some(class_obj)))] {
            let Some(n) = self.db.names.find(prop) else { continue };
            let Some(slot) = self.db.get(class).prop(n).map(|p| p.slot) else { continue };
            if let Some(v) = self.db.objects.values_mut(obj).and_then(|v| v.get_mut(slot)) {
                *v = value;
            }
        }
    }

    // ---- places ----

    fn root_vec<'a>(&'a mut self, root: &Root, frame: &'a mut Frame) -> Option<&'a mut Vec<Value>> {
        match root {
            Root::Local => Some(&mut frame.locals),
            Root::Instance(o) => self.db.objects.values_mut_quiet(*o),
            Root::Default(c) => Some(&mut self.db.structs[c.0 as usize].defaults),
            Root::Temp(_) => None,
        }
    }

    fn read(&mut self, p: &Place, frame: &mut Frame) -> Value {
        self.with_value(p, frame, Value::clone)
    }

    /// Calls `f` with the value at a place (`Value::Empty` when there is none), without copying it.
    fn with_value<R>(&mut self, p: &Place, frame: &mut Frame, f: impl FnOnce(&Value) -> R) -> R {
        let visit = f;
        let empty = Value::Empty;
        let (mut cur, steps): (&Value, &[Step]) = match &p.root {
            Root::Temp(v) => (v, &p.path[..]),
            root => {
                // Reading must not stamp the object as changed (`root_vec` is the writable view).
                let vals: &Vec<Value> = match root {
                    Root::Local => &frame.locals,
                    Root::Instance(o) => match self.db.objects.values(*o) {
                        Some(v) => v,
                        None => return visit(&empty),
                    },
                    Root::Default(c) => &self.db.structs[c.0 as usize].defaults,
                    Root::Temp(_) => return visit(&empty),
                };
                match p.path.first() {
                    Some(Step::Slot(s)) => match vals.get(*s) {
                        Some(v) => (v, &p.path[1..]),
                        None => return visit(&empty),
                    },
                    _ => return visit(&empty),
                }
            }
        };
        for step in steps {
            cur = match (step, cur) {
                (Step::Slot(s), Value::Struct(f)) => match f.get(*s) {
                    Some(v) => v,
                    None => return visit(&empty),
                },
                (Step::Index(i), Value::Array(a)) => match a.get(*i) {
                    Some(v) => v,
                    None => return visit(&empty),
                },
                _ => return visit(&empty),
            };
        }
        visit(cur)
    }

    fn write(&mut self, p: &Place, frame: &mut Frame, value: Value) {
        if let (Root::Instance(o), Some(Step::Slot(first))) = (&p.root, p.path.first())
            && *first < self.db.objects.quiet_from
        {
            self.db.objects.stamp(*o);
        }
        let Some(vals) = self.root_vec(&p.root, frame) else { return };
        let Some(Step::Slot(first)) = p.path.first() else { return };
        let Some(mut cur) = vals.get_mut(*first) else { return };
        for step in &p.path[1..] {
            // A member written into an element that was only made room for (a dynamic array grown by `Length`
            // holds `Empty` where the struct's type is not known) makes the struct.
            if let (Step::Slot(s), Value::Empty) = (step, &*cur) {
                *cur = Value::Struct(vec![Value::Empty; *s + 1].into());
            }
            cur = match (step, cur) {
                (Step::Slot(s), Value::Struct(f)) => match f.get_mut(*s) {
                    Some(v) => v,
                    None => return,
                },
                (Step::Index(i), Value::Array(a)) => {
                    if *i >= a.len() {
                        let fill = a.first().map(zero_like).unwrap_or(Value::Empty);
                        a.resize(*i + 1, fill);
                    }
                    &mut a[*i]
                }
                _ => return,
            };
        }
        *cur = value;
    }

    fn place(&mut self, e: &Expr, frame: &mut Frame, ctx: Option<ObjId>) -> VmResult<Place> {
        Ok(match e {
            Expr::Local(s) => Place { root: Root::Local, path: smallvec::smallvec![Step::Slot(*s)] },
            Expr::Instance(s) => match ctx.or(frame.this) {
                Some(o) => Place { root: Root::Instance(o), path: smallvec::smallvec![Step::Slot(*s)] },
                None => Place { root: Root::Temp(Value::Empty), path: smallvec::SmallVec::new() },
            },
            Expr::Default(s) => {
                let class = ctx.or(frame.this).and_then(|o| self.class_of(o));
                match class {
                    Some(c) => Place { root: Root::Default(c), path: smallvec::smallvec![Step::Slot(*s)] },
                    None => Place { root: Root::Temp(Value::Empty), path: smallvec::SmallVec::new() },
                }
            }
            Expr::ArrayElement(idx, arr) => {
                let i = self.eval(idx, frame, None)?.as_int().max(0) as usize;
                let mut p = self.place(arr, frame, ctx)?;
                if let Some(Step::Slot(s)) = p.path.last_mut() {
                    *s += i;
                }
                p
            }
            Expr::DynArrayElement(idx, arr) => {
                let i = self.eval(idx, frame, None)?.as_int().max(0) as usize;
                let mut p = self.place(arr, frame, ctx)?;
                p.path.push(Step::Index(i));
                p
            }
            Expr::StructMember(m, s) => {
                let mut p = self.place(s, frame, ctx)?;
                if let Root::Temp(v) = &p.root {
                    let member = v.fields().get(*m).cloned().unwrap_or_default();
                    return Ok(Place { root: Root::Temp(member), path: smallvec::SmallVec::new() });
                }
                p.path.push(Step::Slot(*m));
                p
            }
            // `class'X'.default.Prop` and `SomeClass.static...`: the class object is the context.
            Expr::ClassContext(class, inner) => match self.eval(class, frame, ctx)?.as_obj() {
                Some(c) => self.place(inner, frame, Some(c))?,
                None => Place { root: Root::Temp(Value::Empty), path: smallvec::SmallVec::new() },
            },
            Expr::Context(obj, inner) => {
                let o = self.eval(obj, frame, ctx)?.as_obj().filter(|&o| self.ensure_loaded(o));
                match o {
                    Some(o) => self.place(inner, frame, Some(o))?,
                    None => {
                        let at = self.function_path(frame.func);
                        self.warn(format!("{at}: accessed none (statement at {:#06x})", self.current_offset));
                        Place { root: Root::Temp(Value::Empty), path: smallvec::SmallVec::new() }
                    }
                }
            }
            other => Place { root: Root::Temp(self.eval(other, frame, ctx)?), path: smallvec::SmallVec::new() },
        })
    }

    // ---- evaluation ----

    pub fn eval(&mut self, e: &Expr, frame: &mut Frame, ctx: Option<ObjId>) -> VmResult<Value> {
        Ok(match e {
            Expr::Const(v) => v.clone(),
            Expr::SelfObj => Value::Object(frame.this),
            Expr::Nothing | Expr::EndParms => Value::Empty,
            // The common reads, without building a place.
            Expr::Local(s) => frame.locals.get(*s).cloned().unwrap_or_default(),
            Expr::Instance(s) => match ctx.or(frame.this) {
                Some(o) => match self.db.objects.values(o).and_then(|v| v.get(*s)) {
                    Some(v) => v.clone(),
                    None => self.class_object_property(o, *s),
                },
                None => Value::Empty,
            },
            Expr::Default(_)
            | Expr::ArrayElement(..)
            | Expr::DynArrayElement(..)
            | Expr::StructMember(..) => {
                let p = self.place(e, frame, ctx)?;
                self.read(&p, frame)
            }
            Expr::Context(obj, inner) => {
                let o = self.eval(obj, frame, ctx)?.as_obj().filter(|&o| self.ensure_loaded(o));
                match o {
                    Some(o) => self.eval(inner, frame, Some(o))?,
                    None => {
                        let at = self.function_path(frame.func);
                        self.warn(format!("{at}: accessed none (statement at {:#06x})", self.current_offset));
                        Value::Empty
                    }
                }
            }
            Expr::ClassContext(class, inner) => {
                let c = self.eval(class, frame, ctx)?.as_obj();
                match c {
                    Some(c) => self.eval(inner, frame, Some(c))?,
                    None => Value::Empty,
                }
            }
            // `Array.Length = n` grows (with empty elements) or cuts the array.
            Expr::Let(dst, src) if matches!(**dst, Expr::DynArrayLength(_)) => {
                let Expr::DynArrayLength(arr) = &**dst else { unreachable!() };
                let p = self.place(arr, frame, ctx)?;
                let n = self.eval(src, frame, ctx)?.as_int().max(0) as usize;
                if let Value::Array(mut a) = self.read(&p, frame) {
                    let fill = a.first().map(zero_like).unwrap_or(Value::Empty);
                    a.resize(n, fill);
                    self.write(&p, frame, Value::Array(a));
                }
                Value::Empty
            }
            Expr::Let(dst, src) => {
                let p = self.place(dst, frame, ctx)?;
                let v = self.eval(src, frame, ctx)?;
                self.write(&p, frame, v);
                Value::Empty
            }
            Expr::Discard(inner) => {
                self.eval(inner, frame, ctx)?;
                Value::Empty
            }
            Expr::DynArrayLength(arr) => {
                let p = self.place(arr, frame, ctx)?;
                Value::Int(self.with_value(&p, frame, |v| match v {
                    Value::Array(a) => a.len() as i32,
                    _ => 0,
                }))
            }
            Expr::DynArrayInsert(arr, at, count) | Expr::DynArrayRemove(arr, at, count) => {
                let p = self.place(arr, frame, ctx)?;
                let at = self.eval(at, frame, ctx)?.as_int().max(0) as usize;
                let n = self.eval(count, frame, ctx)?.as_int().max(0) as usize;
                if let Value::Array(mut a) = self.read(&p, frame) {
                    if matches!(e, Expr::DynArrayInsert(..)) {
                        let fill = a.first().map(zero_like).unwrap_or(Value::Empty);
                        let at = at.min(a.len());
                        a.splice(at..at, std::iter::repeat_n(fill, n));
                    } else if at < a.len() {
                        let end = (at + n).min(a.len());
                        a.drain(at..end);
                    }
                    self.write(&p, frame, Value::Array(a));
                }
                Value::Empty
            }
            Expr::StructCmp(eq, a, b) => {
                let (a, b) = (self.eval(a, frame, ctx)?, self.eval(b, frame, ctx)?);
                Value::Bool((a == b) == *eq)
            }
            Expr::Cast(code, inner) => {
                let v = self.eval(inner, frame, ctx)?;
                natives::primitive_cast(self, *code, v)?
            }
            Expr::DynamicCast(class, inner) | Expr::MetaCast(class, inner) => {
                let v = self.eval(inner, frame, ctx)?;
                if let Some(o) = v.as_obj() {
                    self.ensure_loaded(o);
                }
                let ok = match (v.as_obj(), class) {
                    (Some(o), Some(c)) => {
                        let target = if matches!(e, Expr::MetaCast(..)) {
                            match self.db.objects.get(o).kind {
                                ObjKind::Struct(s) => Some(s),
                                _ => None,
                            }
                        } else {
                            self.db.objects.get(o).class
                        };
                        target.is_some_and(|t| self.is_child_of(t, *c))
                    }
                    _ => false,
                };
                Value::Object(if ok { v.as_obj() } else { None })
            }
            Expr::Virtual(name, args) | Expr::Global(name, args) => {
                let this = ctx.or(frame.this);
                let class = if matches!(e, Expr::Global(..)) {
                    // `global.` skips states.
                    this.and_then(|o| self.class_of(o))
                } else {
                    None
                };
                let lookup_obj = if matches!(e, Expr::Global(..)) { None } else { this };
                match self.find_function(lookup_obj, class.or_else(|| this.and_then(|o| self.class_of(o))), *name) {
                    Some(f) => self.call(f, this, args, frame, ctx)?,
                    None => {
                        return Err(VmError::Runtime(format!("no function {} for this object", self.db.names.get(*name))));
                    }
                }
            }
            Expr::Final(f, args) => {
                let this = ctx.or(frame.this);
                self.call(*f, this, args, frame, ctx)?
            }
            Expr::Native(index, decl, args) => {
                let this = ctx.or(frame.this);
                match decl {
                    Some(f) => self.call(*f, this, args, frame, ctx)?,
                    None => return Err(VmError::Unimplemented(format!("native index {index} has no declaration"))),
                }
            }
            Expr::DelegateValue(name) => Value::Delegate(ctx.or(frame.this), *name),
            Expr::DelegateCall(var, fallback, args) => {
                let (target, name) = match self.eval(var, frame, ctx)? {
                    Value::Delegate(o, n) if n != Name::NONE => (o, n),
                    _ => (ctx.or(frame.this), *fallback),
                };
                match self.find_function(target, None, name) {
                    Some(f) => self.call(f, target, args, frame, ctx)?,
                    None => Value::Empty,
                }
            }
            Expr::New(args) => {
                // new(Outer, Name, Flags) Class: an instance of a non-actor class with its defaults.
                let class = match args.get(3) {
                    Some(c) => self.eval(c, frame, ctx)?.as_obj(),
                    None => None,
                };
                let name = match args.get(1) {
                    Some(Expr::Nothing) | None => Value::Empty,
                    Some(n) => self.eval(n, frame, ctx)?,
                };
                let outer = match args.first() {
                    Some(Expr::Nothing) | None => None,
                    Some(o) => self.eval(o, frame, ctx)?.as_obj(),
                };
                match class.and_then(|c| self.class_of(c)) {
                    Some(c) => {
                        let n = match name {
                            Value::Str(s) => self.db.names.intern(&s),
                            Value::Name(n) => n,
                            _ => self.db.get(c).name,
                        };
                        Value::Object(Some(self.new_object_in(c, n, outer)))
                    }
                    None => Value::Object(None),
                }
            }
            Expr::Unsupported(op) => return Err(VmError::Unimplemented(format!("opcode {op:#04x}"))),
            other => return Err(VmError::Runtime(format!("{other:?} used as an expression"))),
        })
    }

    fn func_info(&mut self, f: StructId) -> Rc<FuncInfo> {
        let i = f.0 as usize;
        if let Some(Some(info)) = self.func_info.get(i) {
            return info.clone();
        }
        let def = self.db.get(f);
        let info = Rc::new(FuncInfo {
            params: def
                .props
                .iter()
                .filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN_PARM == 0)
                .map(|p| (p.slot, p.flags, p.ty.clone()))
                .collect(),
            ret: def.props.iter().find(|p| p.flags & CPF_RETURN_PARM != 0).map(|p| p.slot),
            native: def.function_flags & FUNC_NATIVE != 0,
            is_state: def.kind == StructKind::State,
        });
        if self.func_info.len() <= i {
            self.func_info.resize(i + 1, None);
        }
        self.func_info[i] = Some(info.clone());
        info
    }

    /// Calls function `f` on `this` with argument expressions evaluated in `frame`.
    fn call(&mut self, f: StructId, this: Option<ObjId>, args: &[Expr], frame: &mut Frame, ctx: Option<ObjId>) -> VmResult<Value> {
        if let Some(i) = self.short_circuit.iter().position(|s| *s == Some(f))
            && args.len() >= 2
        {
            let left = self.eval(&args[0], frame, None)?.as_bool();
            // `&&` stops on false, `||` on true.
            if left == (i == 1) {
                return Ok(Value::Bool(left));
            }
            return Ok(Value::Bool(self.eval(&args[1], frame, None)?.as_bool()));
        }
        let info = self.func_info(f);
        let native = info.native;
        let mut values = Vec::with_capacity(info.params.len());
        let mut out_places = Vec::new();
        for (i, (_slot, flags, ty)) in info.params.iter().enumerate() {
            let arg = args.get(i);
            let v = match arg {
                None | Some(Expr::Nothing) => None,
                Some(a) if flags & CPF_OUT_PARM != 0 => {
                    let p = self.place(a, frame, None)?;
                    let v = self.read(&p, frame);
                    out_places.push((i, p));
                    Some(v)
                }
                Some(a) => Some(self.eval(a, frame, None)?),
            };
            // Natives see an omitted optional argument as empty; script functions get the type's zero.
            let v = match v {
                Some(v) => v,
                None if native => Value::Empty,
                None => self.db.default_value(ty),
            };
            values.push(v);
        }
        let _ = ctx;
        let result = self.invoke(f, this, &mut values)?;
        for (i, p) in out_places {
            let v = std::mem::take(&mut values[i]);
            self.write(&p, frame, v);
        }
        Ok(result)
    }

    /// Runs function `f` with already evaluated arguments; out arguments are updated in place.
    pub fn invoke(&mut self, f: StructId, this: Option<ObjId>, args: &mut [Value]) -> VmResult<Value> {
        if self.depth > 250 {
            return Err(VmError::Recursion);
        }
        if let Some(filter) = &self.trace_calls {
            let path = self.function_path(f);
            if path.to_ascii_lowercase().contains(filter.as_str()) {
                let who = this.map(|o| self.db.names.get(self.db.objects.get(o).name).to_string()).unwrap_or_default();
                let shown: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        Value::Name(n) => format!("'{}'", self.db.names.get(*n)),
                        Value::Object(Some(o)) => self.db.names.get(self.db.objects.get(*o).name).to_string(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                eprintln!("call: {who}.{path}({})", shown.join(", "));
            }
        }
        let def = self.db.get(f);
        if def.function_flags & FUNC_NATIVE != 0 {
            let native = match self.native_cache.get(f.0 as usize) {
                Some(Some(n)) => *n,
                _ => {
                    let key = self.function_path(f).to_ascii_lowercase();
                    let n = self.natives.get(&key).copied();
                    let i = f.0 as usize;
                    if self.native_cache.len() <= i {
                        self.native_cache.resize(i + 1, None);
                    }
                    self.native_cache[i] = Some(n);
                    n
                }
            };
            let Some(native) = native else {
                let path = self.function_path(f);
                if !self.lenient {
                    return Err(VmError::Unimplemented(path));
                }
                // Counted and skipped: the caller gets the declared return type's zero.
                *self.missing.entry(path).or_default() += 1;
                let ret = def.props.iter().find(|p| p.flags & CPF_RETURN_PARM != 0).map(|p| p.ty.clone());
                return Ok(ret.map(|t| self.db.default_value(&t)).unwrap_or(Value::Empty));
            };
            if self.profile {
                let t = std::time::Instant::now();
                let r = native(self, this, args);
                let e = self.profile_natives.entry(f).or_default();
                e.0 += 1;
                e.1 += t.elapsed();
                return r;
            }
            return native(self, this, args);
        }
        if !self.script_overrides.is_empty() {
            let replaced = match self.override_cache.get(f.0 as usize) {
                Some(Some(n)) => *n,
                _ => {
                    let key = self.function_path(f).to_ascii_lowercase();
                    let n = self.script_overrides.get(&key).copied();
                    let i = f.0 as usize;
                    if self.override_cache.len() <= i {
                        self.override_cache.resize(i + 1, None);
                    }
                    self.override_cache[i] = Some(n);
                    n
                }
            };
            if let Some(native) = replaced {
                return native(self, this, args);
            }
        }
        let mut frame = Frame::new(f, this, def.defaults.clone());
        let info = self.func_info(f);
        let ret = info.ret;
        for (i, (slot, _, _)) in info.params.iter().enumerate() {
            if let Some(v) = args.get_mut(i) {
                frame.locals[*slot] = coerce(&frame.locals[*slot], std::mem::take(v));
            }
        }
        let code = self.code(f);
        self.depth += 1;
        let r = self.run(&code, &mut frame, 0);
        self.depth -= 1;
        r?;
        // Out parameters are written back; arguments were moved into the frame, so every one is restored.
        for (i, (slot, _, _)) in info.params.iter().enumerate() {
            if let Some(a) = args.get_mut(i) {
                *a = std::mem::take(&mut frame.locals[*slot]);
            }
        }
        Ok(ret.map(|s| std::mem::take(&mut frame.locals[s])).unwrap_or(Value::Empty))
    }

    pub(crate) fn run_state_code(&mut self, code: &Code, frame: &mut Frame, start: usize) -> VmResult<RunEnd> {
        self.depth += 1;
        let r = self.run(code, frame, start);
        self.depth -= 1;
        r
    }

    fn run(&mut self, code: &Code, frame: &mut Frame, start: usize) -> VmResult<RunEnd> {
        let mut pc = start;
        let info = self.func_info(frame.func);
        let ret_slot = info.ret;
        let state_obj = frame.this.filter(|_| info.is_state);
        let mut budget: u64 = 10_000_000;
        while pc < code.statements.len() {
            self.steps += 1;
            budget -= 1;
            if budget == 0 {
                return Err(VmError::Runtime(format!("runaway loop in {}", self.function_path(frame.func))));
            }
            self.current_pc = pc;
            self.current_offset = code.offsets.get(pc).copied().unwrap_or(0);
            if self.profile {
                *self.profile_counts.entry(frame.func).or_default() += 1;
            }
            let flow = self.exec(&code.statements[pc], code, frame, ret_slot)?;
            // State code stops when the object leaves the state or a latent action starts.
            if let Some(o) = state_obj {
                if self.state_changed.remove(&o) {
                    return Ok(RunEnd::Finished);
                }
                if self.latent.contains_key(&o) {
                    let next = match flow {
                        Flow::Jump(t) => t,
                        _ => pc + 1,
                    };
                    return Ok(RunEnd::Suspended(next));
                }
            }
            match flow {
                Flow::Next => pc += 1,
                Flow::Jump(t) => pc = t,
                Flow::Return | Flow::Stop => return Ok(RunEnd::Finished),
            }
        }
        Ok(RunEnd::Finished)
    }

    fn exec(&mut self, s: &Expr, code: &Code, frame: &mut Frame, ret_slot: Option<usize>) -> VmResult<Flow> {
        Ok(match s {
            Expr::Jump(t) => Flow::Jump(*t),
            Expr::JumpIfNot(t, cond) => {
                if self.eval(cond, frame, None)?.as_bool() {
                    Flow::Next
                } else {
                    Flow::Jump(*t)
                }
            }
            Expr::Return(v) => {
                let v = self.eval(v, frame, None)?;
                if let Some(slot) = ret_slot
                    && v != Value::Empty
                {
                    frame.locals[slot] = v;
                }
                Flow::Return
            }
            Expr::ReturnNothing => Flow::Return,
            Expr::Stop => Flow::Stop,
            Expr::Switch(v) => {
                frame.switch_value = Some(self.eval(v, frame, None)?);
                Flow::Next
            }
            Expr::Case(next, value) => match value {
                None => Flow::Next,
                Some(v) => {
                    let v = self.eval(v, frame, None)?;
                    let cur = frame.switch_value.clone().unwrap_or_default();
                    if values_equal(&cur, &v) { Flow::Next } else { Flow::Jump(*next) }
                }
            },
            Expr::LabelTable(_) | Expr::Nothing | Expr::EndParms => Flow::Next,
            Expr::Assert(line, cond) => {
                if !self.eval(cond, frame, None)?.as_bool() {
                    let at = self.function_path(frame.func);
                    self.warn(format!("{at}: assertion failed at line {line}"));
                }
                Flow::Next
            }
            Expr::GotoLabel(label) => {
                let name = self.eval(label, frame, None)?.as_name();
                match code.labels.get(&name) {
                    Some(&i) => Flow::Jump(i),
                    None => {
                        let at = self.function_path(frame.func);
                        let label = self.db.names.get(name).to_string();
                        self.warn(format!("{at}: no label {label}"));
                        Flow::Stop
                    }
                }
            }
            Expr::Iterator(call, end) => {
                // The loop body starts after this statement. Taken before the arguments are evaluated: they
                // may call script functions (`Vec(0,0,0)`), which move `current_pc`.
                let body = self.current_pc + 1;
                // `foreach Obj.Iter(...)` runs the iterator with that object as self.
                let (iter_this, call) = match &**call {
                    Expr::Context(obj, inner) => (self.eval(obj, frame, None)?.as_obj(), &**inner),
                    other => (frame.this, other),
                };
                let (f, args) = match call {
                    Expr::Native(_, Some(f), args) | Expr::Final(f, args) => (*f, args),
                    _ => return Err(VmError::Runtime("iterator over a non-function".into())),
                };
                let path = self.function_path(f).to_ascii_lowercase();
                let Some(iter) = self.iterators.get(&path).copied() else {
                    return Err(VmError::Unimplemented(self.function_path(f)));
                };
                let mut values = Vec::new();
                let mut places = Vec::new();
                for a in args {
                    match a {
                        Expr::Nothing => values.push(Value::Empty),
                        Expr::Local(_) | Expr::Instance(_) | Expr::StructMember(..) | Expr::ArrayElement(..) | Expr::DynArrayElement(..) => {
                            let p = self.place(a, frame, None)?;
                            values.push(self.read(&p, frame));
                            places.push((values.len() - 1, p));
                        }
                        other => values.push(self.eval(other, frame, None)?),
                    }
                }
                let items = if self.profile {
                    let t = std::time::Instant::now();
                    let r = iter(self, iter_this, &values)?;
                    let e = self.profile_natives.entry(f).or_default();
                    e.0 += 1;
                    e.1 += t.elapsed();
                    r
                } else {
                    iter(self, iter_this, &values)?
                };
                if items.is_empty() {
                    // An actor iterator that found nothing leaves its variable empty.
                    self.clear_iter_objects(&places, &values, frame);
                    return Ok(Flow::Jump(*end));
                }
                let state = IterState { items, index: 0, places, body };
                self.write_iter(&state, frame);
                frame.iters.push(state);
                Flow::Next
            }
            Expr::IteratorNext => {
                let Some(mut state) = frame.iters.pop() else { return Ok(Flow::Next) };
                state.index += 1;
                let more = state.index < state.items.len();
                if more {
                    self.write_iter(&state, frame);
                }
                let body = state.body;
                if !more {
                    // Run to the end, an actor iterator leaves its variable empty (a loop left by `break` keeps it).
                    let item = state.items.last().cloned().unwrap_or_default();
                    self.clear_iter_objects(&state.places, &item, frame);
                }
                // Kept on the stack either way: the pop after the loop removes it.
                frame.iters.push(state);
                if more { Flow::Jump(body) } else { Flow::Next }
            }
            Expr::IteratorPop => {
                frame.iters.pop();
                Flow::Next
            }
            other => {
                self.eval(other, frame, None)?;
                Flow::Next
            }
        })
    }

    /// Sets the object variables an iterator filled to none.
    fn clear_iter_objects(&mut self, places: &[(usize, Place)], values: &[Value], frame: &mut Frame) {
        for (i, p) in places {
            if matches!(values.get(*i), Some(Value::Object(_))) {
                self.write(p, frame, Value::Object(None));
            }
        }
    }

    fn write_iter(&mut self, state: &IterState, frame: &mut Frame) {
        let item = &state.items[state.index];
        for (i, p) in &state.places {
            if let Some(v) = item.get(*i) {
                self.write(p, frame, v.clone());
            }
        }
    }
}

/// Converts a number to the kind of number `like` holds; other values pass through.
fn coerce(like: &Value, v: Value) -> Value {
    match (like, &v) {
        (Value::Float(_), Value::Int(_) | Value::Byte(_)) => Value::Float(v.as_float()),
        (Value::Int(_), Value::Float(_) | Value::Byte(_) | Value::Bool(_)) => Value::Int(v.as_int()),
        (Value::Byte(_), Value::Int(_) | Value::Float(_)) => Value::Byte(v.as_int() as u8),
        (Value::Bool(_), Value::Int(_) | Value::Byte(_)) => Value::Bool(v.as_bool()),
        _ => v,
    }
}

/// A zero value of the same shape, for growing arrays.
fn zero_like(v: &Value) -> Value {
    match v {
        Value::Byte(_) => Value::Byte(0),
        Value::Int(_) => Value::Int(0),
        Value::Bool(_) => Value::Bool(false),
        Value::Float(_) => Value::Float(0.0),
        Value::Name(_) => Value::Name(Name::NONE),
        Value::Str(_) => Value::Str(String::new()),
        Value::Object(_) => Value::Object(None),
        Value::Struct(f) => Value::Struct(f.iter().map(zero_like).collect()),
        Value::Array(_) => Value::Array(Vec::new()),
        Value::Delegate(..) => Value::Delegate(None, Name::NONE),
        Value::Empty => Value::Empty,
    }
}

/// Equality for `switch`: numbers compare by value across byte/int.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Byte(_) | Value::Int(_), Value::Byte(_) | Value::Int(_)) => a.as_int() == b.as_int(),
        (Value::Str(x), Value::Str(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    }
}
