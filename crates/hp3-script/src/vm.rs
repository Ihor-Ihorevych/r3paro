//! The interpreter: runs compiled functions against the object table.

use std::collections::HashMap;
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

pub type VmResult<T> = Result<T, VmError>;

pub struct Frame {
    pub func: StructId,
    pub this: Option<ObjId>,
    pub locals: Vec<Value>,
    switch_value: Option<Value>,
}

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
    path: Vec<Step>,
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
}

impl<'s> Vm<'s> {
    /// Builds a VM over a class database whose classes are already loaded.
    pub fn new(db: ClassDb<'s>) -> Self {
        let mut native_decls = HashMap::new();
        for (i, s) in db.structs.iter().enumerate() {
            if s.kind == StructKind::Function && s.function_flags & FUNC_NATIVE != 0 && s.native_index != 0 {
                native_decls.entry(s.native_index).or_insert(StructId(i as u32));
            }
        }
        Vm {
            db,
            code: HashMap::new(),
            native_decls,
            natives: natives::registry(),
            log: Vec::new(),
            depth: 0,
            steps: 0,
            rng: 0x1234_5678,
            short_circuit: [None, None],
        }
        .with_short_circuit()
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
        if self.log.len() < 10_000 {
            self.log.push(s);
        }
    }

    /// Whether a native function has an implementation.
    pub fn has_native(&self, f: StructId) -> bool {
        self.natives.contains_key(&self.function_path(f).to_ascii_lowercase())
    }

    pub fn code(&mut self, id: StructId) -> Rc<Code> {
        if let Some(c) = self.code.get(&id) {
            return c.clone();
        }
        let script = self.db.get(id).script.clone();
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
    pub fn find_function(&self, obj: Option<ObjId>, class: Option<StructId>, name: Name) -> Option<StructId> {
        if let Some(o) = obj
            && let ObjKind::Instance { state: Some(st), .. } = &self.db.objects.get(o).kind
        {
            let mut s = Some(*st);
            while let Some(sid) = s {
                if let Some(&f) = self.db.get(sid).functions.get(&name) {
                    return Some(f);
                }
                s = self.db.get(sid).super_;
            }
        }
        let mut c = class.or_else(|| obj.and_then(|o| self.class_of(o)));
        while let Some(cid) = c {
            if let Some(&f) = self.db.get(cid).functions.get(&name) {
                return Some(f);
            }
            c = self.db.get(cid).super_;
        }
        None
    }

    /// Creates an instance of a class with its default values.
    pub fn new_object(&mut self, class: StructId, name: Name) -> ObjId {
        let values = self.db.get(class).defaults.clone();
        self.db.objects.add(name, class, values)
    }

    // ---- places ----

    fn root_vec<'a>(&'a mut self, root: &Root, frame: &'a mut Frame) -> Option<&'a mut Vec<Value>> {
        match root {
            Root::Local => Some(&mut frame.locals),
            Root::Instance(o) => self.db.objects.values_mut(*o),
            Root::Default(c) => Some(&mut self.db.structs[c.0 as usize].defaults),
            Root::Temp(_) => None,
        }
    }

    fn read(&mut self, p: &Place, frame: &mut Frame) -> Value {
        let (mut cur, steps): (&Value, &[Step]) = match &p.root {
            Root::Temp(v) => (v, &p.path[..]),
            root => {
                let Some(vals) = self.root_vec(root, frame) else { return Value::Empty };
                match p.path.first() {
                    Some(Step::Slot(s)) => match vals.get(*s) {
                        Some(v) => (v, &p.path[1..]),
                        None => return Value::Empty,
                    },
                    _ => return Value::Empty,
                }
            }
        };
        for step in steps {
            cur = match (step, cur) {
                (Step::Slot(s), Value::Struct(f)) => match f.get(*s) {
                    Some(v) => v,
                    None => return Value::Empty,
                },
                (Step::Index(i), Value::Array(a)) => match a.get(*i) {
                    Some(v) => v,
                    None => return Value::Empty,
                },
                _ => return Value::Empty,
            };
        }
        cur.clone()
    }

    fn write(&mut self, p: &Place, frame: &mut Frame, value: Value) {
        let Some(vals) = self.root_vec(&p.root, frame) else { return };
        let Some(Step::Slot(first)) = p.path.first() else { return };
        let Some(mut cur) = vals.get_mut(*first) else { return };
        for step in &p.path[1..] {
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
            Expr::Local(s) => Place { root: Root::Local, path: vec![Step::Slot(*s)] },
            Expr::Instance(s) => match ctx.or(frame.this) {
                Some(o) => Place { root: Root::Instance(o), path: vec![Step::Slot(*s)] },
                None => Place { root: Root::Temp(Value::Empty), path: vec![] },
            },
            Expr::Default(s) => {
                let class = ctx.or(frame.this).and_then(|o| self.class_of(o));
                match class {
                    Some(c) => Place { root: Root::Default(c), path: vec![Step::Slot(*s)] },
                    None => Place { root: Root::Temp(Value::Empty), path: vec![] },
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
                    return Ok(Place { root: Root::Temp(member), path: vec![] });
                }
                p.path.push(Step::Slot(*m));
                p
            }
            Expr::Context(obj, inner) => {
                let o = self.eval(obj, frame, ctx)?.as_obj();
                match o {
                    Some(o) => self.place(inner, frame, Some(o))?,
                    None => {
                        let at = self.function_path(frame.func);
                        self.warn(format!("{at}: accessed none"));
                        Place { root: Root::Temp(Value::Empty), path: vec![] }
                    }
                }
            }
            other => Place { root: Root::Temp(self.eval(other, frame, ctx)?), path: vec![] },
        })
    }

    // ---- evaluation ----

    pub fn eval(&mut self, e: &Expr, frame: &mut Frame, ctx: Option<ObjId>) -> VmResult<Value> {
        Ok(match e {
            Expr::Const(v) => v.clone(),
            Expr::SelfObj => Value::Object(frame.this),
            Expr::Nothing | Expr::EndParms => Value::Empty,
            Expr::Local(_)
            | Expr::Instance(_)
            | Expr::Default(_)
            | Expr::ArrayElement(..)
            | Expr::DynArrayElement(..)
            | Expr::StructMember(..) => {
                let p = self.place(e, frame, ctx)?;
                self.read(&p, frame)
            }
            Expr::Context(obj, inner) => {
                let o = self.eval(obj, frame, ctx)?.as_obj();
                match o {
                    Some(o) => self.eval(inner, frame, Some(o))?,
                    None => {
                        let at = self.function_path(frame.func);
                        self.warn(format!("{at}: accessed none"));
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
            Expr::DynArrayLength(arr) => match self.eval(arr, frame, ctx)? {
                Value::Array(a) => Value::Int(a.len() as i32),
                _ => Value::Int(0),
            },
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
            Expr::New(_) => return Err(VmError::Unimplemented("new".into())),
            Expr::Unsupported(op) => return Err(VmError::Unimplemented(format!("opcode {op:#04x}"))),
            other => return Err(VmError::Runtime(format!("{other:?} used as an expression"))),
        })
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
        let params: Vec<(usize, u32, PropType)> = self
            .db
            .get(f)
            .props
            .iter()
            .filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN_PARM == 0)
            .map(|p| (p.slot, p.flags, p.ty.clone()))
            .collect();
        let native = self.db.get(f).function_flags & FUNC_NATIVE != 0;
        let mut values = Vec::with_capacity(params.len());
        let mut out_places = Vec::new();
        for (i, (slot, flags, ty)) in params.iter().enumerate() {
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
            values.push((*slot, v));
        }
        let _ = ctx;
        let mut arg_values: Vec<Value> = values.iter().map(|(_, v)| v.clone()).collect();
        let result = self.invoke(f, this, &mut arg_values)?;
        for (i, p) in out_places {
            self.write(&p, frame, arg_values[i].clone());
        }
        Ok(result)
    }

    /// Runs function `f` with already evaluated arguments; out arguments are updated in place.
    pub fn invoke(&mut self, f: StructId, this: Option<ObjId>, args: &mut [Value]) -> VmResult<Value> {
        if self.depth > 250 {
            return Err(VmError::Recursion);
        }
        let def = self.db.get(f);
        if def.function_flags & FUNC_NATIVE != 0 {
            let path = self.function_path(f);
            let key = path.to_ascii_lowercase();
            let Some(native) = self.natives.get(&key).copied() else {
                return Err(VmError::Unimplemented(path));
            };
            return native(self, this, args);
        }
        let params: Vec<usize> = def
            .props
            .iter()
            .filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN_PARM == 0)
            .map(|p| p.slot)
            .collect();
        let ret = def.props.iter().find(|p| p.flags & CPF_RETURN_PARM != 0).map(|p| p.slot);
        let mut frame = Frame { func: f, this, locals: def.defaults.clone(), switch_value: None };
        for (i, slot) in params.iter().enumerate() {
            if let Some(v) = args.get(i) {
                frame.locals[*slot] = coerce(&frame.locals[*slot], v.clone());
            }
        }
        let code = self.code(f);
        self.depth += 1;
        let r = self.run(&code, &mut frame, 0);
        self.depth -= 1;
        r?;
        for (i, slot) in params.iter().enumerate() {
            if let Some(a) = args.get_mut(i) {
                *a = frame.locals[*slot].clone();
            }
        }
        Ok(ret.map(|s| frame.locals[s].clone()).unwrap_or(Value::Empty))
    }

    fn run(&mut self, code: &Code, frame: &mut Frame, start: usize) -> VmResult<()> {
        let mut pc = start;
        let ret_slot = self.db.get(frame.func).props.iter().find(|p| p.flags & CPF_RETURN_PARM != 0).map(|p| p.slot);
        while pc < code.statements.len() {
            self.steps += 1;
            if self.steps % 10_000_000 == 0 && self.depth > 0 {
                // A single call running this long is a runaway loop.
                return Err(VmError::Runtime("runaway loop".into()));
            }
            match self.exec(&code.statements[pc], frame, ret_slot)? {
                Flow::Next => pc += 1,
                Flow::Jump(t) => pc = t,
                Flow::Return | Flow::Stop => return Ok(()),
            }
        }
        Ok(())
    }

    fn exec(&mut self, s: &Expr, frame: &mut Frame, ret_slot: Option<usize>) -> VmResult<Flow> {
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
            Expr::Iterator(..) | Expr::IteratorNext | Expr::IteratorPop => {
                return Err(VmError::Unimplemented("iterators".into()));
            }
            Expr::GotoLabel(_) => return Err(VmError::Unimplemented("goto label".into())),
            other => {
                self.eval(other, frame, None)?;
                Flow::Next
            }
        })
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
