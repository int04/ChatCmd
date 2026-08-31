use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Clone, Debug)]
pub(crate) struct PreparedUpdate {
    pub version: String,
    pub stage_dir: PathBuf,
    extension_source: PathBuf,
    extension_destination: PathBuf,
    payload: InstallPayload,
    working_directory: PathBuf,
    version_marker: PathBuf,
}

#[derive(Clone, Debug)]
enum InstallPayload {
    Executable {
        source: PathBuf,
        destination: PathBuf,
    },
    #[cfg(target_os = "macos")]
    AppBundle {
        source: PathBuf,
        destination: PathBuf,
    },
}

pub(crate) fn prepare_update(stage_dir: PathBuf, version: String) -> Result<PreparedUpdate> {
    let extract_dir = stage_dir.join("extracted");
    let payload_root = locate_payload_root(&extract_dir)?;
    let extension_source = find_extension_folder(&payload_root)
        .ok_or_else(|| anyhow!("update package does not contain an extension folder"))?;
    let current_exe = std::env::current_exe().context("resolve current ChatCMD executable")?;
    let (payload, working_directory, extension_destination) =
        resolve_install_destinations(&payload_root, &current_exe)?;
    verify_directory_writable(&working_directory)?;
    let version_marker = working_directory.join("chatcmd-version.txt");
    Ok(PreparedUpdate {
        version,
        stage_dir,
        extension_source,
        extension_destination,
        payload,
        working_directory,
        version_marker,
    })
}

impl PreparedUpdate {
    pub(crate) fn spawn_installer(&self) -> Result<()> {
        #[cfg(target_os = "windows")]
        return self.spawn_windows_installer();
        #[cfg(target_os = "macos")]
        return self.spawn_macos_installer();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        bail!("automatic installation is supported only on Windows and macOS");
    }

    #[cfg(target_os = "windows")]
    fn spawn_windows_installer(&self) -> Result<()> {
        let (source, destination) = match &self.payload {
            InstallPayload::Executable {
                source,
                destination,
            } => (source, destination),
        };
        let script = self.stage_dir.join("install-update.ps1");
        let error_log = self.stage_dir.join("install-error.log");
        let content = format!(
            "$ErrorActionPreference = 'Stop'\r\n\
$oldPid = {}\r\n\
while (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 350 }}\r\n\
try {{\r\n\
  if (Test-Path -LiteralPath {}) {{ Remove-Item -LiteralPath {} -Recurse -Force }}\r\n\
  Copy-Item -LiteralPath {} -Destination {} -Recurse -Force\r\n\
  Copy-Item -LiteralPath {} -Destination {} -Force\r\n\
  Set-Content -LiteralPath {} -Value {} -Encoding Ascii -NoNewline\r\n\
  Start-Process -FilePath {} -WorkingDirectory {}\r\n\
}} catch {{\r\n\
  ($_ | Out-String) | Set-Content -LiteralPath {}\r\n\
  exit 1\r\n\
}}\r\n",
            std::process::id(),
            ps_quote(&self.extension_destination),
            ps_quote(&self.extension_destination),
            ps_quote(&self.extension_source),
            ps_quote(&self.extension_destination),
            ps_quote(source),
            ps_quote(destination),
            ps_quote(&self.version_marker),
            ps_quote_text(&self.version),
            ps_quote(destination),
            ps_quote(&self.working_directory),
            ps_quote(&error_log),
        );
        std::fs::write(&script, content).context("write Windows update helper")?;
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("start Windows update helper")?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn spawn_macos_installer(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let script = self.stage_dir.join("install-update.sh");
        let error_log = self.stage_dir.join("install-error.log");
        let (install_payload, launch_payload) = match &self.payload {
            InstallPayload::Executable {
                source,
                destination,
            } => (
                format!(
                    "cp -f {} {}\nchmod +x {}\n",
                    sh_quote(source),
                    sh_quote(destination),
                    sh_quote(destination),
                ),
                format!("nohup {} >/dev/null 2>&1 &\n", sh_quote(destination)),
            ),
            InstallPayload::AppBundle {
                source,
                destination,
            } => (
                format!(
                    "rm -rf {}\ncp -R {} {}\n",
                    sh_quote(destination),
                    sh_quote(source),
                    sh_quote(destination),
                ),
                format!("open {}\n", sh_quote(destination)),
            ),
        };
        let content = format!(
            "#!/bin/sh\nset -eu\nOLD_PID={}\nwhile kill -0 \"$OLD_PID\" 2>/dev/null; do sleep 0.35; done\n{{\nrm -rf {}\ncp -R {} {}\n{}\nprintf '%s' {} > {}\n{}\n}} 2>{}\n",
            std::process::id(),
            sh_quote(&self.extension_destination),
            sh_quote(&self.extension_source),
            sh_quote(&self.extension_destination),
            install_payload,
            sh_quote_text(&self.version),
            sh_quote(&self.version_marker),
            launch_payload,
            sh_quote(&error_log),
        );
        std::fs::write(&script, content).context("write macOS update helper")?;
        let mut permissions = std::fs::metadata(&script)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions)?;
        Command::new("/bin/sh")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("start macOS update helper")?;
        Ok(())
    }
}

pub(crate) fn exit_for_update() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(900));
        std::process::exit(0);
    });
}

fn resolve_install_destinations(
    payload_root: &Path,
    current_exe: &Path,
) -> Result<(InstallPayload, PathBuf, PathBuf)> {
    #[cfg(target_os = "windows")]
    {
        let source = find_child(payload_root, "ChatCMD.exe", false)
            .ok_or_else(|| anyhow!("update package does not contain ChatCMD.exe"))?;
        let working_directory = current_exe
            .parent()
            .ok_or_else(|| anyhow!("current executable has no parent directory"))?
            .to_path_buf();
        let extension_destination = working_directory.join("chatgpt-extension");
        return Ok((
            InstallPayload::Executable {
                source,
                destination: current_exe.to_path_buf(),
            },
            working_directory,
            extension_destination,
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let current_app = current_exe
            .ancestors()
            .find(|path| {
                path.extension()
                    .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("app"))
            })
            .map(Path::to_path_buf);
        let distribution_root = current_app
            .as_deref()
            .and_then(Path::parent)
            .or_else(|| current_exe.parent())
            .ok_or_else(|| anyhow!("current executable has no parent directory"))?
            .to_path_buf();
        let extension_destination = distribution_root.join("chatgpt-extension");
        if let Some(source) = find_child(payload_root, "ChatCMD.app", true) {
            let destination = current_app.unwrap_or_else(|| distribution_root.join("ChatCMD.app"));
            return Ok((
                InstallPayload::AppBundle {
                    source,
                    destination,
                },
                distribution_root,
                extension_destination,
            ));
        }
        let source = find_child(payload_root, "ChatCMD", false)
            .ok_or_else(|| anyhow!("update package does not contain ChatCMD or ChatCMD.app"))?;
        return Ok((
            InstallPayload::Executable {
                source,
                destination: current_exe.to_path_buf(),
            },
            distribution_root,
            extension_destination,
        ));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (payload_root, current_exe);
        bail!("automatic installation is supported only on Windows and macOS");
    }
}

fn locate_payload_root(extract_dir: &Path) -> Result<PathBuf> {
    let mut current = extract_dir.to_path_buf();
    for _ in 0..5 {
        if has_expected_payload(&current) && find_extension_folder(&current).is_some() {
            return Ok(current);
        }
        let directories = child_directories(&current)?;
        if directories.len() == 1 {
            current = directories[0].clone();
            continue;
        }
        if let Some(found) = directories
            .into_iter()
            .find(|path| has_expected_payload(path) && find_extension_folder(path).is_some())
        {
            return Ok(found);
        }
        break;
    }
    bail!("update package layout is not recognized")
}

fn has_expected_payload(root: &Path) -> bool {
    #[cfg(target_os = "windows")]
    return find_child(root, "ChatCMD.exe", false).is_some();
    #[cfg(target_os = "macos")]
    return find_child(root, "ChatCMD", false).is_some()
        || find_child(root, "ChatCMD.app", true).is_some();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    false
}

fn find_extension_folder(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut fallback = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name == "chatgpt-extension" {
            return Some(path);
        }
        if (name.contains("extension") || name.contains("extesion")) && !name.ends_with(".app") {
            fallback = Some(path);
        }
    }
    fallback
}

fn find_child(root: &Path, name: &str, directory: bool) -> Option<PathBuf> {
    std::fs::read_dir(root).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let matches_type = if directory {
            path.is_dir()
        } else {
            path.is_file()
        };
        (matches_type
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name))
        .then_some(path)
    })
}

fn child_directories(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(root).context("read extracted update directory")?;
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.file_name().to_string_lossy() != "__MACOSX")
        .map(|entry| entry.path())
        .collect())
}

fn verify_directory_writable(directory: &Path) -> Result<()> {
    let probe = directory.join(format!(".chatcmd-update-write-test-{}", std::process::id()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .with_context(|| format!("ChatCMD cannot write to {}", directory.display()))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

#[cfg(target_os = "windows")]
fn ps_quote(path: &Path) -> String {
    ps_quote_text(&path.to_string_lossy())
}

#[cfg(target_os = "windows")]
fn ps_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "macos")]
fn sh_quote(path: &Path) -> String {
    sh_quote_text(&path.to_string_lossy())
}

#[cfg(target_os = "macos")]
fn sh_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
