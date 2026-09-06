use std::process::Command;

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};

use super::Problem;

fn folder_problem(detail: impl Into<String>) -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Folder picker failed",
        detail,
    )
}

pub(super) async fn pick_project_folder() -> Result<Json<Value>, Problem> {
    let path = tokio::task::spawn_blocking(open_folder_picker)
        .await
        .map_err(|error| folder_problem(error.to_string()))??;
    Ok(Json(json!({ "path": path })))
}

fn open_folder_picker() -> Result<Option<String>, Problem> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()

$source = @'
using System;
using System.Runtime.InteropServices;

[Flags]
public enum FileOpenOptions : uint {
    PickFolders = 0x00000020,
    ForceFileSystem = 0x00000040,
    PathMustExist = 0x00000800,
    DontAddToRecent = 0x02000000
}

public enum ShellDisplayName : uint {
    FileSystemPath = 0x80058000
}

[ComImport]
[Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE")]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IShellItem {
    void BindToHandler(IntPtr pbc, ref Guid bhid, ref Guid riid, out IntPtr ppv);
    void GetParent(out IShellItem ppsi);
    void GetDisplayName(ShellDisplayName sigdnName, out IntPtr ppszName);
    void GetAttributes(uint sfgaoMask, out uint psfgaoAttribs);
    void Compare(IShellItem psi, uint hint, out int piOrder);
}

[ComImport]
[Guid("D57C7288-D4AD-4768-BE02-9D969532D960")]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IFileOpenDialog {
    [PreserveSig] int Show(IntPtr parent);
    void SetFileTypes(uint cFileTypes, IntPtr rgFilterSpec);
    void SetFileTypeIndex(uint iFileType);
    void GetFileTypeIndex(out uint piFileType);
    void Advise(IntPtr pfde, out uint pdwCookie);
    void Unadvise(uint dwCookie);
    void SetOptions(FileOpenOptions fos);
    void GetOptions(out FileOpenOptions pfos);
    void SetDefaultFolder(IShellItem psi);
    void SetFolder(IShellItem psi);
    void GetFolder(out IShellItem ppsi);
    void GetCurrentSelection(out IShellItem ppsi);
    void SetFileName([MarshalAs(UnmanagedType.LPWStr)] string pszName);
    void GetFileName(out IntPtr pszName);
    void SetTitle([MarshalAs(UnmanagedType.LPWStr)] string pszTitle);
    void SetOkButtonLabel([MarshalAs(UnmanagedType.LPWStr)] string pszText);
    void SetFileNameLabel([MarshalAs(UnmanagedType.LPWStr)] string pszLabel);
    void GetResult(out IShellItem ppsi);
    void AddPlace(IShellItem psi, int fdap);
    void SetDefaultExtension([MarshalAs(UnmanagedType.LPWStr)] string pszDefaultExtension);
    void Close(int hr);
    void SetClientGuid(ref Guid guid);
    void ClearClientData();
    void SetFilter(IntPtr pFilter);
    void GetResults(out IntPtr ppenum);
    void GetSelectedItems(out IntPtr ppsai);
}

[ComImport]
[Guid("DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7")]
public class FileOpenDialogCom { }

public static class ModernFolderPicker {
    private const int Cancelled = unchecked((int)0x800704C7);

    public static string Pick(IntPtr owner) {
        IFileOpenDialog dialog = (IFileOpenDialog)new FileOpenDialogCom();
        try {
            FileOpenOptions options;
            dialog.GetOptions(out options);
            dialog.SetOptions(options | FileOpenOptions.PickFolders | FileOpenOptions.ForceFileSystem | FileOpenOptions.PathMustExist | FileOpenOptions.DontAddToRecent);
            dialog.SetTitle("Chọn thư mục dự án");
            dialog.SetOkButtonLabel("Chọn thư mục");

            int result = dialog.Show(owner);
            if (result == Cancelled) return null;
            if (result != 0) Marshal.ThrowExceptionForHR(result);

            IShellItem item;
            dialog.GetResult(out item);
            try {
                IntPtr value;
                item.GetDisplayName(ShellDisplayName.FileSystemPath, out value);
                try { return Marshal.PtrToStringUni(value); }
                finally { Marshal.FreeCoTaskMem(value); }
            } finally {
                if (item != null) Marshal.FinalReleaseComObject(item);
            }
        } finally {
            if (dialog != null) Marshal.FinalReleaseComObject(dialog);
        }
    }
}
'@
Add-Type -TypeDefinition $source -Language CSharp

$owner = New-Object System.Windows.Forms.Form
$owner.Text = 'ChatCMD'
$owner.ShowInTaskbar = $false
$owner.TopMost = $true
$owner.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$owner.Size = New-Object System.Drawing.Size(1, 1)
$owner.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedToolWindow
$owner.Opacity = 0
$owner.Show()
$owner.Activate()
$owner.BringToFront()

try {
    $path = [ModernFolderPicker]::Pick($owner.Handle)
    if ($path) { Write-Output $path }
} finally {
    $owner.Close()
    $owner.Dispose()
}
"#;
        let output = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-STA",
                "-Command",
                script,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| folder_problem(error.to_string()))?;
        if !output.status.success() {
            return Err(folder_problem(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(clean_path(&output.stdout))
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"Chọn thư mục dự án\")",
            ])
            .output()
            .map_err(|error| folder_problem(error.to_string()))?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(clean_path(&output.stdout).map(|value| value.trim_end_matches('/').to_string()))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let output = Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=Chọn thư mục dự án",
            ])
            .output()
            .map_err(|error| folder_problem(error.to_string()))?;
        if !output.status.success() {
            return Ok(None);
        }
        return Ok(clean_path(&output.stdout));
    }
}

fn clean_path(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_string();
    (!value.is_empty()).then_some(value)
}
