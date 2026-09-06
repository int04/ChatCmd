use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::Deserialize;

use super::model::{ReleaseSelection, UpdateAsset, UpdateTarget};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/int04/ChatCMD/releases/latest";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

pub(crate) async fn fetch_latest(
    client: &Client,
    target: UpdateTarget,
) -> Result<Option<ReleaseSelection>> {
    let response = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::ACCEPT, GITHUB_ACCEPT)
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .send()
        .await
        .context("request latest ChatCMD GitHub release")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!(
            "GitHub release API returned HTTP {}",
            response.status().as_u16()
        );
    }
    let release = response
        .json::<GithubRelease>()
        .await
        .context("parse latest ChatCMD GitHub release")?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    validate_https_url(&release.html_url, &["github.com"])
        .context("validate GitHub release page URL")?;

    let note = release.body.unwrap_or_default().trim().to_owned();
    let version = extract_build_version(&note)
        .or_else(|| version_from_tag(&release.tag_name))
        .ok_or_else(|| anyhow!("GitHub release does not contain a valid ChatCMD version"))?;
    if !crate::version::is_valid_version(&version) {
        bail!("GitHub release contains an invalid ChatCMD version");
    }

    let wanted_name = target.asset_name();
    let matching_asset = wanted_name.and_then(|name| {
        release
            .assets
            .iter()
            .find(|asset| asset.name.eq_ignore_ascii_case(name))
            .cloned()
    });
    let asset = match matching_asset {
        Some(asset) => Some(resolve_asset(client, &release.assets, asset).await?),
        None => None,
    };

    Ok(Some(ReleaseSelection {
        version,
        tag: release.tag_name.clone(),
        name: release
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| release.tag_name.clone()),
        release_url: release.html_url,
        note,
        asset,
    }))
}

async fn resolve_asset(
    client: &Client,
    release_assets: &[GithubAsset],
    asset: GithubAsset,
) -> Result<UpdateAsset> {
    validate_https_url(&asset.browser_download_url, &["github.com"])
        .context("validate GitHub release download URL")?;
    let sha256 = if let Some(digest) = asset.digest.as_deref().and_then(parse_sha256_digest) {
        digest
    } else {
        let sums = release_assets
            .iter()
            .find(|candidate| candidate.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
            .ok_or_else(|| {
                anyhow!("GitHub release asset has no SHA-256 digest or checksum file")
            })?;
        fetch_checksum_file(client, sums, &asset.name).await?
    };
    Ok(UpdateAsset {
        name: asset.name,
        download_url: asset.browser_download_url,
        size: asset.size,
        sha256,
    })
}

async fn fetch_checksum_file(
    client: &Client,
    sums_asset: &GithubAsset,
    wanted_name: &str,
) -> Result<String> {
    validate_https_url(&sums_asset.browser_download_url, &["github.com"])
        .context("validate GitHub checksum download URL")?;
    let response = client
        .get(&sums_asset.browser_download_url)
        .send()
        .await
        .context("download ChatCMD SHA256SUMS.txt")?;
    if !response.status().is_success() {
        bail!(
            "GitHub checksum download returned HTTP {}",
            response.status().as_u16()
        );
    }
    let final_url = response.url().as_str().to_owned();
    validate_github_download_url(&final_url).context("validate redirected checksum URL")?;
    let text = response
        .text()
        .await
        .context("read ChatCMD SHA256SUMS.txt")?;
    checksum_from_sums(&text, wanted_name)
        .ok_or_else(|| anyhow!("SHA256SUMS.txt does not contain {wanted_name}"))
}

pub(crate) fn validate_github_download_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value).context("parse GitHub download URL")?;
    if url.scheme() != "https" {
        bail!("GitHub update download must use HTTPS");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("GitHub update download URL has no host"))?;
    if host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("objects.githubusercontent.com")
        || host.eq_ignore_ascii_case("release-assets.githubusercontent.com")
        || host.ends_with(".githubusercontent.com")
    {
        return Ok(());
    }
    bail!("GitHub update download redirected to an untrusted host")
}

fn validate_https_url(value: &str, allowed_hosts: &[&str]) -> Result<()> {
    let url = reqwest::Url::parse(value).context("parse URL")?;
    if url.scheme() != "https" {
        bail!("URL must use HTTPS");
    }
    let host = url.host_str().ok_or_else(|| anyhow!("URL has no host"))?;
    if allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        Ok(())
    } else {
        bail!("URL host is not allowed")
    }
}

fn extract_build_version(note: &str) -> Option<String> {
    let lowercase = note.to_ascii_lowercase();
    let marker = "build version:";
    let index = lowercase.find(marker)? + marker.len();
    let remainder = note.get(index..)?.trim_start();
    let token: String = remainder
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        })
        .collect();
    let token = token.trim_end_matches('.').to_owned();
    crate::version::is_valid_version(&token).then_some(token)
}

fn version_from_tag(tag: &str) -> Option<String> {
    let version = tag
        .trim()
        .trim_start_matches(['v', 'V'])
        .trim_start_matches('.')
        .to_owned();
    crate::version::is_valid_version(&version).then_some(version)
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let digest = value.trim().strip_prefix("sha256:")?.to_ascii_lowercase();
    is_sha256_hex(&digest).then_some(digest)
}

fn checksum_from_sums(contents: &str, wanted_name: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?.trim().to_ascii_lowercase();
        let name = parts.next()?.trim_start_matches('*');
        (name == wanted_name && is_sha256_hex(&digest)).then_some(digest)
    })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_full_build_version_from_release_notes() {
        assert_eq!(
            extract_build_version("Automatically built. Build version: 26.09.03.2207. See sums."),
            Some("26.09.03.2207".to_owned())
        );
        assert_eq!(
            extract_build_version("BUILD VERSION: 26.09.06.1111-DEV\nNotes"),
            Some("26.09.06.1111-DEV".to_owned())
        );
    }

    #[test]
    fn parses_release_tag_fallback() {
        assert_eq!(version_from_tag("v.26.09.03"), Some("26.09.03".to_owned()));
    }

    #[test]
    fn parses_digest_and_checksum_file() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{digest}")),
            Some(digest.clone())
        );
        let sums = format!("{digest}  ChatCMD-windows-x64.zip\n");
        assert_eq!(
            checksum_from_sums(&sums, "ChatCMD-windows-x64.zip"),
            Some(digest)
        );
    }
}
