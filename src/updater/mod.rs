mod archive;
mod github;
mod install;
#[cfg(test)]
mod install_tests;
mod model;

use std::{
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Url};
use uuid::Uuid;

use archive::{download_zip, extract_zip};
use install::{PreparedUpdate, exit_for_update, prepare_update};
use model::UpdatePhase;
pub(crate) use model::UpdateStatus;
use model::{ReleaseSelection, UpdateTarget, is_remote_newer};

struct ManagerState {
    status: UpdateStatus,
    selection: Option<ReleaseSelection>,
    prepared: Option<PreparedUpdate>,
}

pub(crate) struct UpdateManager {
    client: Client,
    state: Mutex<ManagerState>,
    running: AtomicBool,
    port: u16,
}

impl UpdateManager {
    pub(crate) fn new(port: u16) -> Arc<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .user_agent(format!("ChatCMD/{}", crate::version::app_version()))
            .build()
            .unwrap_or_else(|_| Client::new());
        let mut status = UpdateStatus::initial();
        if let Some(error) = install::last_install_error() {
            status.phase = UpdatePhase::Failed;
            status.message = Some(format!(
                "The previous update failed and ChatCMD rolled back to the previous version. {error}"
            ));
        }
        Arc::new(Self {
            client,
            state: Mutex::new(ManagerState {
                status,
                selection: None,
                prepared: None,
            }),
            running: AtomicBool::new(false),
            port,
        })
    }

    pub(crate) fn status(&self) -> UpdateStatus {
        self.state_guard().status.clone()
    }

    pub(crate) async fn check_latest(&self) -> UpdateStatus {
        let target = UpdateTarget::current();
        if !target.supported {
            return self.status();
        }
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self.status();
        }
        install::clear_install_error();
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Checking;
            status.progress_percent = None;
            status.downloaded_bytes = 0;
            status.total_bytes = None;
            status.checksum_verified = false;
            status.message = Some("Checking GitHub Releases for a newer ChatCMD build…".to_owned());
        });
        match github::fetch_latest(&self.client, target).await {
            Ok(selection) => self.finish_check(selection),
            Err(error) => self.fail(&error),
        }
        self.running.store(false, Ordering::Release);
        self.status()
    }

    pub(crate) fn start_update(self: &Arc<Self>) -> UpdateStatus {
        if cfg!(debug_assertions) {
            self.fail_message(
                "Automatic installation is disabled in debug builds. Build a release package to test self-update safely.",
            );
            return self.status();
        }
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self.status();
        }
        let selection = {
            let mut state = self.state_guard();
            let selection = state.selection.clone();
            state.prepared = None;
            selection
        };
        let Some(selection) = selection else {
            self.running.store(false, Ordering::Release);
            self.fail_message("Check for updates before starting an update.");
            return self.status();
        };
        if !is_remote_newer(&selection.version, &crate::version::app_version()) {
            self.running.store(false, Ordering::Release);
            self.mutate_status(|status| {
                status.phase = UpdatePhase::UpToDate;
                status.update_available = false;
                status.message = Some("ChatCMD is already up to date.".to_owned());
            });
            return self.status();
        }
        if selection.asset.is_none() {
            self.running.store(false, Ordering::Release);
            self.fail_message(
                "The latest release has no package for this platform and architecture.",
            );
            return self.status();
        }
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Downloading;
            status.progress_percent = Some(0);
            status.downloaded_bytes = 0;
            status.total_bytes = selection.asset.as_ref().map(|asset| asset.size);
            status.checksum_verified = false;
            status.message = selection
                .asset
                .as_ref()
                .map(|asset| format!("Downloading {} from GitHub Releases…", asset.name));
        });
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = manager.run_update(selection).await {
                manager.fail(&error);
            }
            manager.running.store(false, Ordering::Release);
        });
        self.status()
    }

    pub(crate) fn restart_to_install(&self) -> UpdateStatus {
        if self.running.load(Ordering::Acquire) {
            self.fail_message("The update is still being prepared.");
            return self.status();
        }
        let prepared = self.state_guard().prepared.clone();
        let Some(prepared) = prepared else {
            self.fail_message("No prepared update is ready to install.");
            return self.status();
        };
        if let Err(error) = prepared.spawn_installer() {
            self.fail(&error);
            return self.status();
        }
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Restarting;
            status.progress_percent = Some(100);
            status.message = Some(format!(
                "Installing ChatCMD {}. The application will close and reopen automatically.",
                prepared.version
            ));
        });
        exit_for_update();
        self.status()
    }

    fn finish_check(&self, selection: Option<ReleaseSelection>) {
        let Some(selection) = selection else {
            let mut state = self.state_guard();
            state.selection = None;
            state.status.latest_version = None;
            state.status.release_tag = None;
            state.status.release_name = None;
            state.status.release_url = None;
            state.status.note = None;
            state.status.asset_name = None;
            state.status.phase = UpdatePhase::UpToDate;
            state.status.update_available = false;
            state.status.download_available = false;
            state.status.message =
                Some("No published ChatCMD release was found on GitHub.".to_owned());
            return;
        };
        let current = crate::version::app_version();
        let available = is_remote_newer(&selection.version, &current);
        let asset_name = selection.asset.as_ref().map(|asset| asset.name.clone());
        let download_available = selection.asset.is_some();
        let prepared_version = self
            .state_guard()
            .prepared
            .as_ref()
            .map(|prepared| prepared.version.clone());
        let mut state = self.state_guard();
        state.status.current_version = current;
        state.status.latest_version = Some(selection.version.clone());
        state.status.release_tag = Some(selection.tag.clone());
        state.status.release_name = Some(selection.name.clone());
        state.status.release_url = Some(selection.release_url.clone());
        state.status.note = Some(selection.note.clone());
        state.status.update_available = available;
        state.status.download_available = download_available;
        state.status.asset_name = asset_name;
        state.status.progress_percent = None;
        state.status.downloaded_bytes = 0;
        state.status.total_bytes = selection.asset.as_ref().map(|asset| asset.size);
        state.status.checksum_verified = false;
        state.status.message = if available && !download_available {
            Some("A newer release exists, but it has no package for this architecture.".to_owned())
        } else if available {
            Some("A newer ChatCMD release is available on GitHub.".to_owned())
        } else {
            Some("ChatCMD is already up to date.".to_owned())
        };
        state.status.phase = if available {
            if prepared_version.as_deref() == Some(selection.version.as_str()) {
                UpdatePhase::ReadyToRestart
            } else {
                UpdatePhase::Available
            }
        } else {
            UpdatePhase::UpToDate
        };
        state.selection = Some(selection);
    }

    async fn run_update(self: &Arc<Self>, selection: ReleaseSelection) -> Result<()> {
        let asset = selection
            .asset
            .clone()
            .ok_or_else(|| anyhow!("release has no compatible update package"))?;
        let stage_dir = create_stage_dir(&selection.version).await?;
        let archive_path = stage_dir.join("update.zip");
        let extract_dir = stage_dir.join("extracted");
        let url = Url::parse(&asset.download_url).context("parse GitHub update asset URL")?;
        let download_manager = Arc::clone(self);
        let actual_sha256 = download_zip(
            &self.client,
            url,
            &archive_path,
            asset.size,
            move |downloaded, total| {
                download_manager.update_download_progress(downloaded, total);
            },
        )
        .await?;

        self.mutate_status(|status| {
            status.phase = UpdatePhase::Verifying;
            status.progress_percent = Some(60);
            status.message = Some(format!("Verifying SHA-256 for {}…", asset.name));
        });
        if !actual_sha256.eq_ignore_ascii_case(&asset.sha256) {
            bail!(
                "SHA-256 verification failed for {}: expected {}, received {}",
                asset.name,
                asset.sha256,
                actual_sha256
            );
        }
        self.mutate_status(|status| {
            status.checksum_verified = true;
            status.progress_percent = Some(65);
            status.message = Some("SHA-256 verification succeeded.".to_owned());
        });

        self.mutate_status(|status| {
            status.phase = UpdatePhase::Extracting;
            status.progress_percent = Some(65);
            status.downloaded_bytes = 0;
            status.total_bytes = None;
            status.message = Some("Extracting the verified update package…".to_owned());
        });
        let extract_manager = Arc::clone(self);
        let archive_for_extract = archive_path.clone();
        let destination_for_extract = extract_dir.clone();
        tokio::task::spawn_blocking(move || {
            extract_zip(
                &archive_for_extract,
                &destination_for_extract,
                move |done, total| extract_manager.update_extract_progress(done, total),
            )
        })
        .await
        .context("join update extraction task")??;

        self.mutate_status(|status| {
            status.phase = UpdatePhase::Preparing;
            status.progress_percent = Some(94);
            status.message =
                Some("Validating files and preparing the restart installer…".to_owned());
        });
        let version = selection.version.clone();
        let prepare_stage = stage_dir.clone();
        let port = self.port;
        let prepared =
            tokio::task::spawn_blocking(move || prepare_update(prepare_stage, version, port))
                .await
                .context("join update preparation task")??;
        {
            let mut state = self.state_guard();
            state.prepared = Some(prepared);
            state.status.phase = UpdatePhase::ReadyToRestart;
            state.status.progress_percent = Some(100);
            state.status.message = Some(
                "Update verified and staged. ChatCMD is ready to restart and install it."
                    .to_owned(),
            );
        }
        Ok(())
    }

    fn update_download_progress(&self, downloaded: u64, total: Option<u64>) {
        self.mutate_status(|status| {
            status.downloaded_bytes = downloaded;
            status.total_bytes = total;
            status.progress_percent =
                total.and_then(|total| scaled_percent(downloaded, total, 0, 55));
        });
    }

    fn update_extract_progress(&self, extracted: u64, total: u64) {
        self.mutate_status(|status| {
            status.downloaded_bytes = extracted;
            status.total_bytes = Some(total);
            status.progress_percent = scaled_percent(extracted, total, 65, 90);
        });
    }

    fn fail(&self, error: &anyhow::Error) {
        self.fail_message(&format!("{error:#}"));
    }

    fn fail_message(&self, message: &str) {
        self.mutate_status(|status| {
            status.phase = UpdatePhase::Failed;
            status.progress_percent = None;
            status.message = Some(message.to_owned());
        });
    }

    fn mutate_status(&self, mutate: impl FnOnce(&mut UpdateStatus)) {
        let mut state = self.state_guard();
        mutate(&mut state.status);
    }

    fn state_guard(&self) -> MutexGuard<'_, ManagerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn scaled_percent(done: u64, total: u64, start: u8, end: u8) -> Option<u8> {
    if total == 0 {
        return Some(end);
    }
    let span = u64::from(end.saturating_sub(start));
    let portion = done.min(total).saturating_mul(span) / total;
    Some(start.saturating_add(u8::try_from(portion).unwrap_or(u8::MAX)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_phase_progress() {
        assert_eq!(scaled_percent(0, 100, 0, 55), Some(0));
        assert_eq!(scaled_percent(50, 100, 0, 55), Some(27));
        assert_eq!(scaled_percent(100, 100, 65, 90), Some(90));
    }
}
