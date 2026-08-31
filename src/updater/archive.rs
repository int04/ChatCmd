use std::{
    fs::File as StdFile,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use tokio::{fs::File, io::AsyncWriteExt};
use zip::ZipArchive;

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const MAX_UPDATE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub(crate) async fn download_zip<F>(
    client: &Client,
    url: Url,
    destination: &Path,
    mut progress: F,
) -> Result<()>
where
    F: FnMut(u64, Option<u64>),
{
    let response = client
        .get(url)
        .send()
        .await
        .context("download update archive")?;
    if response.url().scheme() != "https" || response.url().host_str() != Some("cdn.chatcmd.net") {
        bail!("update download redirected outside the ChatCMD CDN");
    }
    if !response.status().is_success() {
        bail!("update CDN returned HTTP {}", response.status());
    }
    let total = response.content_length();
    if total.is_some_and(|size| size > MAX_UPDATE_BYTES) {
        bail!("update archive is larger than the supported limit");
    }
    let mut stream = response.bytes_stream();
    let mut file = File::create(destination)
        .await
        .context("create update archive")?;
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
        progress(downloaded, total);
    }
    file.flush().await.context("flush update archive")?;
    if downloaded == 0 {
        bail!("update archive is empty");
    }
    Ok(())
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
        preserve_unix_permissions(&output, entry.unix_mode())?;
    }
    progress(total, total);
    Ok(())
}

#[cfg(unix)]
fn preserve_unix_permissions(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, permissions).context("restore update file permissions")?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn preserve_unix_permissions(_: &Path, _: Option<u32>) -> Result<()> {
    Ok(())
}
