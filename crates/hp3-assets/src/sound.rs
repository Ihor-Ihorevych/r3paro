//! Sounds: a file type name (`wav` or `bik`), the embedded file, and HP3's per-sound extras.

use hp3_package::{Error, Package, Reader, Result};

#[derive(Clone, Debug)]
pub struct Sound {
    pub file_type: String,
    pub data_offset: usize,
    pub data_len: usize,
    /// A byte envelope present on some voice and creature sounds (probably amplitude, for mouth movement).
    pub envelope: Option<Vec<u8>>,
    /// Unknown; -1 on most effects, a large positive value on others.
    pub unknown: i32,
}

pub fn read_sound(pkg: &Package, r: &mut Reader<'_>) -> Result<Sound> {
    let file_type = pkg.name(r.ci()? as usize).to_string();
    let data = r.lazy_bytes()?;
    let data_len = data.len();
    let data_offset = r.file_pos() - data_len;
    // Sounds in execdemo.uax end right after the data.
    if r.at_end() {
        return Ok(Sound { file_type, data_offset, data_len, envelope: None, unknown: -1 });
    }
    let envelope = match r.i32()? {
        0 => None,
        1 => Some(r.array(|r| r.u8())?),
        other => return Err(Error::Format(format!("sound envelope flag {other}"))),
    };
    let unknown = r.i32()?;
    Ok(Sound { file_type, data_offset, data_len, envelope, unknown })
}
