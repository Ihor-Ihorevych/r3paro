//! Whole-install check: reads every export of every package and reports what could not be read exactly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::object::{self, Body};
use crate::set::PackageSet;

#[derive(Default, Debug)]
pub struct ClassStats {
    pub exports: usize,
    pub bytes: usize,
    /// Exports whose decoded data stopped short of the serial size.
    pub leftover_exports: usize,
    pub leftover_bytes: usize,
    pub errors: usize,
    pub first_problem: Option<String>,
}

#[derive(Default, Debug)]
pub struct Report {
    pub packages: usize,
    pub package_errors: Vec<(PathBuf, String)>,
    pub exports: usize,
    pub decoded: usize,
    /// Keyed by the native class that decided the layout, then the export's own class.
    pub by_class: BTreeMap<String, ClassStats>,
    /// Native classes whose data is not decoded yet: export count and bytes.
    pub unparsed: BTreeMap<String, (usize, usize)>,
    pub scripts: usize,
    pub script_bytes: u64,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.by_class.values().map(|s| s.errors).sum::<usize>() + self.package_errors.len()
    }

    pub fn leftover_exports(&self) -> usize {
        self.by_class.values().map(|s| s.leftover_exports).sum()
    }
}

/// Decodes the native data of a class from a reader over it; `None` when the layout is not known.
pub type NativeDecoder<'d> = &'d dyn Fn(&str, &crate::Package, &mut crate::Reader<'_>) -> Option<crate::Result<()>>;

pub fn check(
    set: &PackageSet,
    filter: Option<&str>,
    native: NativeDecoder<'_>,
    mut progress: impl FnMut(&str),
) -> Report {
    let mut rep = Report::default();
    for path in set.files() {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(f) = filter
            && !stem.eq_ignore_ascii_case(f)
        {
            continue;
        }
        progress(&stem);
        let pkg = match set.get(&stem) {
            Ok(p) => p,
            Err(e) => {
                rep.package_errors.push((path.clone(), e.to_string()));
                continue;
            }
        };
        rep.packages += 1;
        for i in 0..pkg.exports.len() {
            rep.exports += 1;
            let chain = set.export_chain(&pkg, i);
            let size = pkg.exports[i].serial_size;
            let result = object::read_export(&pkg, i, &chain);
            let key = match &result {
                Ok(o) => format!("{} ({})", o.native_class, chain[0]),
                Err(_) => format!("? ({})", chain[0]),
            };
            let st = rep.by_class.entry(key).or_default();
            st.exports += 1;
            st.bytes += size;
            let where_ = || format!("{}:{} [{}]", pkg.name, pkg.ref_path(crate::ObjRef::from_export(i)), i);
            match result {
                Err(e) => {
                    st.errors += 1;
                    if st.first_problem.is_none() {
                        st.first_problem = Some(format!("{}: {e}", where_()));
                    }
                }
                Ok(o) => {
                    if let Body::Class { structure, .. }
                    | Body::State { structure, .. }
                    | Body::Function { structure, .. }
                    | Body::Struct { structure, .. } = &o.body
                    {
                        rep.scripts += 1;
                        rep.script_bytes += structure.script_size as u64;
                    }
                    if matches!(o.body, Body::Unparsed) {
                        let base = pkg.exports[i].serial_offset + size - o.tail.len();
                        let mut r = crate::Reader::with_base(o.tail, base);
                        match native(o.native_class, &pkg, &mut r) {
                            None => {
                                let u = rep.unparsed.entry(o.native_class.to_string()).or_default();
                                u.0 += 1;
                                u.1 += o.tail.len();
                            }
                            Some(Err(e)) => {
                                st.errors += 1;
                                if st.first_problem.is_none() {
                                    st.first_problem = Some(format!("{}: {e}", where_()));
                                }
                            }
                            Some(Ok(())) if r.at_end() => rep.decoded += 1,
                            Some(Ok(())) => {
                                st.leftover_exports += 1;
                                st.leftover_bytes += r.remaining();
                                if st.first_problem.is_none() {
                                    st.first_problem = Some(format!(
                                        "{}: {} native bytes left at export offset {:#x}",
                                        where_(),
                                        r.remaining(),
                                        r.file_pos() - pkg.exports[i].serial_offset
                                    ));
                                }
                            }
                        }
                    } else if !o.tail.is_empty() {
                        st.leftover_exports += 1;
                        st.leftover_bytes += o.tail.len();
                        if st.first_problem.is_none() {
                            st.first_problem = Some(format!("{}: {} bytes left", where_(), o.tail.len()));
                        }
                    } else {
                        rep.decoded += 1;
                    }
                }
            }
        }
        set.unload_all();
    }
    rep
}
