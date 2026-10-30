//! UnrealScript bytecode as stored in HP3's packages.
//!
//! On disk, object and name references are compact indices, while the script size and every jump offset count
//! bytes of the loaded form, where both are 4 bytes. The decoder therefore walks every token, tracking the loaded
//! offset, until it reaches the stored script size.

use crate::package::{ObjRef, Package};
use crate::reader::Reader;
use crate::{Error, Result};

pub mod op {
    pub const LOCAL_VARIABLE: u8 = 0x00;
    pub const INSTANCE_VARIABLE: u8 = 0x01;
    pub const DEFAULT_VARIABLE: u8 = 0x02;
    pub const STATE_VARIABLE: u8 = 0x03;
    pub const RETURN: u8 = 0x04;
    pub const SWITCH: u8 = 0x05;
    pub const JUMP: u8 = 0x06;
    pub const JUMP_IF_NOT: u8 = 0x07;
    pub const STOP: u8 = 0x08;
    pub const ASSERT: u8 = 0x09;
    pub const CASE: u8 = 0x0A;
    pub const NOTHING: u8 = 0x0B;
    pub const LABEL_TABLE: u8 = 0x0C;
    pub const GOTO_LABEL: u8 = 0x0D;
    pub const EAT_STRING: u8 = 0x0E;
    pub const LET: u8 = 0x0F;
    pub const DYN_ARRAY_ELEMENT: u8 = 0x10;
    pub const NEW: u8 = 0x11;
    pub const CLASS_CONTEXT: u8 = 0x12;
    pub const META_CAST: u8 = 0x13;
    pub const LET_BOOL: u8 = 0x14;
    pub const LINE_NUMBER: u8 = 0x15;
    pub const END_FUNCTION_PARMS: u8 = 0x16;
    pub const SELF: u8 = 0x17;
    pub const SKIP: u8 = 0x18;
    pub const CONTEXT: u8 = 0x19;
    pub const ARRAY_ELEMENT: u8 = 0x1A;
    pub const VIRTUAL_FUNCTION: u8 = 0x1B;
    pub const FINAL_FUNCTION: u8 = 0x1C;
    pub const INT_CONST: u8 = 0x1D;
    pub const FLOAT_CONST: u8 = 0x1E;
    pub const STRING_CONST: u8 = 0x1F;
    pub const OBJECT_CONST: u8 = 0x20;
    pub const NAME_CONST: u8 = 0x21;
    pub const ROTATION_CONST: u8 = 0x22;
    pub const VECTOR_CONST: u8 = 0x23;
    pub const BYTE_CONST: u8 = 0x24;
    pub const INT_ZERO: u8 = 0x25;
    pub const INT_ONE: u8 = 0x26;
    pub const TRUE: u8 = 0x27;
    pub const FALSE: u8 = 0x28;
    pub const NATIVE_PARM: u8 = 0x29;
    pub const NO_OBJECT: u8 = 0x2A;
    pub const INT_CONST_BYTE: u8 = 0x2C;
    pub const BOOL_VARIABLE: u8 = 0x2D;
    pub const DYNAMIC_CAST: u8 = 0x2E;
    pub const ITERATOR: u8 = 0x2F;
    pub const ITERATOR_POP: u8 = 0x30;
    pub const ITERATOR_NEXT: u8 = 0x31;
    pub const STRUCT_CMP_EQ: u8 = 0x32;
    pub const STRUCT_CMP_NE: u8 = 0x33;
    pub const UNICODE_STRING_CONST: u8 = 0x34;
    pub const RANGE_CONST: u8 = 0x35;
    pub const STRUCT_MEMBER: u8 = 0x36;
    pub const DYN_ARRAY_LENGTH: u8 = 0x37;
    pub const GLOBAL_FUNCTION: u8 = 0x38;
    pub const PRIMITIVE_CAST: u8 = 0x39;
    pub const RETURN_NOTHING: u8 = 0x3A;
    pub const DELEGATE_CMP_EQ: u8 = 0x3B;
    pub const DELEGATE_CMP_NE: u8 = 0x3C;
    pub const DELEGATE_FUNCTION_CMP_EQ: u8 = 0x3D;
    pub const DELEGATE_FUNCTION_CMP_NE: u8 = 0x3E;
    pub const EMPTY_DELEGATE: u8 = 0x3F;
    pub const DYN_ARRAY_INSERT: u8 = 0x40;
    pub const DYN_ARRAY_REMOVE: u8 = 0x41;
    pub const DEBUG_INFO: u8 = 0x42;
    pub const DELEGATE_FUNCTION: u8 = 0x43;
    pub const DELEGATE_PROPERTY: u8 = 0x44;
    pub const LET_DELEGATE: u8 = 0x45;
    pub const EXTENDED_NATIVE: u8 = 0x60;
    pub const FIRST_NATIVE: u8 = 0x70;
}

/// Size of an object or name reference in the loaded script.
const REF_SIZE: u32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Obj(ObjRef),
    Name(usize),
    U8(u8),
    U16(u16),
    I32(i32),
    F32(f32),
    Str(String),
    /// Label name and loaded code offset.
    Labels(Vec<(usize, u32)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub op: u8,
    /// Native function index for native calls.
    pub native: Option<u16>,
    /// Offset of the token in the loaded script.
    pub offset: u32,
    pub operands: Vec<Operand>,
    pub args: Vec<Token>,
}

impl Token {
    fn new(op: u8, offset: u32) -> Token {
        Token { op, native: None, offset, operands: Vec::new(), args: Vec::new() }
    }
}

pub struct Decoder<'p, 'r, 'a> {
    pkg: &'p Package,
    r: &'r mut Reader<'a>,
    mem: u32,
    depth: u32,
}

impl<'p, 'r, 'a> Decoder<'p, 'r, 'a> {
    fn err(&self, msg: String) -> Error {
        Error::Format(format!("bytecode at script offset {:#x} (file {:#x}): {msg}", self.mem, self.r.file_pos()))
    }

    fn obj(&mut self, t: &mut Token) -> Result<()> {
        let v = self.r.ci()?;
        self.mem += REF_SIZE;
        t.operands.push(Operand::Obj(ObjRef(v)));
        Ok(())
    }

    fn name(&mut self, t: &mut Token) -> Result<usize> {
        let v = self.r.ci()?;
        if v < 0 || v as usize >= self.pkg.names.len() {
            return Err(self.err(format!("bad name index {v}")));
        }
        self.mem += REF_SIZE;
        t.operands.push(Operand::Name(v as usize));
        Ok(v as usize)
    }

    fn u8(&mut self, t: &mut Token) -> Result<u8> {
        let v = self.r.u8()?;
        self.mem += 1;
        t.operands.push(Operand::U8(v));
        Ok(v)
    }

    fn u16(&mut self, t: &mut Token) -> Result<u16> {
        let v = self.r.u16()?;
        self.mem += 2;
        t.operands.push(Operand::U16(v));
        Ok(v)
    }

    fn i32(&mut self, t: &mut Token) -> Result<()> {
        let v = self.r.i32()?;
        self.mem += 4;
        t.operands.push(Operand::I32(v));
        Ok(())
    }

    fn f32(&mut self, t: &mut Token) -> Result<()> {
        let v = self.r.f32()?;
        self.mem += 4;
        t.operands.push(Operand::F32(v));
        Ok(())
    }

    fn arg(&mut self, t: &mut Token) -> Result<()> {
        let e = self.expr()?;
        t.args.push(e);
        Ok(())
    }

    /// Arguments up to and including the end-of-parameters token.
    fn call_args(&mut self, t: &mut Token) -> Result<()> {
        loop {
            let e = self.expr()?;
            let end = e.op == op::END_FUNCTION_PARMS;
            t.args.push(e);
            if end {
                return Ok(());
            }
            if t.args.len() > 64 {
                return Err(self.err("runaway parameter list".into()));
            }
        }
    }

    pub fn expr(&mut self) -> Result<Token> {
        self.depth += 1;
        if self.depth > 512 {
            return Err(self.err("expression nesting too deep".into()));
        }
        let offset = self.mem;
        let code = self.r.u8()?;
        self.mem += 1;
        let mut t = Token::new(code, offset);
        use op::*;
        match code {
            LOCAL_VARIABLE | INSTANCE_VARIABLE | DEFAULT_VARIABLE | STATE_VARIABLE | NATIVE_PARM | OBJECT_CONST => {
                self.obj(&mut t)?
            }
            RETURN | GOTO_LABEL | EAT_STRING | BOOL_VARIABLE | DYN_ARRAY_LENGTH => self.arg(&mut t)?,
            SWITCH => {
                self.u8(&mut t)?;
                self.arg(&mut t)?;
            }
            JUMP => {
                self.u16(&mut t)?;
            }
            JUMP_IF_NOT => {
                self.u16(&mut t)?;
                self.arg(&mut t)?;
            }
            STOP | NOTHING | END_FUNCTION_PARMS | SELF | INT_ZERO | INT_ONE | TRUE | FALSE | NO_OBJECT
            | ITERATOR_POP | ITERATOR_NEXT | EMPTY_DELEGATE => {}
            ASSERT => {
                self.u16(&mut t)?;
                self.arg(&mut t)?;
            }
            CASE => {
                if self.u16(&mut t)? != 0xFFFF {
                    self.arg(&mut t)?;
                }
            }
            LABEL_TABLE => {
                let mut labels = Vec::new();
                loop {
                    let n = self.r.ci()?;
                    let at = self.r.u32()?;
                    self.mem += REF_SIZE + 4;
                    if n < 0 || n as usize >= self.pkg.names.len() {
                        return Err(self.err(format!("bad label name {n}")));
                    }
                    if self.pkg.name(n as usize).eq_ignore_ascii_case("None") {
                        break;
                    }
                    labels.push((n as usize, at));
                }
                t.operands.push(Operand::Labels(labels));
            }
            LET | LET_BOOL | DYN_ARRAY_ELEMENT | ARRAY_ELEMENT | LET_DELEGATE => {
                self.arg(&mut t)?;
                self.arg(&mut t)?;
            }
            DELEGATE_CMP_EQ | DELEGATE_CMP_NE | DELEGATE_FUNCTION_CMP_EQ | DELEGATE_FUNCTION_CMP_NE => {
                self.arg(&mut t)?;
                self.arg(&mut t)?;
                self.arg(&mut t)?;
            }
            NEW => {
                for _ in 0..4 {
                    self.arg(&mut t)?;
                }
            }
            CLASS_CONTEXT | CONTEXT => {
                self.arg(&mut t)?;
                self.u16(&mut t)?;
                self.u8(&mut t)?;
                self.arg(&mut t)?;
            }
            META_CAST | DYNAMIC_CAST | STRUCT_MEMBER => {
                self.obj(&mut t)?;
                self.arg(&mut t)?;
            }
            LINE_NUMBER => {
                self.u16(&mut t)?;
            }
            SKIP => {
                self.u16(&mut t)?;
                self.arg(&mut t)?;
            }
            VIRTUAL_FUNCTION | GLOBAL_FUNCTION => {
                self.name(&mut t)?;
                self.call_args(&mut t)?;
            }
            FINAL_FUNCTION => {
                self.obj(&mut t)?;
                self.call_args(&mut t)?;
            }
            DELEGATE_FUNCTION => {
                self.obj(&mut t)?;
                self.name(&mut t)?;
                self.call_args(&mut t)?;
            }
            DELEGATE_PROPERTY => {
                self.name(&mut t)?;
            }
            INT_CONST => self.i32(&mut t)?,
            FLOAT_CONST => self.f32(&mut t)?,
            STRING_CONST => {
                let mut s = String::new();
                loop {
                    let b = self.r.u8()?;
                    self.mem += 1;
                    if b == 0 {
                        break;
                    }
                    s.push(b as char);
                }
                t.operands.push(Operand::Str(s));
            }
            UNICODE_STRING_CONST => {
                let mut units = Vec::new();
                loop {
                    let u = self.r.u16()?;
                    self.mem += 2;
                    if u == 0 {
                        break;
                    }
                    units.push(u);
                }
                t.operands.push(Operand::Str(String::from_utf16_lossy(&units)));
            }
            NAME_CONST => {
                self.name(&mut t)?;
            }
            ROTATION_CONST => {
                for _ in 0..3 {
                    self.i32(&mut t)?;
                }
            }
            VECTOR_CONST => {
                for _ in 0..3 {
                    self.f32(&mut t)?;
                }
            }
            RANGE_CONST => {
                self.f32(&mut t)?;
                self.f32(&mut t)?;
            }
            BYTE_CONST | INT_CONST_BYTE => {
                self.u8(&mut t)?;
            }
            ITERATOR => {
                self.arg(&mut t)?;
                self.u16(&mut t)?;
            }
            STRUCT_CMP_EQ | STRUCT_CMP_NE => {
                self.obj(&mut t)?;
                self.arg(&mut t)?;
                self.arg(&mut t)?;
            }
            PRIMITIVE_CAST => {
                self.u8(&mut t)?;
                self.arg(&mut t)?;
            }
            RETURN_NOTHING => {}
            DYN_ARRAY_INSERT | DYN_ARRAY_REMOVE => {
                for _ in 0..3 {
                    self.arg(&mut t)?;
                }
            }
            DEBUG_INFO => {
                self.i32(&mut t)?;
                self.i32(&mut t)?;
                self.i32(&mut t)?;
                self.u8(&mut t)?;
            }
            EXTENDED_NATIVE..=0x6F => {
                let lo = self.r.u8()?;
                self.mem += 1;
                t.native = Some((((code - EXTENDED_NATIVE) as u16) << 8) | lo as u16);
                self.call_args(&mut t)?;
            }
            FIRST_NATIVE..=0xFF => {
                t.native = Some(code as u16);
                self.call_args(&mut t)?;
            }
            _ => return Err(self.err(format!("unknown token {code:#04x}"))),
        }
        self.depth -= 1;
        Ok(t)
    }
}

/// Decodes a script of `size` loaded bytes.
pub fn decode_script(pkg: &Package, r: &mut Reader<'_>, size: u32) -> Result<Vec<Token>> {
    let mut d = Decoder { pkg, r, mem: 0, depth: 0 };
    let mut out = Vec::new();
    while d.mem < size {
        d.depth = 0;
        out.push(d.expr()?);
    }
    if d.mem != size {
        return Err(Error::Format(format!("script decoded to {:#x} loaded bytes, header says {size:#x}", d.mem)));
    }
    Ok(out)
}

/// Calls `f` on every token of a script, parents before children.
pub fn walk(tokens: &[Token], f: &mut impl FnMut(&Token)) {
    for t in tokens {
        f(t);
        walk(&t.args, f);
    }
}
