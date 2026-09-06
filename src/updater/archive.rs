use std::{
    fmt::Write as _,
    fs::File as StdFile,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use sha2::{Digest, Sha256};
use tokio::{fs::File, io::AsyncWriteExt};
use zip::ZipArchive;

use super::github::validate_github_download_url;

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const MAX_UPDATE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub(crate) async fn download_zip<F>(
    client: &Client,
    url: Url,
    destination: &Path,
    expected_size: u64,
    mut progress: F,
) -> Result<String>
where
    F: FnMut(u64, Option<u64>),
{
    validate_github_download_url(url.as_str()).context("validate update download URL")?;
    let response = client
        .get(url)
        .send()
        .await
        .context("download update archive from GitHub")?;
    validate_github_download_url(response.url().as_str())
        .context("validate redirected update download URL")?;
    if !response.status().is_success() {
        bail!(
            "GitHub update download returned HTTP {}",
            response.status().as_u16()
        );
    }
    let total = response
        .content_length()
        .or((expected_size > 0).then_some(expected_size));
    if total.is_some_and(|size| size > MAX_UPDATE_BYTES) {
        bail!("update archive is larger than the supported limit");
    }
    let mut stream = response.bytes_stream();
    let mut file = File::create(destination)
        .await
        .context("create update archive")?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;
    progress(0, total);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read update archive chunk")?;
        downloaded = downloaded
            .checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| anyhow!("update archive size overflow"))?;
        if downloaded > MAX_UPDATE_BYTES {
            bail!("update archive is larger than the supported limit");
        }
        file.write_all(&chunk)
            .await
            .context("write update archive chunk")?;
        hasher.update(&chunk);
        progress(downloaded, total);
    }
    file.flush().await.context("flush update archive")?;
    file.sync_all().await.context("sync update archive")?;
    if downloaded == 0 {
        bail!("update archive is empty");
    }
    if expected_size > 0 && downloaded != expected_size {
        bail!(
            "downloaded update size mismatch: expected {expected_size} bytes, received {downloaded} bytes"
        );
    }
    Ok(hex_digest(&hasher.finalize()))
}

pub(crate) fn extract_zip<F>(archive_path: &Path, destination: &Path, mut progress: F) -> Result<()>
where
    F: FnMut(u64, u64),
{
    std::fs::create_dir_all(destination).context("create update extraction directory")?;
    let archive_file = StdFile::open(archive_path).context("open update archive")?;
    let mut archive = ZipArchive::new(archive_file).context("read update zip")?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .context("inspect update zip entry")?;
        reject_symlink(entry.unix_mode())?;
        if !entry.is_dir() {
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| anyhow!("extracted update size overflow"))?;
            if total > MAX_EXTRACTED_BYTES {
                bail!("extracted update is larger than the supported limit");
            }
        }
    }
    let mut extracted = 0_u64;
    progress(0, total);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("open update zip entry")?;
        reject_symlink(entry.unix_mode())?;
        let relative = entry
            .enclosed_name()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("update zip contains an unsafe path"))?;
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output).context("create update zip directory")?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).context("create update file parent")?;
        }
        let mut output_file = StdFile::create(&output).context("create extracted update file")?;
        let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
        loop {
            let read = entry
                .read(&mut buffer)
                .context("read compressed update file")?;
            if read == 0 {
                break;
            }
            output_file
                .write_all(&buffer[..read])
                .context("write extracted update file")?;
            extracted = extracted
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .ok_or_else(|| anyhow!("extracted update size overflow"))?;
            if extracted > MAX_EXTRACTED_BYTES {
                bail!("extracted update is larger than the supported limit");
            }
            progress(extracted, total);
        }
        output_file
            .sync_all()
            .context("sync extracted update file")?;
        preserve_unix_permissions(&output, entry.unix_mode())?;
    }
    progress(total, total);
    Ok(())
}

fn reject_symlink(mode: Option<u32>) -> Result<()> {
    const FILE_TYPE_MASK: u32 = 0o170000;
    const SYMLINK: u32 = 0o120000;
    if mode.is_some_and(|value| value & FILE_TYPE_MASK == SYMLINK) {
        bail!("update zip contains a symbolic link");
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
fn preserve_unix_permissions(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        let permissions = std::fs::Permissions::from_mode(mode & 0o7777);
        std::fs::set_permissions(path, permissions).context("restore update file permissions")?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn preserve_unix_permissions(_: &Path, _: Option<u32>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_symlinks() {
        assert!(reject_symlink(Some(0o120777)).is_err());
        assert!(reject_symlink(Some(0o100755)).is_ok());
    }

    #[test]
    fn renders_lowercase_sha256_hex() {
        assert_eq!(hex_digest(&[0, 1, 15, 16, 255]), "00010f10ff");
    }
}
