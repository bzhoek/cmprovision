//! Reads and replaces the embedded `bootconf.txt` of a Raspberry Pi 4 bootloader image.
//!
//! The image is a chain of 8-byte aligned sections, each starting with a big-endian
//! magic and length. File sections carry a 12-byte name and 4 reserved bytes before
//! their contents.

use thiserror::Error;

const MAGIC: u32 = 0x55aa_f00f;
const MAGIC_MASK: u32 = 0xffff_f00f;
const FILE_MAGIC: u32 = 0x55aa_f11f;
const FILE_HDR_LEN: usize = 20;
const FILENAME_LEN: usize = 12;
pub const MAX_BOOTCONF_SIZE: usize = 2024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EepromError {
    #[error("EEPROM image is corrupt")]
    Corrupt,
    #[error("EEPROM image has no bootconf.txt")]
    NoBootconf,
    #[error("EEPROM settings exceed {MAX_BOOTCONF_SIZE} bytes")]
    SettingsTooLarge,
}

struct Section {
    offset: usize,
    len: usize,
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

fn find_bootconf(data: &[u8]) -> Result<Section, EepromError> {
    let mut offset = 0;
    while offset + 8 < data.len() {
        let magic = read_u32(data, offset).ok_or(EepromError::Corrupt)?;
        let len = read_u32(data, offset + 4).ok_or(EepromError::Corrupt)? as usize;
        if magic & MAGIC_MASK != MAGIC {
            return Err(EepromError::Corrupt);
        }
        if magic == FILE_MAGIC {
            let name = &data[offset + 8..(offset + 8 + FILE_HDR_LEN).min(data.len())];
            if name.starts_with(b"bootconf.txt\0") {
                return Ok(Section { offset, len });
            }
        }
        offset = (offset + 8 + len + 7) & !7;
    }
    Err(EepromError::NoBootconf)
}

pub fn settings(data: &[u8]) -> Result<&[u8], EepromError> {
    let section = find_bootconf(data)?;
    let len = section.len.checked_sub(FILENAME_LEN + 4).ok_or(EepromError::Corrupt)?;
    let start = section.offset + 4 + FILE_HDR_LEN;
    data.get(start..start + len).ok_or(EepromError::Corrupt)
}

pub fn set_settings(data: &mut [u8], settings: &[u8]) -> Result<(), EepromError> {
    if settings.len() > MAX_BOOTCONF_SIZE {
        return Err(EepromError::SettingsTooLarge);
    }
    let section = find_bootconf(data)?;
    let start = section.offset + 4 + FILE_HDR_LEN;
    let contents = data.get_mut(start..start + MAX_BOOTCONF_SIZE).ok_or(EepromError::Corrupt)?;
    contents.fill(0xff);
    contents[..settings.len()].copy_from_slice(settings);

    let len = (settings.len() + FILENAME_LEN + 4) as u32;
    data[section.offset + 4..section.offset + 8].copy_from_slice(&len.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: &[u8] = include_bytes!("../tests/fixtures/eeprom/pieeprom.bin");

    #[test]
    fn reads_bootconf_like_php() {
        let expected = include_bytes!("../tests/fixtures/eeprom/bootconf.txt");
        assert_eq!(settings(IMAGE).unwrap(), expected);
    }

    #[test]
    fn patches_bootconf_like_php() {
        let mut image = IMAGE.to_vec();
        set_settings(&mut image, include_bytes!("../tests/fixtures/eeprom/patched-settings.txt")).unwrap();
        assert!(image == include_bytes!("../tests/fixtures/eeprom/pieeprom-patched.bin"));
    }

    #[test]
    fn patched_settings_read_back() {
        let mut image = IMAGE.to_vec();
        set_settings(&mut image, b"[all]\nBOOT_ORDER=0xf1\n").unwrap();
        assert_eq!(settings(&image).unwrap(), b"[all]\nBOOT_ORDER=0xf1\n");
    }

    #[test]
    fn rejects_oversized_settings() {
        let mut image = IMAGE.to_vec();
        let settings = vec![b'x'; MAX_BOOTCONF_SIZE + 1];
        assert_eq!(set_settings(&mut image, &settings), Err(EepromError::SettingsTooLarge));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(settings(&[0u8; 64]).err(), Some(EepromError::Corrupt));
        assert_eq!(settings(&[]).err(), Some(EepromError::NoBootconf));
    }
}
