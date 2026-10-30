//! Runs a few of HP3's own script functions; needs `HP3_DIR` and is skipped without it.

use std::path::PathBuf;

use hp3_script::{ClassDb, Value, Vm};

fn vm_for(dir: &std::path::Path) -> Option<(hp3_package::PackageSet,)> {
    Some((hp3_package::PackageSet::open(dir).ok()?,))
}

fn call(vm: &mut Vm<'_>, class: &str, func: &str, mut args: Vec<Value>) -> (Value, Vec<Value>) {
    let c = vm.db.find(class).expect("class");
    let n = vm.db.names.find(func).expect("function name");
    let f = vm.find_function(None, Some(c), n).expect("function");
    let name = vm.db.names.intern("Test");
    let obj = vm.new_object(c, name);
    let r = vm.invoke(f, Some(obj), &mut args).expect("call succeeds");
    (r, args)
}

#[test]
fn script_functions_run() {
    let Some(dir) = std::env::var_os("HP3_DIR").map(PathBuf::from) else {
        eprintln!("HP3_DIR not set; skipping");
        return;
    };
    let (set,) = vm_for(&dir).expect("open HP3 folder");
    let mut db = ClassDb::new(&set);
    db.load_all_classes();
    let mut vm = Vm::new(db);
    let (_, args) = call(
        &mut vm,
        "Actor",
        "ReplaceText",
        vec![Value::Str("Hello Harry and Harry".into()), Value::Str("Harry".into()), Value::Str("Ron".into())],
    );
    assert_eq!(args[0], Value::Str("Hello Ron and Ron".into()));
    let (r, _) = call(&mut vm, "Actor", "Vec", vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    assert_eq!(r, Value::Struct(Box::new([Value::Float(1.0), Value::Float(2.0), Value::Float(3.0)])));
}
