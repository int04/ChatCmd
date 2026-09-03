use std::{
    ffi::OsStr,
    io::{BufRead as _, BufReader},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
};

pub fn spawn_test_helper(test_name: &str, root: &Path) -> Child {
    Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", test_name, "--nocapture"])
        .env(OsStr::new("CHATCMD_CRASH_HELPER_ROOT"), root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn crash helper")
}

pub fn kill_at_marker(mut child: Child, marker: &str) -> ExitStatus {
    let stdout = child.stdout.take().expect("crash helper stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        assert_ne!(reader.read_line(&mut line).expect("read crash marker"), 0);
        if line.contains(marker) {
            break;
        }
        line.clear();
    }
    child
        .kill()
        .expect("kill crash helper at deterministic phase");
    child.wait().expect("reap crash helper")
}
