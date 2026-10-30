//! `config` properties: class defaults read from the ini files, and text to property values (also used by
//! `SetPropertyText`).
//!
//! Each class reads its config properties from its own `[Package.Class]` section; a key it does not have
//! keeps the value its parent class read. `globalconfig` properties are read from the declaring class's
//! section. Static arrays use `Key[i]=`, dynamic arrays repeat the key. Enum values given by name are not
//! understood yet (the default stays).

use hp3_core::Config;
use hp3_script::class_db::{PropType, StructKind};
use hp3_script::{StructId, Value, Vm};

const CPF_CONFIG: u32 = 0x0000_4000;
const CPF_GLOBAL_CONFIG: u32 = 0x0004_0000;

/// Splits `a,b,(c,d),"e,f"` at top-level commas.
fn split_top(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut quoted, mut start) = (Vec::new(), 0i32, false, 0);
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s)
}

/// The number at the start of `s` (sign, digits, one decimal point, exponent), if any.
fn leading_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
        end += 1;
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    while end < b.len() {
        match b[end] {
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot => seen_dot = true,
            _ => break,
        }
        end += 1;
    }
    if !seen_digit {
        return None;
    }
    s[..end].parse().ok()
}

/// A property value from its text form; `None` when the text does not fit the type.
pub fn parse(vm: &mut Vm<'_>, ty: &PropType, text: &str) -> Option<Value> {
    let t = text.trim();
    Some(match ty {
        // Numbers are read from the front, the rest ignored (as C's atoi/atof do).
        PropType::Int => Value::Int(leading_number(t)? as i32),
        PropType::Float => Value::Float(leading_number(t)? as f32),
        PropType::Byte => Value::Byte(leading_number(t)? as u8),
        PropType::Bool => match t.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Value::Bool(true),
            "false" | "0" | "no" | "off" | "" => Value::Bool(false),
            _ => return None,
        },
        // A name is one token: it ends at whitespace or a separator.
        PropType::Name => {
            let u = unquote(t);
            let end = u.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(u.len());
            Value::Name(vm.db.names.intern(&u[..end]))
        }
        PropType::Str => Value::Str(unquote(t).to_string()),
        PropType::Object | PropType::Class => {
            // `Class'Package.Name'` or a bare path.
            let path = match (t.find('\''), t.rfind('\'')) {
                (Some(a), Some(b)) if b > a => &t[a + 1..b],
                _ => unquote(t),
            };
            if path.is_empty() || path.eq_ignore_ascii_case("none") {
                Value::Object(None)
            } else {
                Value::Object(Some(crate::find_object(vm, path)?))
            }
        }
        PropType::Struct(id) => {
            let inner = t.strip_prefix('(').and_then(|x| x.strip_suffix(')'))?;
            let mut fields = vm.db.get(*id).defaults.clone();
            for part in split_top(inner) {
                let Some((k, v)) = part.split_once('=') else { continue };
                let Some(n) = vm.db.names.find(k.trim()) else { continue };
                let Some(prop) = vm.db.get(*id).prop(n).cloned() else { continue };
                if let Some(val) = parse(vm, &prop.ty, v)
                    && let Some(slot) = fields.get_mut(prop.slot)
                {
                    *slot = val;
                }
            }
            Value::Struct(fields.into())
        }
        PropType::Array(inner) | PropType::FixedArray(inner, _) => {
            let inner_text = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(t);
            Value::Array(split_top(inner_text).into_iter().filter_map(|x| parse(vm, inner, x)).collect())
        }
        PropType::Pointer | PropType::Map | PropType::Delegate => return None,
    })
}

fn depth(vm: &Vm<'_>, mut c: StructId) -> usize {
    let mut d = 0;
    while let Some(s) = vm.db.get(c).super_ {
        c = s;
        d += 1;
    }
    d
}

fn section(vm: &Vm<'_>, class: StructId) -> String {
    let def = vm.db.get(class);
    format!("{}.{}", def.pkg.name, vm.db.names.get(def.name))
}

/// Reads every loaded class's config properties into its defaults. Returns how many values were set.
pub fn apply(vm: &mut Vm<'_>, config: &Config) -> usize {
    let mut classes: Vec<StructId> =
        (0..vm.db.structs.len()).map(|i| StructId(i as u32)).filter(|&c| vm.db.get(c).kind == StructKind::Class).collect();
    // Parents first, so a class missing a key inherits what its parent read.
    classes.sort_by_key(|&c| depth(vm, c));
    let mut set = 0;
    for class in classes {
        let props: Vec<_> = vm.db.get(class).props.iter().filter(|p| p.flags & (CPF_CONFIG | CPF_GLOBAL_CONFIG) != 0).cloned().collect();
        if props.is_empty() {
            continue;
        }
        let own_section = section(vm, class);
        for prop in props {
            let sec = if prop.flags & CPF_GLOBAL_CONFIG != 0 { section(vm, prop.owner) } else { own_section.clone() };
            let key = vm.db.names.get(prop.name).to_string();
            // Inherited first: the parent's (already configured) value.
            if let Some(parent) = vm.db.get(class).super_
                && vm.db.get(parent).prop(prop.name).is_some()
            {
                for i in 0..prop.dim {
                    let v = vm.db.get(parent).defaults.get(prop.slot + i).cloned();
                    if let Some(v) = v {
                        vm.db.structs[class.0 as usize].defaults[prop.slot + i] = v;
                    }
                }
            }
            let mut values: Vec<(usize, Value)> = Vec::new();
            match &prop.ty {
                PropType::Array(inner) => {
                    let texts: Vec<String> = config.get_all(&sec, &key).into_iter().map(str::to_string).collect();
                    if !texts.is_empty() {
                        let items: Vec<Value> = texts.iter().filter_map(|t| parse(vm, inner, t)).collect();
                        values.push((0, Value::Array(items)));
                    }
                }
                ty => {
                    for i in 0..prop.dim {
                        let text = if prop.dim > 1 {
                            config.get(&sec, &format!("{key}[{i}]")).or_else(|| (i == 0).then(|| config.get(&sec, &key)).flatten())
                        } else {
                            config.get(&sec, &key)
                        };
                        if let Some(text) = text.map(str::to_string)
                            && let Some(v) = parse(vm, ty, &text)
                        {
                            values.push((i, v));
                        }
                    }
                }
            }
            for (i, v) in values {
                if let Some(slot) = vm.db.structs[class.0 as usize].defaults.get_mut(prop.slot + i) {
                    *slot = v;
                    set += 1;
                }
            }
        }
    }
    set
}
