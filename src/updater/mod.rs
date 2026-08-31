mod archive;
mod install;
mod model;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Url};
use serde::Deserialize;
use uuid::Uuid;

use archive::{download_zip, extract_zip};
use install::{PreparedUpdate, exit_for_update, prepare_update};
use model::{RemoteVersion, UpdateAsset, UpdateTarget, is_remote_newer};
pub(crate) use model::{UpdatePhase, UpdateStatus};

const CDN_BASE_URL: &str = "https://cdn.chatcmd.net/";

pub(crate) struct UpdateManager {
    client: Client,
    status: Mutex<UpdateStatus>,
    prepared: Mutex<Option<PreparedUpdate>>,
    running: AtomicBool,
}

#[derive(Debug, Deserialize)]
struct RemoteProblem {
    code: Option<String>,
    message: Option<String>,
}

impl UpdateManager {
    pub(crate) fn new() -> Arc<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .user_agent(format!("ChatCMD/{}", crate::version::app_version()))
            .build()
            .unwrap_or_else(|_| Client::new());
        Arc::new(Self {
            client,
            status: Mutex::new(UpdateStatus::initial()),
            prepared: Mutex::new(None),
            running: AtomicBool::new(false),
        })
    }

    pub(crate) fn status(&self) -> UpdateStatus {
        self.status_guard().clone()
    }

    pub(crate) async fn check_latest(&self, backend_base_url: &str) -> Result<UpdateStatus> {
        let target = UpdateTarget::current();
        if !target.supported {
            return Ok(self.status());
        }
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Checking;
            status.progress_percent = None;
            status.message = None;
        });
        let remote = match self.fetch_remote_version(backend_base_url, target).await {
            Ok(remote) => remote,
            Err(error) => {
                self.fail(&error);
                return Err(error);
            }
        };
        let Some(remote) = remote else {
            self.mutate_status(|status| {
                status.latest_version = None;
                status.note = None;
                status.phase = UpdatePhase::UpToDate;
                status.update_available = false;
                status.download_available = false;
                status.message =
                    Some("No client version has been published for this platform yet.".to_owned());
            });
            return Ok(self.status());
        };
        let remote_version = remote.version.trim();
        if !crate::version::is_valid_version(remote_version) {
            let error = anyhow!("backend returned an invalid client version");
            self.fail(&error);
            return Err(error);
        }
        let current_version = crate::version::app_version();
        let available = is_remote_newer(remote_version, &current_version);
        let asset = target.asset(&remote);
        let prepared_version = self
            .prepared_guard()
            .as_ref()
            .map(|value| value.version.clone());
        self.mutate_status(|status| {
            status.latest_version = Some(remote.version.trim().to_owned());
            status.note = Some(remote.note.trim().to_owned());
            status.update_available = available;
            status.download_available = asset.is_some();
            status.progress_percent = None;
            status.downloaded_bytes = 0;
            status.total_bytes = None;
            status.message = if available && asset.is_none() {
                Some(
                    "The latest version does not include a package for this architecture."
                        .to_owned(),
                )
            } else {
                None
            };
            status.phase = if available {
                if prepared_version.as_deref() == Some(remote.version.trim()) {
                    UpdatePhase::ReadyToRestart
                } else {
                    UpdatePhase::Available
                }
            } else {
                UpdatePhase::UpToDate
            };
        });
        Ok(self.status())
    }

    pub(crate) fn start_update(self: &Arc<Self>, backend_base_url: String) -> UpdateStatus {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self.status();
        }
        self.prepared_guard().take();
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Checking;
            status.progress_percent = None;
            status.downloaded_bytes = 0;
            status.total_bytes = None;
            status.message = None;
        });
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = manager.run_update(&backend_base_url).await {
                manager.fail(&error);
            }
            manager.running.store(false, Ordering::Release);
        });
        self.status()
    }

    pub(crate) fn restart_to_install(&self) -> Result<UpdateStatus> {
        if self.running.load(Ordering::Acquire) {
            bail!("the update is still being prepared");
        }
        let prepared = self
            .prepared_guard()
            .clone()
            .ok_or_else(|| anyhow!("no prepared update is ready to install"))?;
        prepared.spawn_installer()?;
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Restarting;
            status.progress_percent = Some(100);
            status.message = Some("ChatCMD is restarting to finish the update.".to_owned());
        });
        exit_for_update();
        Ok(self.status())
    }

    async fn run_update(self: &Arc<Self>, backend_base_url: &str) -> Result<()> {
        let target = UpdateTarget::current();
        if !target.supported {
            bail!("automatic updates are not supported on this platform");
        }
        let remote = self
            .fetch_remote_version(backend_base_url, target)
            .await?
            .ok_or_else(|| anyhow!("no client version has been published for this platform"))?;
        let current_version = crate::version::app_version();
        if !is_remote_newer(&remote.version, &current_version) {
            self.mutate_status(|status| {
                status.latest_version = Some(remote.version.trim().to_owned());
                status.note = Some(remote.note.trim().to_owned());
                status.phase = UpdatePhase::UpToDate;
                status.update_available = false;
                status.download_available = target.asset(&remote).is_some();
                status.message = None;
            });
            return Ok(());
        }
        let asset = target
            .asset(&remote)
            .ok_or_else(|| anyhow!("the latest version has no package for this architecture"))?;
        self.set_remote_asset(&asset);
        let stage_dir = create_stage_dir(&asset.version).await?;
        let archive_path = stage_dir.join("update.zip");
        let extract_dir = stage_dir.join("extracted");
        let url = cdn_url(&asset.object_key)?;
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Downloading;
            status.progress_percent = Some(0);
            status.downloaded_bytes = 0;
            status.total_bytes = None;
        });
        let download_manager = Arc::clone(self);
        download_zip(
            &self.client,
            url,
            &archive_path,
            move |downloaded, total| {
                download_manager.update_download_progress(downloaded, total);
            },
        )
        .await?;
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Extracting;
            status.progress_percent = Some(0);
            status.downloaded_bytes = 0;
            status.total_bytes = None;
        });
        let extract_manager = Arc::clone(self);
        let extract_archive = archive_path.clone();
        let extract_destination = extract_dir.clone();
        tokio::task::spawn_blocking(move || {
            extract_zip(
                &extract_archive,
                &extract_destination,
                move |done, total| {
                    extract_manager.update_extract_progress(done, total);
                },
            )
        })
        .await
        .context("join update extraction task")??;
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Preparing;
            status.progress_percent = None;
            status.message = None;
        });
        let prepared_stage = stage_dir.clone();
        let prepared_version = asset.version.clone();
        let prepared =
            tokio::task::spawn_blocking(move || prepare_update(prepared_stage, prepared_version))
                .await
                .context("join update preparation task")??;
        *self.prepared_guard() = Some(prepared);
        self.mutate_status(|status| {
            status.phase = UpdatePhase::ReadyToRestart;
            status.progress_percent = Some(100);
            status.message =
                Some("Update files are ready. Restart ChatCMD to install them.".to_owned());
        });
        Ok(())
    }

    async fn fetch_remote_version(
        &self,
        backend_base_url: &str,
        target: UpdateTarget,
    ) -> Result<Option<RemoteVersion>> {
        let api_target = target
            .api_target
            .ok_or_else(|| anyhow!("unsupported update target"))?;
        let url = format!(
            "{}/api/client-version?target={api_target}",
            backend_base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .context("check latest ChatCMD version")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let problem = response.json::<RemoteProblem>().await.ok();
            if problem.as_ref().and_then(|value| value.code.as_deref()) == Some("version_not_found")
            {
                return Ok(None);
            }
            let detail = problem
                .and_then(|value| value.message)
                .unwrap_or_else(|| "version API returned HTTP 404".to_owned());
            bail!("{detail}");
        }
        if !response.status().is_success() {
            let status = response.status();
            let problem = response.json::<RemoteProblem>().await.ok();
            let detail = problem
                .and_then(|value| value.message)
                .unwrap_or_else(|| format!("version API returned HTTP {status}"));
            bail!("{detail}");
        }
        let remote = response
            .json::<RemoteVersion>()
            .await
            .context("parse latest ChatCMD version")?;
        if !crate::version::is_valid_version(remote.version.trim()) {
            bail!("backend returned an invalid client version");
        }
        Ok(Some(remote))
    }

    fn set_remote_asset(&self, asset: &UpdateAsset) {
        self.mutate_status(|status| {
            status.latest_version = Some(asset.version.clone());
            status.note = Some(asset.note.clone());
            status.update_available = true;
            status.download_available = true;
            status.message = None;
        });
    }

    fn update_download_progress(&self, downloaded: u64, total: Option<u64>) {
        self.mutate_status(|status| {
            status.downloaded_bytes = downloaded;
            status.total_bytes = total;
            status.progress_percent = total.and_then(|total| percent(downloaded, total));
        });
    }

    fn update_extract_progress(&self, extracted: u64, total: u64) {
        self.mutate_status(|status| {
            status.downloaded_bytes = extracted;
            status.total_bytes = Some(total);
            status.progress_percent = percent(extracted, total);
        });
    }

    fn fail(&self, error: &anyhow::Error) {
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Failed;
            status.progress_percent = None;
            status.message = Some(format!("{error:#}"));
        });
    }

    fn mutate_status(&self, mutate: impl FnOnce(&mut UpdateStatus)) {
        let mut status = self.status_guard();
        mutate(&mut status);
    }

    fn status_guard(&self) -> std::sync::MutexGuard<'_, UpdateStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn prepared_guard(&self) -> std::sync::MutexGuard<'_, Option<PreparedUpdate>> {
        self.prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn percent(done: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return Some(100);
    }
    let value = done.saturating_mul(100) / total;
    Some(u8::try_from(value.min(100)).unwrap_or(100))
}

fn cdn_url(object_key: &str) -> Result<Url> {
    let base = Url::parse(CDN_BASE_URL).context("parse update CDN base URL")?;
    let url = base.join(object_key).context("build update CDN URL")?;
    if url.scheme() != "https" || url.host_str() != Some("cdn.chatcmd.net") {
        bail!("update object key resolved outside the ChatCMD CDN");
    }
    Ok(url)
}

async fn create_stage_dir(version: &str) -> Result<std::path::PathBuf> {
    let safe_version: String = version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let path = std::env::temp_dir()
        .join("ChatCMD")
        .join("updates")
        .join(format!("{}-{}", safe_version, Uuid::new_v4()));
    tokio::fs::create_dir_all(&path)
        .await
        .context("create update staging directory")?;
    Ok(path)
}
