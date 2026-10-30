//! Native functions, registered by `Class.Function` (lower case). Core's natives live here; other packages'
//! natives are added by the crates that implement them.
//!
//! Every function here is written from the script declarations and standard UnrealScript semantics
//! (NOT yet verified against HP3's Core.dll).

use std::collections::HashMap;

use crate::names::Name;
use crate::value::{ObjId, Value};
use crate::vm::{Vm, VmError, VmResult};

pub type NativeFn = fn(&mut Vm<'_>, Option<ObjId>, &mut [Value]) -> VmResult<Value>;

fn vec3(v: &Value) -> [f32; 3] {
    let f = v.fields();
    [f.first().map(Value::as_float).unwrap_or(0.0), f.get(1).map(Value::as_float).unwrap_or(0.0), f.get(2).map(Value::as_float).unwrap_or(0.0)]
}

fn mk_vec(v: [f32; 3]) -> Value {
    Value::Struct(Box::new([Value::Float(v[0]), Value::Float(v[1]), Value::Float(v[2])]))
}

fn rot3(v: &Value) -> [i32; 3] {
    let f = v.fields();
    [f.first().map(Value::as_int).unwrap_or(0), f.get(1).map(Value::as_int).unwrap_or(0), f.get(2).map(Value::as_int).unwrap_or(0)]
}

fn mk_rot(r: [i32; 3]) -> Value {
    Value::Struct(Box::new([Value::Int(r[0]), Value::Int(r[1]), Value::Int(r[2])]))
}

/// Formats a float the way UnrealScript prints floats: six decimals.
pub fn float_to_string(f: f32) -> String {
    format!("{f:.6}")
}

fn parse_float(s: &str) -> f32 {
    let t = s.trim();
    let end = t
        .char_indices()
        .take_while(|(i, c)| c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0))
        .count();
    t[..end].parse().unwrap_or(0.0)
}

fn parse_int(s: &str) -> i32 {
    let t = s.trim();
    let end = t.char_indices().take_while(|(i, c)| c.is_ascii_digit() || ((*c == '-' || *c == '+') && *i == 0)).count();
    t[..end].parse().unwrap_or(0)
}

/// Unreal's pseudo-random float in [0, 1). Deterministic so runs can be replayed.
fn frand(vm: &mut Vm<'_>) -> f32 {
    vm.rng = vm.rng.wrapping_mul(214_013).wrapping_add(2_531_011);
    ((vm.rng >> 16) & 0x7FFF) as f32 / 32768.0
}

pub fn primitive_cast(vm: &mut Vm<'_>, code: u8, v: Value) -> VmResult<Value> {
    Ok(match code {
        0x39 => {
            let r = rot3(&v);
            let (p, y) = (r[0] as f32 * std::f32::consts::TAU / 65536.0, r[1] as f32 * std::f32::consts::TAU / 65536.0);
            mk_vec([p.cos() * y.cos(), p.cos() * y.sin(), p.sin()])
        }
        0x3A | 0x41 | 0x44 => Value::Int(v.as_int()),
        0x3B | 0x3E | 0x45 | 0x47 => Value::Bool(v.as_bool()),
        0x3C | 0x3F | 0x42 => Value::Float(v.as_float()),
        0x3D | 0x40 | 0x43 => Value::Byte(v.as_int() as u8),
        0x48 => Value::Bool(v.as_name() != Name::NONE),
        0x49 => Value::Byte(parse_int(v.as_str()) as u8),
        0x4A => Value::Int(parse_int(v.as_str())),
        0x4B => {
            let s = v.as_str().trim();
            Value::Bool(s.eq_ignore_ascii_case("true") || parse_int(s) != 0)
        }
        0x4C => Value::Float(parse_float(v.as_str())),
        0x4D => {
            let p: Vec<f32> = v.as_str().split(',').map(parse_float).collect();
            mk_vec([p.first().copied().unwrap_or(0.0), p.get(1).copied().unwrap_or(0.0), p.get(2).copied().unwrap_or(0.0)])
        }
        0x4E => {
            let p: Vec<i32> = v.as_str().split(',').map(parse_int).collect();
            mk_rot([p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0), p.get(2).copied().unwrap_or(0)])
        }
        0x4F => Value::Bool(vec3(&v) != [0.0; 3]),
        0x50 => {
            let [x, y, z] = vec3(&v);
            let yaw = (y.atan2(x) * 32768.0 / std::f32::consts::PI) as i32 & 0xFFFF;
            let pitch = (z.atan2((x * x + y * y).sqrt()) * 32768.0 / std::f32::consts::PI) as i32 & 0xFFFF;
            mk_rot([pitch, yaw, 0])
        }
        0x51 => Value::Bool(rot3(&v) != [0; 3]),
        0x52 | 0x53 => Value::Str(v.as_int().to_string()),
        0x54 => Value::Str(if v.as_bool() { "True" } else { "False" }.into()),
        0x55 => Value::Str(float_to_string(v.as_float())),
        0x56 => Value::Str(match v.as_obj() {
            Some(o) => object_path(vm, o),
            None => "None".into(),
        }),
        0x57 => Value::Str(vm.db.names.get(v.as_name()).to_string()),
        0x58 => {
            let [x, y, z] = vec3(&v);
            Value::Str(format!("{},{},{}", float_to_string(x), float_to_string(y), float_to_string(z)))
        }
        0x59 => {
            let [p, y, r] = rot3(&v);
            Value::Str(format!("{p},{y},{r}"))
        }
        other => return Err(VmError::Unimplemented(format!("primitive cast {other:#04x}"))),
    })
}

/// `Package.Name` of an object, the way scripts print object references.
pub fn object_path(vm: &Vm<'_>, o: ObjId) -> String {
    let e = vm.db.objects.get(o);
    match &e.key {
        Some((pkg, idx)) => match vm.db.set.get(pkg) {
            Ok(p) => p.ref_path(hp3_package::ObjRef::from_export(*idx)),
            Err(_) => vm.db.names.get(e.name).to_string(),
        },
        None => vm.db.names.get(e.name).to_string(),
    }
}

macro_rules! natives {
    ($( $name:literal => |$vm:ident, $this:ident, $a:ident| $body:expr ),* $(,)?) => {
        vec![$( ($name, (|$vm: &mut Vm<'_>, $this: Option<ObjId>, $a: &mut [Value]| -> VmResult<Value> {
            let _ = (&$vm, &$this, &$a);
            Ok($body)
        }) as NativeFn) ),*]
    };
}

fn int_op(a: &[Value]) -> (i32, i32) {
    (a[0].as_int(), a[1].as_int())
}

fn float_op(a: &[Value]) -> (f32, f32) {
    (a[0].as_float(), a[1].as_float())
}

pub fn registry() -> HashMap<String, NativeFn> {
    let list: Vec<(&str, NativeFn)> = natives![
        // Integers.
        "Object.Add_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.wrapping_add(y)) },
        "Object.Subtract_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.wrapping_sub(y)) },
        "Object.Multiply_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.wrapping_mul(y)) },
        "Object.Divide_IntInt" => |vm, this, a| {
            let (x, y) = int_op(a);
            if y == 0 { vm.warn("divide by zero".into()); Value::Int(0) } else { Value::Int(x.wrapping_div(y)) }
        },
        "Object.Percent_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(if y == 0.0 { 0.0 } else { x % y }) },
        "Object.Subtract_PreInt" => |vm, this, a| Value::Int(a[0].as_int().wrapping_neg()),
        "Object.Complement_PreInt" => |vm, this, a| Value::Int(!a[0].as_int()),
        "Object.LessLess_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.wrapping_shl(y as u32)) },
        "Object.GreaterGreater_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.wrapping_shr(y as u32)) },
        "Object.GreaterGreaterGreater_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int((x as u32).wrapping_shr(y as u32) as i32) },
        "Object.And_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x & y) },
        "Object.Or_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x | y) },
        "Object.Xor_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x ^ y) },
        "Object.Less_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x < y) },
        "Object.Greater_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x > y) },
        "Object.LessEqual_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x <= y) },
        "Object.GreaterEqual_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x >= y) },
        "Object.EqualEqual_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x == y) },
        "Object.NotEqual_IntInt" => |vm, this, a| { let (x, y) = int_op(a); Value::Bool(x != y) },
        "Object.AddEqual_IntInt" => |vm, this, a| { let v = a[0].as_int().wrapping_add(a[1].as_int()); a[0] = Value::Int(v); Value::Int(v) },
        "Object.SubtractEqual_IntInt" => |vm, this, a| { let v = a[0].as_int().wrapping_sub(a[1].as_int()); a[0] = Value::Int(v); Value::Int(v) },
        "Object.MultiplyEqual_IntFloat" => |vm, this, a| { let v = (a[0].as_int() as f32 * a[1].as_float()) as i32; a[0] = Value::Int(v); Value::Int(v) },
        "Object.DivideEqual_IntFloat" => |vm, this, a| {
            let d = a[1].as_float();
            let v = if d == 0.0 { 0 } else { (a[0].as_int() as f32 / d) as i32 };
            a[0] = Value::Int(v); Value::Int(v)
        },
        "Object.AddAdd_PreInt" => |vm, this, a| { let v = a[0].as_int().wrapping_add(1); a[0] = Value::Int(v); Value::Int(v) },
        "Object.SubtractSubtract_PreInt" => |vm, this, a| { let v = a[0].as_int().wrapping_sub(1); a[0] = Value::Int(v); Value::Int(v) },
        "Object.AddAdd_Int" => |vm, this, a| { let v = a[0].as_int(); a[0] = Value::Int(v.wrapping_add(1)); Value::Int(v) },
        "Object.SubtractSubtract_Int" => |vm, this, a| { let v = a[0].as_int(); a[0] = Value::Int(v.wrapping_sub(1)); Value::Int(v) },
        "Object.Rand" => |vm, this, a| { let n = a[0].as_int(); Value::Int(if n > 0 { (frand(vm) * n as f32) as i32 % n } else { 0 }) },
        "Object.Min" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.min(y)) },
        "Object.Max" => |vm, this, a| { let (x, y) = int_op(a); Value::Int(x.max(y)) },
        "Object.Clamp" => |vm, this, a| { let (v, lo, hi) = (a[0].as_int(), a[1].as_int(), a[2].as_int()); Value::Int(v.max(lo).min(hi)) },
        "Object.ClockwiseFrom_IntInt" => |vm, this, a| {
            let (x, y) = int_op(a);
            Value::Bool(((x - y) & 0xFFFF) > 32768)
        },
        // Bytes.
        "Object.AddEqual_ByteByte" => |vm, this, a| { let v = a[0].as_byte().wrapping_add(a[1].as_byte()); a[0] = Value::Byte(v); Value::Byte(v) },
        "Object.SubtractEqual_ByteByte" => |vm, this, a| { let v = a[0].as_byte().wrapping_sub(a[1].as_byte()); a[0] = Value::Byte(v); Value::Byte(v) },
        "Object.MultiplyEqual_ByteByte" => |vm, this, a| { let v = a[0].as_byte().wrapping_mul(a[1].as_byte()); a[0] = Value::Byte(v); Value::Byte(v) },
        "Object.DivideEqual_ByteByte" => |vm, this, a| {
            let d = a[1].as_byte();
            let v = if d == 0 { 0 } else { a[0].as_byte() / d };
            a[0] = Value::Byte(v); Value::Byte(v)
        },
        "Object.AddAdd_PreByte" => |vm, this, a| { let v = a[0].as_byte().wrapping_add(1); a[0] = Value::Byte(v); Value::Byte(v) },
        "Object.SubtractSubtract_PreByte" => |vm, this, a| { let v = a[0].as_byte().wrapping_sub(1); a[0] = Value::Byte(v); Value::Byte(v) },
        "Object.AddAdd_Byte" => |vm, this, a| { let v = a[0].as_byte(); a[0] = Value::Byte(v.wrapping_add(1)); Value::Byte(v) },
        "Object.SubtractSubtract_Byte" => |vm, this, a| { let v = a[0].as_byte(); a[0] = Value::Byte(v.wrapping_sub(1)); Value::Byte(v) },
        // Booleans; && and || short-circuit in the VM.
        "Object.Not_PreBool" => |vm, this, a| Value::Bool(!a[0].as_bool()),
        "Object.AndAnd_BoolBool" => |vm, this, a| Value::Bool(a[0].as_bool() && a[1].as_bool()),
        "Object.OrOr_BoolBool" => |vm, this, a| Value::Bool(a[0].as_bool() || a[1].as_bool()),
        "Object.XorXor_BoolBool" => |vm, this, a| Value::Bool(a[0].as_bool() != a[1].as_bool()),
        "Object.EqualEqual_BoolBool" => |vm, this, a| Value::Bool(a[0].as_bool() == a[1].as_bool()),
        "Object.NotEqual_BoolBool" => |vm, this, a| Value::Bool(a[0].as_bool() != a[1].as_bool()),
        // Floats.
        "Object.Add_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x + y) },
        "Object.Subtract_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x - y) },
        "Object.Multiply_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x * y) },
        "Object.Divide_FloatFloat" => |vm, this, a| {
            let (x, y) = float_op(a);
            if y == 0.0 { vm.warn("divide by zero".into()); Value::Float(0.0) } else { Value::Float(x / y) }
        },
        "Object.MultiplyMultiply_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x.powf(y)) },
        "Object.Subtract_PreFloat" => |vm, this, a| Value::Float(-a[0].as_float()),
        "Object.Less_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x < y) },
        "Object.Greater_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x > y) },
        "Object.LessEqual_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x <= y) },
        "Object.GreaterEqual_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x >= y) },
        "Object.EqualEqual_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x == y) },
        "Object.NotEqual_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool(x != y) },
        "Object.ComplementEqual_FloatFloat" => |vm, this, a| { let (x, y) = float_op(a); Value::Bool((x - y).abs() < 1e-4) },
        "Object.AddEqual_FloatFloat" => |vm, this, a| { let v = a[0].as_float() + a[1].as_float(); a[0] = Value::Float(v); Value::Float(v) },
        "Object.SubtractEqual_FloatFloat" => |vm, this, a| { let v = a[0].as_float() - a[1].as_float(); a[0] = Value::Float(v); Value::Float(v) },
        "Object.MultiplyEqual_FloatFloat" => |vm, this, a| { let v = a[0].as_float() * a[1].as_float(); a[0] = Value::Float(v); Value::Float(v) },
        "Object.DivideEqual_FloatFloat" => |vm, this, a| {
            let d = a[1].as_float();
            let v = if d == 0.0 { 0.0 } else { a[0].as_float() / d };
            a[0] = Value::Float(v); Value::Float(v)
        },
        "Object.Abs" => |vm, this, a| Value::Float(a[0].as_float().abs()),
        "Object.Sin" => |vm, this, a| Value::Float(a[0].as_float().sin()),
        "Object.Cos" => |vm, this, a| Value::Float(a[0].as_float().cos()),
        "Object.Tan" => |vm, this, a| Value::Float(a[0].as_float().tan()),
        "Object.Asin" => |vm, this, a| Value::Float(a[0].as_float().asin()),
        "Object.Acos" => |vm, this, a| Value::Float(a[0].as_float().acos()),
        "Object.Atan" => |vm, this, a| Value::Float(a[0].as_float().atan()),
        "Object.Exp" => |vm, this, a| Value::Float(a[0].as_float().exp()),
        "Object.Loge" => |vm, this, a| Value::Float(a[0].as_float().ln()),
        "Object.Sqrt" => |vm, this, a| Value::Float(a[0].as_float().sqrt()),
        "Object.Square" => |vm, this, a| { let x = a[0].as_float(); Value::Float(x * x) },
        "Object.FRand" => |vm, this, a| Value::Float(frand(vm)),
        "Object.FMin" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x.min(y)) },
        "Object.FMax" => |vm, this, a| { let (x, y) = float_op(a); Value::Float(x.max(y)) },
        "Object.FClamp" => |vm, this, a| Value::Float(a[0].as_float().max(a[1].as_float()).min(a[2].as_float())),
        "Object.Lerp" => |vm, this, a| { let t = a[0].as_float(); Value::Float(a[1].as_float() + (a[2].as_float() - a[1].as_float()) * t) },
        "Object.Smerp" => |vm, this, a| {
            let t = a[0].as_float();
            let s = 3.0 * t * t - 2.0 * t * t * t;
            Value::Float(a[1].as_float() + (a[2].as_float() - a[1].as_float()) * s)
        },
        // Strings.
        "Object.Concat_StrStr" => |vm, this, a| Value::Str(format!("{}{}", a[0].as_str(), a[1].as_str())),
        "Object.At_StrStr" => |vm, this, a| Value::Str(format!("{} {}", a[0].as_str(), a[1].as_str())),
        "Object.Less_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() < a[1].as_str()),
        "Object.Greater_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() > a[1].as_str()),
        "Object.LessEqual_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() <= a[1].as_str()),
        "Object.GreaterEqual_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() >= a[1].as_str()),
        "Object.EqualEqual_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() == a[1].as_str()),
        "Object.NotEqual_StrStr" => |vm, this, a| Value::Bool(a[0].as_str() != a[1].as_str()),
        "Object.ComplementEqual_StrStr" => |vm, this, a| Value::Bool(a[0].as_str().to_lowercase() == a[1].as_str().to_lowercase()),
        "Object.Len" => |vm, this, a| Value::Int(a[0].as_str().chars().count() as i32),
        "Object.InStr" => |vm, this, a| {
            let (s, t) = (a[0].as_str(), a[1].as_str());
            Value::Int(s.find(t).map(|b| s[..b].chars().count() as i32).unwrap_or(-1))
        },
        "Object.Mid" => |vm, this, a| {
            let s: Vec<char> = a[0].as_str().chars().collect();
            let start = a[1].as_int().max(0) as usize;
            let count = match &a[2] { Value::Empty => s.len(), v => v.as_int().max(0) as usize };
            Value::Str(s.iter().skip(start).take(count).collect())
        },
        "Object.Left" => |vm, this, a| Value::Str(a[0].as_str().chars().take(a[1].as_int().max(0) as usize).collect()),
        "Object.Right" => |vm, this, a| {
            let s: Vec<char> = a[0].as_str().chars().collect();
            let n = (a[1].as_int().max(0) as usize).min(s.len());
            Value::Str(s[s.len() - n..].iter().collect())
        },
        "Object.Caps" => |vm, this, a| Value::Str(a[0].as_str().to_uppercase()),
        "Object.Chr" => |vm, this, a| Value::Str(char::from_u32(a[0].as_int() as u32).map(String::from).unwrap_or_default()),
        "Object.Asc" => |vm, this, a| Value::Int(a[0].as_str().chars().next().map(|c| c as i32).unwrap_or(0)),
        // Names and objects.
        "Object.EqualEqual_NameName" => |vm, this, a| Value::Bool(a[0].as_name() == a[1].as_name()),
        "Object.NotEqual_NameName" => |vm, this, a| Value::Bool(a[0].as_name() != a[1].as_name()),
        "Object.EqualEqual_ObjectObject" => |vm, this, a| Value::Bool(a[0].as_obj() == a[1].as_obj()),
        "Object.NotEqual_ObjectObject" => |vm, this, a| Value::Bool(a[0].as_obj() != a[1].as_obj()),
        "Object.Log" => |vm, this, a| {
            let tag = match a.get(1) { Some(Value::Name(n)) if *n != Name::NONE => vm.db.names.get(*n).to_string(), _ => "ScriptLog".into() };
            let line = format!("{tag}: {}", a[0].as_str());
            vm.log.push(line);
            Value::Empty
        },
        "Object.Warn" => |vm, this, a| { let line = format!("ScriptWarning: {}", a[0].as_str()); vm.log.push(line); Value::Empty },
        "Object.IsA" => |vm, this, a| {
            let name = a[0].as_name();
            let mut c = this.and_then(|o| vm.class_of(o));
            let mut found = false;
            while let Some(cid) = c {
                if vm.db.get(cid).name == name { found = true; break; }
                c = vm.db.get(cid).super_;
            }
            Value::Bool(found)
        },
        "Object.ClassIsChildOf" => |vm, this, a| {
            let (Some(x), Some(y)) = (a[0].as_obj(), a[1].as_obj()) else { return Ok(Value::Bool(false)) };
            match (vm.class_of(x), vm.class_of(y)) {
                (Some(cx), Some(cy)) => Value::Bool(vm.is_child_of(cx, cy)),
                _ => Value::Bool(false),
            }
        },
        // Vectors and rotators.
        "Object.Add_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); mk_vec([x[0] + y[0], x[1] + y[1], x[2] + y[2]]) },
        "Object.Subtract_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); mk_vec([x[0] - y[0], x[1] - y[1], x[2] - y[2]]) },
        "Object.Subtract_PreVector" => |vm, this, a| { let x = vec3(&a[0]); mk_vec([-x[0], -x[1], -x[2]]) },
        "Object.Multiply_VectorFloat" => |vm, this, a| { let (x, f) = (vec3(&a[0]), a[1].as_float()); mk_vec([x[0] * f, x[1] * f, x[2] * f]) },
        "Object.Multiply_FloatVector" => |vm, this, a| { let (f, x) = (a[0].as_float(), vec3(&a[1])); mk_vec([x[0] * f, x[1] * f, x[2] * f]) },
        "Object.Multiply_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); mk_vec([x[0] * y[0], x[1] * y[1], x[2] * y[2]]) },
        "Object.Divide_VectorFloat" => |vm, this, a| {
            let (x, f) = (vec3(&a[0]), a[1].as_float());
            if f == 0.0 { mk_vec([0.0; 3]) } else { mk_vec([x[0] / f, x[1] / f, x[2] / f]) }
        },
        "Object.Dot_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); Value::Float(x[0] * y[0] + x[1] * y[1] + x[2] * y[2]) },
        "Object.Cross_VectorVector" => |vm, this, a| {
            let (x, y) = (vec3(&a[0]), vec3(&a[1]));
            mk_vec([x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]])
        },
        "Object.EqualEqual_VectorVector" => |vm, this, a| Value::Bool(vec3(&a[0]) == vec3(&a[1])),
        "Object.NotEqual_VectorVector" => |vm, this, a| Value::Bool(vec3(&a[0]) != vec3(&a[1])),
        "Object.AddEqual_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); a[0] = mk_vec([x[0] + y[0], x[1] + y[1], x[2] + y[2]]); a[0].clone() },
        "Object.SubtractEqual_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); a[0] = mk_vec([x[0] - y[0], x[1] - y[1], x[2] - y[2]]); a[0].clone() },
        "Object.MultiplyEqual_VectorFloat" => |vm, this, a| { let (x, f) = (vec3(&a[0]), a[1].as_float()); a[0] = mk_vec([x[0] * f, x[1] * f, x[2] * f]); a[0].clone() },
        "Object.MultiplyEqual_VectorVector" => |vm, this, a| { let (x, y) = (vec3(&a[0]), vec3(&a[1])); a[0] = mk_vec([x[0] * y[0], x[1] * y[1], x[2] * y[2]]); a[0].clone() },
        "Object.DivideEqual_VectorFloat" => |vm, this, a| {
            let (x, f) = (vec3(&a[0]), a[1].as_float());
            a[0] = if f == 0.0 { mk_vec([0.0; 3]) } else { mk_vec([x[0] / f, x[1] / f, x[2] / f]) };
            a[0].clone()
        },
        "Object.VSize" => |vm, this, a| { let x = vec3(&a[0]); Value::Float((x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()) },
        "Object.Normal" => |vm, this, a| {
            let x = vec3(&a[0]);
            let l = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt();
            if l == 0.0 { mk_vec([0.0; 3]) } else { mk_vec([x[0] / l, x[1] / l, x[2] / l]) }
        },
        "Object.VRand" => |vm, this, a| {
            loop {
                let v = [frand(vm) * 2.0 - 1.0, frand(vm) * 2.0 - 1.0, frand(vm) * 2.0 - 1.0];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if l > 0.01 && l <= 1.0 { break mk_vec([v[0] / l, v[1] / l, v[2] / l]); }
            }
        },
        "Object.MirrorVectorByNormal" => |vm, this, a| {
            let (v, n) = (vec3(&a[0]), vec3(&a[1]));
            let d = 2.0 * (v[0] * n[0] + v[1] * n[1] + v[2] * n[2]);
            mk_vec([v[0] - d * n[0], v[1] - d * n[1], v[2] - d * n[2]])
        },
        "Object.Add_RotatorRotator" => |vm, this, a| { let (x, y) = (rot3(&a[0]), rot3(&a[1])); mk_rot([x[0] + y[0], x[1] + y[1], x[2] + y[2]]) },
        "Object.Subtract_RotatorRotator" => |vm, this, a| { let (x, y) = (rot3(&a[0]), rot3(&a[1])); mk_rot([x[0] - y[0], x[1] - y[1], x[2] - y[2]]) },
        "Object.Multiply_RotatorFloat" => |vm, this, a| { let (r, f) = (rot3(&a[0]), a[1].as_float()); mk_rot(r.map(|c| (c as f32 * f) as i32)) },
        "Object.Multiply_FloatRotator" => |vm, this, a| { let (f, r) = (a[0].as_float(), rot3(&a[1])); mk_rot(r.map(|c| (c as f32 * f) as i32)) },
        "Object.Divide_RotatorFloat" => |vm, this, a| {
            let (r, f) = (rot3(&a[0]), a[1].as_float());
            mk_rot(if f == 0.0 { [0; 3] } else { r.map(|c| (c as f32 / f) as i32) })
        },
        "Object.AddEqual_RotatorRotator" => |vm, this, a| { let (x, y) = (rot3(&a[0]), rot3(&a[1])); a[0] = mk_rot([x[0] + y[0], x[1] + y[1], x[2] + y[2]]); a[0].clone() },
        "Object.SubtractEqual_RotatorRotator" => |vm, this, a| { let (x, y) = (rot3(&a[0]), rot3(&a[1])); a[0] = mk_rot([x[0] - y[0], x[1] - y[1], x[2] - y[2]]); a[0].clone() },
        "Object.MultiplyEqual_RotatorFloat" => |vm, this, a| { let (r, f) = (rot3(&a[0]), a[1].as_float()); a[0] = mk_rot(r.map(|c| (c as f32 * f) as i32)); a[0].clone() },
        "Object.DivideEqual_RotatorFloat" => |vm, this, a| {
            let (r, f) = (rot3(&a[0]), a[1].as_float());
            a[0] = mk_rot(if f == 0.0 { [0; 3] } else { r.map(|c| (c as f32 / f) as i32) });
            a[0].clone()
        },
        "Object.EqualEqual_RotatorRotator" => |vm, this, a| Value::Bool(rot3(&a[0]).map(|c| c & 0xFFFF) == rot3(&a[1]).map(|c| c & 0xFFFF)),
        "Object.NotEqual_RotatorRotator" => |vm, this, a| Value::Bool(rot3(&a[0]).map(|c| c & 0xFFFF) != rot3(&a[1]).map(|c| c & 0xFFFF)),
        "Object.Normalize" => |vm, this, a| mk_rot(rot3(&a[0]).map(|c| { let c = c & 0xFFFF; if c > 32767 { c - 65536 } else { c } })),
        "Object.RotRand" => |vm, this, a| {
            let roll = a.first().is_some_and(Value::as_bool);
            let r = [(frand(vm) * 65536.0) as i32, (frand(vm) * 65536.0) as i32, if roll { (frand(vm) * 65536.0) as i32 } else { 0 }];
            mk_rot(r)
        },
    ];
    list.into_iter().map(|(k, f)| (k.to_ascii_lowercase(), f)).collect()
}
