//! Bootloader images from the raspberrypi/rpi-eeprom repository, stored per release channel.

use std::fs;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;

pub const ZIPBALL_URL: &str = "https://api.github.com/repos/raspberrypi/rpi-eeprom/zipball";

/// Pi 4 / CM4 bootloaders; firmware-2712 is for the Pi 5, which scriptexecute cannot boot.
const FIRMWARE_DIR: &str = "firmware-2711/";

static IMAGE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^pieeprom-.+\.bin$").unwrap());
static CHANNEL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").unwrap());

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firmware {
    pub channel: String,
    pub name: String,
    /// `channel/name`, as stored in a project's `eeprom_firmware`.
    pub path: String,
}

/// Images in the real channel directories, newest first. Symlinked channels such as
/// `stable` are aliases of another channel and would only list the same images twice.
pub fn list(dir: &Path) -> Vec<Firmware> {
    let mut channels: Vec<String> = read_dir_names(dir)
        .into_iter()
        .filter(|name| CHANNEL_NAME.is_match(name))
        .filter(|name| fs::symlink_metadata(dir.join(name)).is_ok_and(|m| m.is_dir()))
        .collect();
    channels.sort();

    channels.into_iter().flat_map(|channel| list_channel(dir, &channel)).collect()
}

pub fn list_channel(dir: &Path, channel: &str) -> Vec<Firmware> {
    let mut names: Vec<String> = read_dir_names(&dir.join(channel))
        .into_iter()
        .filter(|name| IMAGE_NAME.is_match(name))
        .collect();
    names.sort_by(|a, b| b.cmp(a));

    names
        .into_iter()
        .map(|name| Firmware { path: format!("{channel}/{name}"), channel: channel.to_string(), name })
        .collect()
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default()
}

/// The file for a project's `channel/name` reference, which may go through a channel alias.
pub fn resolve(dir: &Path, path: &str) -> Option<PathBuf> {
    let (channel, name) = path.split_once('/')?;
    if !CHANNEL_NAME.is_match(channel) || !IMAGE_NAME.is_match(name) || name.contains('/') {
        return None;
    }
    let file = dir.join(channel).join(name);
    file.is_file().then_some(file)
}

/// Extracts the Pi 4 firmware tree of a GitHub zipball of rpi-eeprom into `dest`.
pub fn extract_zipball(reader: impl Read + Seek, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(reader).context("Error opening .zip file")?;
    let root = zip.by_index(0).context("Error listing .zip file")?.name()?.into_owned();
    let prefix = format!("{root}{FIRMWARE_DIR}");
    fs::create_dir_all(dest)?;

    for i in 1..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let Some(relative) = name.strip_prefix(&prefix).map(PathBuf::from) else {
            continue;
        };
        if relative.as_os_str().is_empty() || entry.enclosed_name().is_none() || !is_plain(&relative) {
            continue;
        }
        let target = dest.join(&relative);

        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.is_symlink() {
            let mut link = String::new();
            entry.read_to_string(&mut link)?;
            replace_with_symlink(&link, &target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents)?;
            fs::write(&target, contents).with_context(|| format!("writing {}", target.display()))?;
        }
    }
    Ok(())
}

fn is_plain(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn replace_with_symlink(link: &str, target: &Path) -> Result<()> {
    let link = link.trim_end_matches('/');
    if !is_plain(Path::new(link)) || link.contains('/') {
        tracing::warn!("skipping symlink {} -> {link} pointing outside its channel", target.display());
        return Ok(());
    }
    match fs::symlink_metadata(target) {
        // Installations from before the rpi-eeprom layout change have real stable/beta
        // directories; keep them so projects referencing their images stay valid.
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => fs::remove_file(target)?,
        Err(_) => {}
    }
    std::os::unix::fs::symlink(link, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use super::*;

    fn zipball() -> Cursor<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let root = "raspberrypi-rpi-eeprom-abc123/";
        zip.add_directory(root, options).unwrap();
        zip.add_directory(format!("{root}firmware-2711/"), options).unwrap();
        zip.add_directory(format!("{root}firmware-2711/latest/"), options).unwrap();
        for name in ["pieeprom-2026-09-23.bin", "pieeprom-2026-10-15.bin", "recovery.bin"] {
            zip.start_file(format!("{root}firmware-2711/latest/{name}"), options).unwrap();
            zip.write_all(name.as_bytes()).unwrap();
        }
        zip.add_directory(format!("{root}firmware-2711/default/"), options).unwrap();
        zip.start_file(format!("{root}firmware-2711/default/pieeprom-2026-05-17.bin"), options).unwrap();
        zip.write_all(b"default").unwrap();
        zip.add_symlink(format!("{root}firmware-2711/stable"), "latest/", options).unwrap();
        zip.add_symlink(format!("{root}firmware-2711/escape"), "../../etc", options).unwrap();
        zip.start_file(format!("{root}firmware-2712/latest/pieeprom-2026-10-15.bin"), options).unwrap();
        zip.write_all(b"pi5").unwrap();
        zip.start_file(format!("{root}README.md"), options).unwrap();
        Cursor::new(zip.finish().unwrap().into_inner())
    }

    #[test]
    fn extracts_pi4_tree_with_channel_aliases() {
        let dir = tempfile::tempdir().unwrap();
        extract_zipball(zipball(), dir.path()).unwrap();

        assert_eq!(fs::read(dir.path().join("latest/pieeprom-2026-10-15.bin")).unwrap(), b"pieeprom-2026-10-15.bin");
        assert_eq!(fs::read_link(dir.path().join("stable")).unwrap(), Path::new("latest"));
        assert!(!dir.path().join("escape").exists());
        assert!(!dir.path().join("README.md").exists());
    }

    fn installed() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in ["latest/pieeprom-2026-09-23.bin", "latest/pieeprom-2026-10-15.bin", "latest/recovery.bin", "default/pieeprom-2026-05-17.bin"] {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"x").unwrap();
        }
        std::os::unix::fs::symlink("latest", dir.path().join("stable")).unwrap();
        dir
    }

    #[test]
    fn lists_real_channels_newest_first() {
        let dir = installed();
        let paths: Vec<String> = list(dir.path()).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["default/pieeprom-2026-05-17.bin", "latest/pieeprom-2026-10-15.bin", "latest/pieeprom-2026-09-23.bin"]);
    }

    #[test]
    fn resolves_legacy_channel_through_alias() {
        let dir = installed();
        assert!(resolve(dir.path(), "stable/pieeprom-2026-10-15.bin").is_some());
        assert!(resolve(dir.path(), "beta/pieeprom-2026-10-15.bin").is_none());
        assert!(resolve(dir.path(), "../latest/pieeprom-2026-10-15.bin").is_none());
        assert!(resolve(dir.path(), "latest/recovery.bin").is_none());
    }

    #[test]
    fn keeps_legacy_channel_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("stable")).unwrap();
        fs::write(dir.path().join("stable/pieeprom-2021-01-01.bin"), b"old").unwrap();

        extract_zipball(zipball(), dir.path()).unwrap();

        assert!(resolve(dir.path(), "stable/pieeprom-2021-01-01.bin").is_some());
    }
}
