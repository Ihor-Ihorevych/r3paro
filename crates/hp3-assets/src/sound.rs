//! Sounds: a file type name (`wav` or `bik`), the embedded file, and HP3's per-sound extras.

use hp3_package::{Error, Package, Reader, Result};

/// Data that drives a speaker's face while the sound plays.
#[derive(Clone, Debug)]
pub enum Envelope {
    None,
    /// A byte per step, on creature and effect sounds (probably amplitude).
    Bytes(Vec<u8>),
    /// Four header values and float keys, on dialog lines (probably lip-sync curves).
    Curves { header: [i32; 4], values: Vec<f32> },
}

#[derive(Clone, Debug)]
pub struct Sound {
    pub file_type: String,
    pub data_offset: usize,
    pub data_len: usize,
    pub envelope: Envelope,
    /// Unknown; -1 on most effects, a large positive value on others.
    pub unknown: i32,
}

/// Lowest package version seen with the envelope and trailing value; every HP3 sound package is 129.
const EXTRAS_VERSION: u16 = 127;

pub fn read_sound(pkg: &Package, r: &mut Reader<'_>) -> Result<Sound> {
    let file_type = pkg.name(r.ci()? as usize).to_string();
    let data = r.lazy_bytes()?;
    let data_len = data.len();
    let data_offset = r.file_pos() - data_len;
    // Older packages (execdemo.uax is version 126) end right after the data.
    if pkg.summary.version < EXTRAS_VERSION {
        return Ok(Sound { file_type, data_offset, data_len, envelope: Envelope::None, unknown: -1 });
    }
    let envelope = match r.i32()? {
        0 => Envelope::None,
        1 => Envelope::Bytes(r.array(|r| r.u8())?),
        2 => Envelope::Curves {
            header: [r.i32()?, r.i32()?, r.i32()?, r.i32()?],
            values: r.array(|r| r.f32())?,
        },
        other => return Err(Error::Format(format!("sound envelope flag {other}"))),
    };
    let unknown = r.i32()?;
    Ok(Sound { file_type, data_offset, data_len, envelope, unknown })
}
