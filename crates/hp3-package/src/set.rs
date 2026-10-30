//! Finds packages by name in an HP3 install and resolves class hierarchies across packages.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::object::RF_HAS_STACK;
use crate::package::{ObjRef, Package};
use crate::reader::Reader;
use crate::{Error, Result};

/// Folders of an HP3 install that hold packages.
pub const PACKAGE_DIRS: &[&str] = &["System", "Maps", "Textures", "StaticMeshes", "Animations", "Sounds", "Music"];
pub const PACKAGE_EXTS: &[&str] = &["u", "unr", "utx", "usx", "ukx", "uax", "umx"];

/// Classes implemented entirely in native code: they have no class object in any package. Returns the parent
/// class, `None` for `Object` and for unknown classes.
pub fn builtin_super(class: &str) -> Option<&'static str> {
    let c = class.to_ascii_lowercase();
    Some(match c.as_str() {
        "object" => return None,
        "field" | "textbuffer" | "package" | "linker" | "linkerload" | "linkersave" | "metadata" => "Object",
        "struct" | "enum" | "const" | "property" => "Field",
        "state" | "function" => "Struct",
        "class" => "State",
        "classproperty" => "ObjectProperty",
        "byteproperty" | "intproperty" | "boolproperty" | "floatproperty" | "objectproperty" | "nameproperty"
        | "strproperty" | "stringproperty" | "arrayproperty" | "structproperty" | "mapproperty"
        | "fixedarrayproperty" | "delegateproperty" | "pointerproperty" => "Property",
        "model" | "staticmesh" | "mesh" | "convexvolume" | "terrainprimitive" | "fluidsurfaceprimitive"
        | "projectorprimitive" => "Primitive",
        "primitive" | "polys" | "levelbase" | "meshanimation" | "terrainsector" | "meshinstance" | "kmeshprops" => {
            "Object"
        }
        "level" | "pendinglevel" => "LevelBase",
        "lodmesh" => "Mesh",
        "skeletalmesh" | "vertmesh" => "LodMesh",
        "staticmeshinstance" | "sound" | "music" | "font" => "Object",
        _ => return None,
    })
}

fn canonical_builtin(name: &str) -> String {
    const NAMES: &[&str] = &[
        "Object", "Field", "TextBuffer", "Package", "Struct", "Enum", "Const", "Property", "State", "Function",
        "Class", "ObjectProperty", "Primitive", "LevelBase", "Mesh", "LodMesh",
    ];
    NAMES.iter().find(|n| n.eq_ignore_ascii_case(name)).map(|n| n.to_string()).unwrap_or_else(|| name.to_string())
}

pub struct PackageSet {
    root: PathBuf,
    /// Language whose localized packages are used: `int` when present, else the only one installed.
    pub language: String,
    files: HashMap<String, PathBuf>,
    loaded: RefCell<HashMap<String, Rc<Package>>>,
    chains: RefCell<HashMap<String, Rc<Vec<String>>>>,
}

impl PackageSet {
    /// Indexes every package file under the install's package folders.
    pub fn open(root: &Path) -> Result<PackageSet> {
        let mut files = HashMap::new();
        let mut localized: HashMap<String, HashMap<String, PathBuf>> = HashMap::new();
        let entries = std::fs::read_dir(root).map_err(|e| Error::Io(root.to_path_buf(), e))?;
        for entry in entries.flatten() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if !PACKAGE_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&dir_name)) || !entry.path().is_dir() {
                continue;
            }
            let Ok(inner) = std::fs::read_dir(entry.path()) else { continue };
            for f in inner.flatten() {
                let p = f.path();
                let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()) else { continue };
                if PACKAGE_EXTS.contains(&ext.as_str()) {
                    files.entry(stem).or_insert(p);
                } else if let Some((lang, base)) = ext.split_once('_')
                    && lang.len() == 3
                    && PACKAGE_EXTS.contains(&base)
                {
                    // Localized packages: `<name>.<language>_<extension>`, e.g. `alldialog.rus_uax`.
                    localized.entry(lang.to_string()).or_default().insert(stem, p);
                }
            }
        }
        let language = if localized.contains_key("int") || localized.len() != 1 {
            "int".to_string()
        } else {
            localized.keys().next().cloned().unwrap_or_else(|| "int".to_string())
        };
        if let Some(loc) = localized.remove(&language) {
            files.extend(loc);
        }
        Ok(PackageSet {
            root: root.to_path_buf(),
            language,
            files,
            loaded: RefCell::new(HashMap::new()),
            chains: RefCell::new(HashMap::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All package files, sorted by path.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.files.values().cloned().collect();
        v.sort();
        v
    }

    pub fn path_of(&self, name: &str) -> Option<&Path> {
        self.files.get(&name.to_ascii_lowercase()).map(|p| p.as_path())
    }

    pub fn get(&self, name: &str) -> Result<Rc<Package>> {
        let key = name.to_ascii_lowercase();
        if let Some(p) = self.loaded.borrow().get(&key) {
            return Ok(p.clone());
        }
        let path = self.files.get(&key).ok_or_else(|| Error::MissingPackage(name.to_string()))?;
        let pkg = Rc::new(Package::open(path)?);
        self.loaded.borrow_mut().insert(key, pkg.clone());
        Ok(pkg)
    }

    /// Releases loaded packages (class chains stay cached).
    pub fn unload_all(&self) {
        self.loaded.borrow_mut().clear();
    }

    /// Package and export index an object reference of `pkg` points to, following imports into other packages.
    pub fn resolve(&self, pkg: &Rc<Package>, r: ObjRef) -> Option<(Rc<Package>, usize)> {
        if let Some(e) = r.export() {
            return (e < pkg.exports.len()).then(|| (pkg.clone(), e));
        }
        let i = r.import()?;
        let imp = pkg.imports.get(i)?;
        let class = pkg.name(imp.class_name).to_string();
        let mut parts = Vec::new();
        let mut cur = r;
        while !cur.is_none() {
            parts.push(pkg.ref_name(cur).to_string());
            cur = pkg.ref_outer(cur);
            if parts.len() > 32 {
                return None;
            }
        }
        let top = parts.pop()?;
        parts.reverse();
        let target = self.get(&top).ok()?;
        let path = format!("{}@{class}", parts.join("."));
        let index = target.find_export_path(&path).or_else(|| target.find_export_path(&parts.join(".")))?;
        Some((target, index))
    }

    /// Class hierarchy of an export, most derived first, ending in `Object`.
    pub fn export_chain(&self, pkg: &Package, index: usize) -> Rc<Vec<String>> {
        let class = pkg.exports[index].class;
        if class.is_none() {
            return self.class_chain("Core", "Class", None);
        }
        let class_pkg = pkg.export_class_package(index);
        let class_name = pkg.ref_name(class).to_string();
        let local = (class.export().is_some()).then_some(pkg);
        self.class_chain(&class_pkg, &class_name, local)
    }

    /// Class hierarchy of `package.class`. `local` is used when the class lives in a package being read.
    pub fn class_chain(&self, package: &str, class: &str, local: Option<&Package>) -> Rc<Vec<String>> {
        let key = format!("{}.{}", package.to_ascii_lowercase(), class.to_ascii_lowercase());
        if let Some(c) = self.chains.borrow().get(&key) {
            return c.clone();
        }
        let mut chain = vec![class.to_string()];
        match self.find_super(package, class, local) {
            Some((super_pkg, super_name)) => {
                let rest = self.class_chain(&super_pkg, &super_name, None);
                chain.extend(rest.iter().cloned());
            }
            None => {
                let mut cur = class.to_string();
                while let Some(s) = builtin_super(&cur) {
                    chain.push(s.to_string());
                    cur = s.to_string();
                }
                chain[0] = canonical_builtin(&chain[0]);
            }
        }
        let chain = Rc::new(chain);
        self.chains.borrow_mut().insert(key, chain.clone());
        chain
    }

    /// Package and name of a class's super class, read from its class object.
    fn find_super(&self, package: &str, class: &str, local: Option<&Package>) -> Option<(String, String)> {
        let owned;
        let pkg: &Package = match local {
            Some(p) if p.name.eq_ignore_ascii_case(package) => p,
            _ => {
                owned = self.get(package).ok()?;
                &owned
            }
        };
        let index = pkg.exports.iter().position(|e| e.class.is_none() && pkg.name(e.name).eq_ignore_ascii_case(class))?;
        let exp = &pkg.exports[index];
        let mut r = Reader::with_base(pkg.export_data(index), exp.serial_offset);
        if exp.flags & RF_HAS_STACK != 0 {
            crate::object::read_state_frame(&mut r).ok()?;
        }
        let super_ref = ObjRef(r.ci().ok()?);
        if super_ref.is_none() {
            // Object itself.
            return builtin_super(class).map(|s| ("Core".to_string(), s.to_string()));
        }
        let name = pkg.ref_name(super_ref).to_string();
        let owner = match super_ref.import() {
            Some(i) => pkg.import_package(i).to_string(),
            None => pkg.name.clone(),
        };
        Some((owner, name))
    }
}
