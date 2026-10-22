//! Compressed disk images: upload, deletion and the background job computing the
//! hashes the provisioning script verifies against.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use axum::extract::Multipart;
use axum::extract::multipart::Field;
use rand::distr::{Alphanumeric, SampleString};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::models::{self, Image};
use crate::{AppState, Clock};

pub const EXTENSIONS: &[&str] = &["gz", "bz2", "xz"];
pub const UNSUPPORTED: &str = "Only .gz, .bz2 and .xz images are supported";

/// Queues images for hashing; one worker processes them in order, like the single
/// Laravel queue worker did, so a Pi is not saturated by parallel decompression.
#[derive(Clone)]
pub struct Hasher {
    tx: mpsc::UnboundedSender<i64>,
}

impl Hasher {
    pub fn spawn(db: SqlitePool, uploads: PathBuf) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<i64>();
        tokio::spawn(async move {
            while let Some(id) = rx.recv().await {
                if let Err(err) = hash(&db, &uploads, id).await {
                    tracing::error!("hashing image {id}: {err:#}");
                }
            }
        });
        Hasher { tx }
    }

    pub fn enqueue(&self, id: i64) {
        let _ = self.tx.send(id);
    }

    /// Picks up images whose hashing was interrupted, replacing Laravel's persistent jobs table.
    pub async fn enqueue_pending(&self, db: &SqlitePool) -> Result<()> {
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM images WHERE sha256 = '' OR uncompressed_sha256 IS NULL ORDER BY id")
                .fetch_all(db)
                .await?;
        ids.into_iter().for_each(|id| self.enqueue(id));
        Ok(())
    }
}

pub fn path(uploads: &Path, image: &Image) -> PathBuf {
    uploads.join(&image.filename_on_server)
}

async fn hash(db: &SqlitePool, uploads: &Path, id: i64) -> Result<()> {
    let Some(image) = models::image(db, Some(id)).await? else {
        return Ok(());
    };
    let file = path(uploads, &image);

    if image.sha256.is_empty() {
        let source = file.clone();
        let sha256 = tokio::task::spawn_blocking(move || sha256(File::open(source)?)).await??;
        sqlx::query("UPDATE images SET sha256 = ? WHERE id = ?").bind(sha256).bind(id).execute(db).await?;
    }
    if image.uncompressed_sha256.is_none() {
        let extension = image.filename_extension.clone();
        let (sha256, size) = tokio::task::spawn_blocking(move || uncompressed(&file, &extension)).await??;
        sqlx::query("UPDATE images SET uncompressed_sha256 = ?, uncompressed_size = ? WHERE id = ?")
            .bind(sha256)
            .bind(size as i64)
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}

fn sha256(mut reader: impl Read) -> io::Result<String> {
    Ok(sha256_and_size(&mut reader)?.0)
}

fn sha256_and_size(reader: &mut impl Read) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok((hex::encode(hasher.finalize()), size));
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
}

/// Hash and size of the decompressed disk image, i.e. what ends up on the eMMC.
pub fn uncompressed(file: &Path, extension: &str) -> Result<(String, u64)> {
    let open = || File::open(file).with_context(|| format!("opening {}", file.display()));
    let result = match extension {
        "gz" => sha256_and_size(&mut flate2::read::MultiGzDecoder::new(BufReader::new(open()?))),
        "bz2" => sha256_and_size(&mut bzip2::read::MultiBzDecoder::new(BufReader::new(open()?))),
        "xz" => {
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
            sha256_and_size(&mut lzma_rust2::XzReaderMt::new(BufReader::new(open()?), true, threads)?)
        }
        _ => bail!("Unsupported image file extension"),
    };
    result.with_context(|| format!("decompressing {}", file.display()))
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("{UNSUPPORTED}")]
    Unsupported,
    #[error("The image field is required.")]
    Missing,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

fn extension(filename: &str) -> Option<&str> {
    let (_, ext) = filename.rsplit_once('.')?;
    EXTENSIONS.contains(&ext).then_some(ext)
}

/// Streams the multipart `image` field to disk, so multi-gigabyte uploads never sit in memory.
pub async fn store(state: &AppState, multipart: &mut Multipart) -> Result<Image, UploadError> {
    while let Some(field) = multipart.next_field().await.context("reading upload")? {
        if field.name() == Some("image") {
            return store_field(state, field).await;
        }
    }
    Err(UploadError::Missing)
}

async fn store_field(state: &AppState, mut field: Field<'_>) -> Result<Image, UploadError> {
    let filename = field.file_name().ok_or(UploadError::Missing)?.to_string();
    let ext = extension(&filename).ok_or(UploadError::Unsupported)?.to_string();

    let uploads = state.config.uploads_dir();
    tokio::fs::create_dir_all(&uploads).await.context("creating uploads directory")?;
    let (on_server, mut file) = loop {
        let name = format!("{}.{ext}", Alphanumeric.sample_string(&mut rand::rng(), 40));
        match tokio::fs::OpenOptions::new().write(true).create_new(true).open(uploads.join(&name)).await {
            Ok(file) => break (name, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(anyhow::Error::from(err).context("creating upload").into()),
        }
    };

    let written = async {
        while let Some(chunk) = field.chunk().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(uploads.join(&on_server)).await;
        return Err(err.context("receiving upload").into());
    }

    let image = insert(&state.db, &state.clock, &filename, &on_server, &ext).await?;
    state.hasher.enqueue(image.id);
    Ok(image)
}

async fn insert(db: &SqlitePool, clock: &Clock, filename: &str, on_server: &str, ext: &str) -> Result<Image> {
    let now = clock.timestamp();
    let id = sqlx::query(
        "INSERT INTO images (filename, filename_on_server, filename_extension, sha256, created_at, updated_at) \
         VALUES (?, ?, ?, '', ?, ?)",
    )
    .bind(filename)
    .bind(on_server)
    .bind(ext)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?
    .last_insert_rowid();
    models::image(db, Some(id)).await?.context("reading back image")
}

pub async fn delete(state: &AppState, image: &Image) -> Result<()> {
    match tokio::fs::remove_file(path(&state.config.uploads_dir(), image)).await {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("deleting image file"),
    }
    sqlx::query("DELETE FROM images WHERE id = ?").bind(image.id).execute(&state.db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn disk() -> Vec<u8> {
        (0..3 * 1024 * 1024 + 512).map(|i| (i % 251) as u8).collect()
    }

    fn expected() -> (String, u64) {
        let data = disk();
        (hex::encode(Sha256::digest(&data)), data.len() as u64)
    }

    fn write(dir: &Path, name: &str, contents: Vec<u8>) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn hashes_uncompressed_gzip_including_multiple_members() {
        let dir = tempfile::tempdir().unwrap();
        let data = disk();
        let (first, second) = data.split_at(1000);
        let mut gz = Vec::new();
        for part in [first, second] {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(part).unwrap();
            gz.extend(e.finish().unwrap());
        }
        assert_eq!(uncompressed(&write(dir.path(), "a.gz", gz), "gz").unwrap(), expected());
    }

    #[test]
    fn hashes_uncompressed_bzip2() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        e.write_all(&disk()).unwrap();
        assert_eq!(uncompressed(&write(dir.path(), "a.bz2", e.finish().unwrap()), "bz2").unwrap(), expected());
    }

    #[test]
    fn hashes_uncompressed_xz() {
        let dir = tempfile::tempdir().unwrap();
        let mut options = lzma_rust2::XzOptions::with_preset(1);
        options.set_block_size(std::num::NonZeroU64::new(1024 * 1024));
        let mut e = lzma_rust2::XzWriter::new(Vec::new(), options).unwrap();
        e.write_all(&disk()).unwrap();
        assert_eq!(uncompressed(&write(dir.path(), "a.xz", e.finish().unwrap()), "xz").unwrap(), expected());
    }

    #[test]
    fn corrupt_archive_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(uncompressed(&write(dir.path(), "a.gz", b"not gzip".to_vec()), "gz").is_err());
    }

    #[test]
    fn accepts_only_supported_extensions() {
        assert_eq!(extension("raspios.img.xz"), Some("xz"));
        assert_eq!(extension("raspios.img"), None);
        assert_eq!(extension("raspios.zip"), None);
        assert_eq!(extension("xz"), None);
    }
}
