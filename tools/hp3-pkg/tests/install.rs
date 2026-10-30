//! Whole-install check against the user's own HP3 folder, named by `HP3_DIR`; skipped when it is unset.

use std::path::PathBuf;

#[test]
fn every_export_decodes_exactly() {
    let Some(dir) = std::env::var_os("HP3_DIR").map(PathBuf::from) else {
        eprintln!("HP3_DIR not set; skipping");
        return;
    };
    let set = hp3_package::PackageSet::open(&dir).expect("open HP3 folder");
    let decode = |class: &str, pkg: &hp3_package::Package, r: &mut hp3_package::Reader<'_>| {
        hp3_assets::read_native(class, pkg, r).map(|res| res.map(|_| ()))
    };
    let rep = hp3_package::check::check(&set, None, &decode, |_| {});
    assert!(rep.packages > 0, "no packages found under {}", dir.display());
    assert_eq!(rep.errors(), 0, "decode errors");
    assert_eq!(rep.leftover_exports(), 0, "exports with leftover bytes");
    assert!(rep.unparsed.is_empty(), "classes without a decoder: {:?}", rep.unparsed.keys().collect::<Vec<_>>());
}
