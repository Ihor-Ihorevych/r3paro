//! Turns a struct's bytecode into an expression tree with every reference resolved: variable slots, functions,
//! natives and jump targets as statement indices.

use crate::fx::FxHashMap as HashMap;
use std::rc::Rc;

use hp3_package::bytecode::{Operand, Token, op};
use hp3_package::ObjRef;

use crate::class_db::{ClassDb, StructId};
use crate::names::Name;
use crate::value::{ObjId, Value};

#[derive(Clone, Debug)]
pub enum Expr {
    Local(usize),
    Instance(usize),
    Default(usize),
    Const(Value),
    SelfObj,
    Nothing,
    EndParms,
    Let(Box<Expr>, Box<Expr>),
    Jump(usize),
    JumpIfNot(usize, Box<Expr>),
    Return(Box<Expr>),
    ReturnNothing,
    Stop,
    Switch(Box<Expr>),
    /// Statement to go to when the value differs, and the value (none for `default:`).
    Case(usize, Option<Box<Expr>>),
    GotoLabel(Box<Expr>),
    LabelTable(Vec<(Name, u32)>),
    Assert(u16, Box<Expr>),
    Discard(Box<Expr>),
    /// Object expression, then the expression evaluated with that object as `self`.
    Context(Box<Expr>, Box<Expr>),
    /// Class expression, then the expression evaluated against that class's defaults or statics.
    ClassContext(Box<Expr>, Box<Expr>),
    ArrayElement(Box<Expr>, Box<Expr>),
    DynArrayElement(Box<Expr>, Box<Expr>),
    DynArrayLength(Box<Expr>),
    DynArrayInsert(Box<Expr>, Box<Expr>, Box<Expr>),
    DynArrayRemove(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Member slot inside the struct, struct expression.
    StructMember(usize, Box<Expr>),
    StructCmp(bool, Box<Expr>, Box<Expr>),
    Cast(u8, Box<Expr>),
    DynamicCast(Option<StructId>, Box<Expr>),
    MetaCast(Option<StructId>, Box<Expr>),
    New(Vec<Expr>),
    Virtual(Name, Vec<Expr>),
    Global(Name, Vec<Expr>),
    Final(StructId, Vec<Expr>),
    /// Native call by its fixed index, with the function that declares it.
    Native(u16, Option<StructId>, Vec<Expr>),
    /// Call through a delegate variable; the name is the function used when the delegate is unset.
    DelegateCall(Box<Expr>, Name, Vec<Expr>),
    /// A function of `self` taken as a delegate value.
    DelegateValue(Name),
    Iterator(Box<Expr>, usize),
    IteratorNext,
    IteratorPop,
    Unsupported(u8),
}

pub struct Code {
    pub statements: Vec<Expr>,
    /// Loaded-script offset of each statement.
    pub offsets: Vec<u32>,
    pub by_offset: HashMap<u32, usize>,
    pub labels: HashMap<Name, usize>,
}

pub struct Compiler<'a, 'd> {
    pub db: &'a mut ClassDb<'d>,
    pub owner: StructId,
    /// Native index to the function declaring it.
    pub native_decls: &'a HashMap<u16, StructId>,
    pending_jumps: Vec<u32>,
}

impl<'a, 'd> Compiler<'a, 'd> {
    pub fn new(db: &'a mut ClassDb<'d>, owner: StructId, native_decls: &'a HashMap<u16, StructId>) -> Self {
        Compiler { db, owner, native_decls, pending_jumps: Vec::new() }
    }

    fn pkg(&self) -> Rc<hp3_package::Package> {
        self.db.get(self.owner).pkg.clone()
    }

    /// Slot of a property referenced from bytecode, looked up in the struct that declares it.
    fn prop_slot(&mut self, r: ObjRef) -> Option<usize> {
        let pkg = self.pkg();
        let (pp, pi) = self.db.set.resolve(&pkg, r)?;
        let outer = pp.exports[pi].outer;
        let (op_, oi) = self.db.set.resolve(&pp, outer)?;
        let owner = self.db.load_struct(&op_, oi)?;
        let name = self.db.names.intern(pp.name(pp.exports[pi].name));
        self.db.get(owner).prop(name).map(|p| p.slot)
    }

    /// A variable reference as a local or instance access, depending on where it is declared.
    fn var_expr(&mut self, r: ObjRef) -> Option<Expr> {
        let pkg = self.pkg();
        let (pp, pi) = self.db.set.resolve(&pkg, r)?;
        let outer = pp.exports[pi].outer;
        let (op_, oi) = self.db.set.resolve(&pp, outer)?;
        let owner = self.db.load_struct(&op_, oi)?;
        let name = self.db.names.intern(pp.name(pp.exports[pi].name));
        let slot = self.db.get(owner).prop(name)?.slot;
        Some(if self.db.get(owner).kind == crate::class_db::StructKind::Function { Expr::Local(slot) } else { Expr::Instance(slot) })
    }

    fn struct_ref(&mut self, r: ObjRef) -> Option<StructId> {
        let pkg = self.pkg();
        match self.db.set.resolve(&pkg, r) {
            Some((p, i)) => self.db.load_struct(&p, i),
            None if !r.is_none() => self.db.native_class(pkg.ref_name(r)),
            None => None,
        }
    }

    fn obj(&mut self, r: ObjRef) -> Option<ObjId> {
        let pkg = self.pkg();
        self.db.resolve_obj(&pkg, r)
    }

    fn name(&mut self, n: usize) -> Name {
        let pkg = self.pkg();
        self.db.names.intern(pkg.name(n))
    }

    fn obj_operand(t: &Token, i: usize) -> ObjRef {
        match t.operands.get(i) {
            Some(Operand::Obj(r)) => *r,
            _ => ObjRef::NONE,
        }
    }

    fn u16_operand(t: &Token, i: usize) -> u16 {
        match t.operands.get(i) {
            Some(Operand::U16(v)) => *v,
            _ => 0,
        }
    }

    fn args(&mut self, t: &Token) -> Vec<Expr> {
        t.args.iter().filter(|a| a.op != op::END_FUNCTION_PARMS).map(|a| self.expr(a)).collect()
    }

    fn jump(&mut self, offset: u16) -> usize {
        self.pending_jumps.push(offset as u32);
        // Resolved to a statement index after all statements are known.
        offset as usize
    }

    pub fn expr(&mut self, t: &Token) -> Expr {
        let arg = |c: &mut Self, i: usize| Box::new(t.args.get(i).map(|a| c.expr(a)).unwrap_or(Expr::Nothing));
        if let Some(index) = t.native {
            let decl = self.native_decls.get(&index).copied();
            let args = self.args(t);
            return Expr::Native(index, decl, args);
        }
        match t.op {
            op::LOCAL_VARIABLE => self.prop_slot(Self::obj_operand(t, 0)).map(Expr::Local).unwrap_or(Expr::Unsupported(t.op)),
            op::INSTANCE_VARIABLE | op::STATE_VARIABLE | op::NATIVE_PARM => {
                self.prop_slot(Self::obj_operand(t, 0)).map(Expr::Instance).unwrap_or(Expr::Unsupported(t.op))
            }
            op::DEFAULT_VARIABLE => self.prop_slot(Self::obj_operand(t, 0)).map(Expr::Default).unwrap_or(Expr::Unsupported(t.op)),
            op::RETURN => Expr::Return(arg(self, 0)),
            op::RETURN_NOTHING => Expr::ReturnNothing,
            op::SWITCH => Expr::Switch(arg(self, 0)),
            op::JUMP => Expr::Jump(self.jump(Self::u16_operand(t, 0))),
            op::JUMP_IF_NOT => {
                let target = self.jump(Self::u16_operand(t, 0));
                Expr::JumpIfNot(target, arg(self, 0))
            }
            op::STOP => Expr::Stop,
            op::ASSERT => Expr::Assert(Self::u16_operand(t, 0), arg(self, 0)),
            op::CASE => {
                let next = Self::u16_operand(t, 0);
                if next == 0xFFFF {
                    Expr::Case(usize::MAX, None)
                } else {
                    let target = self.jump(next);
                    Expr::Case(target, Some(arg(self, 0)))
                }
            }
            op::NOTHING => Expr::Nothing,
            op::END_FUNCTION_PARMS => Expr::EndParms,
            op::LABEL_TABLE => {
                let labels = match t.operands.first() {
                    Some(Operand::Labels(l)) => l.clone(),
                    _ => Vec::new(),
                };
                Expr::LabelTable(labels.into_iter().map(|(n, at)| (self.name(n), at)).collect())
            }
            op::GOTO_LABEL => Expr::GotoLabel(arg(self, 0)),
            op::EAT_STRING => Expr::Discard(arg(self, 0)),
            op::LET | op::LET_BOOL | op::LET_DELEGATE => Expr::Let(arg(self, 0), arg(self, 1)),
            op::DYN_ARRAY_ELEMENT => Expr::DynArrayElement(arg(self, 0), arg(self, 1)),
            op::ARRAY_ELEMENT => Expr::ArrayElement(arg(self, 0), arg(self, 1)),
            op::NEW => Expr::New(t.args.iter().map(|a| self.expr(a)).collect()),
            op::CLASS_CONTEXT => Expr::ClassContext(arg(self, 0), arg(self, 1)),
            op::CONTEXT => Expr::Context(arg(self, 0), arg(self, 1)),
            op::META_CAST => {
                let c = self.struct_ref(Self::obj_operand(t, 0));
                Expr::MetaCast(c, arg(self, 0))
            }
            op::DYNAMIC_CAST => {
                let c = self.struct_ref(Self::obj_operand(t, 0));
                Expr::DynamicCast(c, arg(self, 0))
            }
            op::LINE_NUMBER => Expr::Nothing,
            op::SELF => Expr::SelfObj,
            op::SKIP => *arg(self, 0),
            op::BOOL_VARIABLE => *arg(self, 0),
            op::VIRTUAL_FUNCTION | op::GLOBAL_FUNCTION => {
                let n = match t.operands.first() {
                    Some(Operand::Name(n)) => self.name(*n),
                    _ => Name::NONE,
                };
                let args = self.args(t);
                if t.op == op::GLOBAL_FUNCTION { Expr::Global(n, args) } else { Expr::Virtual(n, args) }
            }
            op::FINAL_FUNCTION => {
                let f = self.struct_ref(Self::obj_operand(t, 0));
                let args = self.args(t);
                match f {
                    Some(f) => Expr::Final(f, args),
                    None => Expr::Unsupported(t.op),
                }
            }
            op::INT_CONST => match t.operands.first() {
                Some(Operand::I32(v)) => Expr::Const(Value::Int(*v)),
                _ => Expr::Const(Value::Int(0)),
            },
            op::FLOAT_CONST => match t.operands.first() {
                Some(Operand::F32(v)) => Expr::Const(Value::Float(*v)),
                _ => Expr::Const(Value::Float(0.0)),
            },
            op::STRING_CONST | op::UNICODE_STRING_CONST => match t.operands.first() {
                Some(Operand::Str(s)) => Expr::Const(Value::Str(s.clone())),
                _ => Expr::Const(Value::Str(String::new())),
            },
            op::OBJECT_CONST => {
                let o = self.obj(Self::obj_operand(t, 0));
                Expr::Const(Value::Object(o))
            }
            op::NAME_CONST => match t.operands.first() {
                Some(Operand::Name(n)) => Expr::Const(Value::Name(self.name(*n))),
                _ => Expr::Const(Value::Name(Name::NONE)),
            },
            op::ROTATION_CONST | op::VECTOR_CONST => Expr::Const(Value::Struct(
                t.operands
                    .iter()
                    .map(|o| match o {
                        Operand::I32(v) => Value::Int(*v),
                        Operand::F32(v) => Value::Float(*v),
                        _ => Value::Empty,
                    })
                    .collect(),
            )),
            op::BYTE_CONST => match t.operands.first() {
                Some(Operand::U8(v)) => Expr::Const(Value::Byte(*v)),
                _ => Expr::Const(Value::Byte(0)),
            },
            op::INT_CONST_BYTE => match t.operands.first() {
                Some(Operand::U8(v)) => Expr::Const(Value::Int(*v as i32)),
                _ => Expr::Const(Value::Int(0)),
            },
            op::INT_ZERO => Expr::Const(Value::Int(0)),
            op::INT_ONE => Expr::Const(Value::Int(1)),
            op::TRUE => Expr::Const(Value::Bool(true)),
            op::FALSE => Expr::Const(Value::Bool(false)),
            op::NO_OBJECT => Expr::Const(Value::Object(None)),
            op::ITERATOR => {
                let end = self.jump(Self::u16_operand(t, 0));
                Expr::Iterator(arg(self, 0), end)
            }
            op::ITERATOR_NEXT => Expr::IteratorNext,
            op::ITERATOR_POP => Expr::IteratorPop,
            op::STRUCT_CMP_EQ | op::STRUCT_CMP_NE => Expr::StructCmp(t.op == op::STRUCT_CMP_EQ, arg(self, 0), arg(self, 1)),
            op::STRUCT_MEMBER => match self.prop_slot(Self::obj_operand(t, 0)) {
                Some(slot) => Expr::StructMember(slot, arg(self, 0)),
                None => Expr::Unsupported(t.op),
            },
            op::DYN_ARRAY_LENGTH => Expr::DynArrayLength(arg(self, 0)),
            op::DYN_ARRAY_INSERT => Expr::DynArrayInsert(arg(self, 0), arg(self, 1), arg(self, 2)),
            op::DYN_ARRAY_REMOVE => Expr::DynArrayRemove(arg(self, 0), arg(self, 1), arg(self, 2)),
            op::PRIMITIVE_CAST => match t.operands.first() {
                Some(Operand::U8(c)) => Expr::Cast(*c, arg(self, 0)),
                _ => Expr::Unsupported(t.op),
            },
            op::DELEGATE_FUNCTION => {
                let var = self.var_expr(Self::obj_operand(t, 0)).unwrap_or(Expr::Unsupported(t.op));
                let n = match t.operands.get(1) {
                    Some(Operand::Name(n)) => self.name(*n),
                    _ => Name::NONE,
                };
                let args = self.args(t);
                Expr::DelegateCall(Box::new(var), n, args)
            }
            op::DELEGATE_PROPERTY => match t.operands.first() {
                Some(Operand::Name(n)) => Expr::DelegateValue(self.name(*n)),
                _ => Expr::Unsupported(t.op),
            },
            other => Expr::Unsupported(other),
        }
    }

    /// Compiles a whole script.
    pub fn compile(mut self, tokens: &[Token]) -> Code {
        let mut statements: Vec<Expr> = tokens.iter().map(|t| self.expr(t)).collect();
        let offsets: Vec<u32> = tokens.iter().map(|t| t.offset).collect();
        let by_offset: HashMap<u32, usize> = offsets.iter().enumerate().map(|(i, &o)| (o, i)).collect();
        let end = statements.len();
        let fix = |target: &mut usize| {
            if *target != usize::MAX {
                *target = by_offset.get(&(*target as u32)).copied().unwrap_or(end);
            }
        };
        let mut labels = HashMap::default();
        for s in &mut statements {
            match s {
                Expr::Jump(t) | Expr::JumpIfNot(t, _) | Expr::Case(t, _) | Expr::Iterator(_, t) => fix(t),
                Expr::LabelTable(l) => {
                    for (n, at) in l.iter() {
                        labels.insert(*n, by_offset.get(at).copied().unwrap_or(end));
                    }
                }
                _ => {}
            }
        }
        self.pending_jumps.clear();
        Code { statements, offsets, by_offset, labels }
    }
}
