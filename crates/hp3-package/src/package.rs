//! One package file: header, name table, import table and export table.

use std::path::{Path, PathBuf};

use crate::reader::Reader;
use crate::{Error, Result};

pub const PACKAGE_TAG: u32 = 0x9E2A_83C1;
/// The file version HP3 writes into every package.
pub const HP3_VERSION: u16 = 129;

/// Reference to an object from inside a package: 0 is none, negative is an import, positive an export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ObjRef(pub i32);

impl ObjRef {
    pub const NONE: ObjRef = ObjRef(0);

    pub fn is_none(self) -> bool {
        self.0 == 0
    }

    pub fn import(self) -> Option<usize> {
        (self.0 < 0).then(|| (-self.0 - 1) as usize)
    }

    pub fn export(self) -> Option<usize> {
        (self.0 > 0).then(|| (self.0 - 1) as usize)
    }

    pub fn from_export(index: usize) -> ObjRef {
        ObjRef(index as i32 + 1)
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub version: u16,
    pub licensee: u16,
    pub flags: u32,
    pub name_count: usize,
    pub name_offset: usize,
    pub export_count: usize,
    pub export_offset: usize,
    pub import_count: usize,
    pub import_offset: usize,
    pub guid: [u8; 16],
    /// (export count, name count) per generation.
    pub generations: Vec<(i32, i32)>,
}

#[derive(Clone, Debug)]
pub struct NameEntry {
    pub name: String,
    pub flags: u32,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub class_package: usize,
    pub class_name: usize,
    pub outer: ObjRef,
    pub name: usize,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub class: ObjRef,
    pub super_: ObjRef,
    pub outer: ObjRef,
    pub name: usize,
    pub flags: u32,
    pub serial_size: usize,
    pub serial_offset: usize,
}

pub struct Package {
    /// File stem, which is the package name objects refer to.
    pub name: String,
    pub path: PathBuf,
    pub data: Vec<u8>,
    pub summary: Summary,
    pub names: Vec<NameEntry>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}

impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Package({})", self.name)
    }
}

impl Package {
    pub fn open(path: &Path) -> Result<Package> {
        let data = std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Self::parse(name, path.to_path_buf(), data)
    }

    pub fn parse(name: String, path: PathBuf, data: Vec<u8>) -> Result<Package> {
        let mut r = Reader::new(&data);
        let tag = r.u32()?;
        if tag != PACKAGE_TAG {
            return Err(Error::Format(format!("not a package (tag {tag:#010x})")));
        }
        let version = r.u16()?;
        let licensee = r.u16()?;
        let flags = r.u32()?;
        let name_count = r.u32()? as usize;
        let name_offset = r.u32()? as usize;
        let export_count = r.u32()? as usize;
        let export_offset = r.u32()? as usize;
        let import_count = r.u32()? as usize;
        let import_offset = r.u32()? as usize;
        let guid = r.guid()?;
        let gen_count = r.u32()? as usize;
        if gen_count > 4096 {
            return Err(Error::Format(format!("bad generation count {gen_count}")));
        }
        let mut generations = Vec::with_capacity(gen_count);
        for _ in 0..gen_count {
            generations.push((r.i32()?, r.i32()?));
        }
        let summary = Summary {
            version,
            licensee,
            flags,
            name_count,
            name_offset,
            export_count,
            export_offset,
            import_count,
            import_offset,
            guid,
            generations,
        };

        r.seek(name_offset)?;
        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            let name = r.fstring()?;
            let flags = r.u32()?;
            names.push(NameEntry { name, flags });
        }

        r.seek(import_offset)?;
        let mut imports = Vec::with_capacity(import_count);
        for _ in 0..import_count {
            imports.push(Import {
                class_package: r.ci()? as usize,
                class_name: r.ci()? as usize,
                outer: ObjRef(r.i32()?),
                name: r.ci()? as usize,
            });
        }

        r.seek(export_offset)?;
        let mut exports = Vec::with_capacity(export_count);
        for _ in 0..export_count {
            let class = ObjRef(r.ci()?);
            let super_ = ObjRef(r.ci()?);
            let outer = ObjRef(r.i32()?);
            let name = r.ci()? as usize;
            let flags = r.u32()?;
            let serial_size = r.ci()? as usize;
            let serial_offset = if serial_size > 0 { r.ci()? as usize } else { 0 };
            if serial_offset + serial_size > data.len() {
                return Err(Error::Format(format!("export {} runs past end of file", exports.len())));
            }
            exports.push(Export { class, super_, outer, name, flags, serial_size, serial_offset });
        }

        let pkg = Package { name, path, data, summary, names, imports, exports };
        pkg.validate()?;
        Ok(pkg)
    }

    fn validate(&self) -> Result<()> {
        let n = self.names.len();
        let check = |idx: usize, what: &str| {
            if idx >= n {
                Err(Error::Format(format!("{what} name index {idx} out of range")))
            } else {
                Ok(())
            }
        };
        for i in &self.imports {
            check(i.class_package, "import class package")?;
            check(i.class_name, "import class")?;
            check(i.name, "import")?;
        }
        for e in &self.exports {
            check(e.name, "export")?;
        }
        Ok(())
    }

    pub fn name(&self, index: usize) -> &str {
        self.names.get(index).map(|n| n.name.as_str()).unwrap_or("<bad name>")
    }

    pub fn find_name(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.name.eq_ignore_ascii_case(name))
    }

    /// Object name of a reference.
    pub fn ref_name(&self, r: ObjRef) -> &str {
        if let Some(i) = r.import() {
            self.imports.get(i).map(|imp| self.name(imp.name)).unwrap_or("<bad import>")
        } else if let Some(e) = r.export() {
            self.exports.get(e).map(|exp| self.name(exp.name)).unwrap_or("<bad export>")
        } else {
            "None"
        }
    }

    pub fn ref_outer(&self, r: ObjRef) -> ObjRef {
        if let Some(i) = r.import() {
            self.imports.get(i).map(|imp| imp.outer).unwrap_or_default()
        } else if let Some(e) = r.export() {
            self.exports.get(e).map(|exp| exp.outer).unwrap_or_default()
        } else {
            ObjRef::NONE
        }
    }

    /// Dotted path inside the package (`Group.Name`), or `Package.Group.Name` for imports.
    pub fn ref_path(&self, r: ObjRef) -> String {
        let mut parts = Vec::new();
        let mut cur = r;
        let mut guard = 0;
        while !cur.is_none() && guard < 64 {
            parts.push(self.ref_name(cur).to_string());
            cur = self.ref_outer(cur);
            guard += 1;
        }
        if r.export().is_some() {
            parts.push(self.name.clone());
        }
        parts.reverse();
        parts.join(".")
    }

    /// Top-level package an import lives in.
    pub fn import_package(&self, index: usize) -> &str {
        let mut cur = ObjRef(-(index as i32) - 1);
        let mut guard = 0;
        loop {
            let outer = self.ref_outer(cur);
            if outer.is_none() || guard > 64 {
                return self.ref_name(cur);
            }
            cur = outer;
            guard += 1;
        }
    }

    /// Name of an export's class (`Class` for class objects, which reference no class).
    pub fn export_class_name(&self, index: usize) -> &str {
        let class = self.exports[index].class;
        if class.is_none() { "Class" } else { self.ref_name(class) }
    }

    /// Package holding the class of an export.
    pub fn export_class_package(&self, index: usize) -> String {
        let class = self.exports[index].class;
        if let Some(i) = class.import() {
            self.import_package(i).to_string()
        } else if class.is_none() {
            "Core".to_string()
        } else {
            self.name.clone()
        }
    }

    pub fn export_data(&self, index: usize) -> &[u8] {
        let e = &self.exports[index];
        &self.data[e.serial_offset..e.serial_offset + e.serial_size]
    }

    pub fn find_export(&self, name: &str, class: Option<&str>) -> Option<usize> {
        self.exports.iter().enumerate().position(|(i, e)| {
            self.name(e.name).eq_ignore_ascii_case(name)
                && class.is_none_or(|c| self.export_class_name(i).eq_ignore_ascii_case(c))
        })
    }

    /// Finds a top-level-or-nested export by its object name and outer chain (`Outer.Name`, package omitted).
    /// A `@Class` suffix restricts the match to exports of that class.
    pub fn find_export_path(&self, path: &str) -> Option<usize> {
        let (path, class) = match path.split_once('@') {
            Some((p, c)) => (p, Some(c)),
            None => (path, None),
        };
        let parts: Vec<&str> = path.split('.').collect();
        (0..self.exports.len()).find(|&i| {
            if let Some(c) = class
                && !self.export_class_name(i).eq_ignore_ascii_case(c)
            {
                return false;
            }
            let mut cur = ObjRef::from_export(i);
            for part in parts.iter().rev() {
                if cur.is_none() || !self.ref_name(cur).eq_ignore_ascii_case(part) {
                    return false;
                }
                cur = self.ref_outer(cur);
            }
            cur.is_none()
        })
    }
}
