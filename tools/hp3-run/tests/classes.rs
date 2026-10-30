//! Class loading against the user's own HP3 folder, named by `HP3_DIR`; skipped when it is unset.

use std::path::PathBuf;

#[test]
fn every_class_and_default_decodes() {
    let Some(dir) = std::env::var_os("HP3_DIR").map(PathBuf::from) else {
        eprintln!("HP3_DIR not set; skipping");
        return;
    };
    let set = hp3_package::PackageSet::open(&dir).expect("open HP3 folder");
    let mut db = hp3_script::ClassDb::new(&set);
    db.load_all_classes();
    assert!(db.stats.classes > 1000, "only {} classes", db.stats.classes);
    assert_eq!(db.stats.unknown_tags, 0, "{:?}", db.stats.problems);
    assert_eq!(db.stats.mismatched_tags, 0, "{:?}", db.stats.problems);
    assert_eq!(db.stats.unresolved_objects, 0, "{:?}", db.stats.problems);
}
