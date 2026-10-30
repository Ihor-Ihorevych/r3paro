//! Reader for the package files of *Harry Potter and the Prisoner of Azkaban* (file version 129).
//!
//! `Package` parses the header and the name, import and export tables. `PackageSet` finds packages by name in
//! the game folders and resolves class hierarchies across them. `object` deserializes the exports themselves.

pub mod bytecode;
pub mod check;
pub mod object;
pub mod package;
pub mod props;
pub mod reader;
pub mod set;

use std::path::PathBuf;

pub use package::{Export, Import, ObjRef, Package};
pub use reader::Reader;
pub use set::builtin_super;
pub use set::PackageSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("unexpected end of data at {at:#x} (needed {need} bytes)")]
    Eof { at: usize, need: usize },
    #[error("{0}")]
    Format(String),
    #[error("package not found: {0}")]
    MissingPackage(String),
}

pub type Result<T> = std::result::Result<T, Error>;
