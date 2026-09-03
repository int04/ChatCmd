use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const BUFFER_BYTES: usize = 64 * 1024;
pub const MARKER: &[u8] = b"CHATCMD-PLAN23-MARKER";

#[derive(Debug)]
pub struct LargeFileFixture {
    pub path: PathBuf,
    pub size: u64,
    pub marker_offsets: [u64; 3],
    pub sha256: String,
}

/// Generates deterministic content without retaining the complete fixture in memory.
pub fn write_large_file(path: &Path, size: u64, seed: u64) -> LargeFileFixture {
    assert!(size >= (MARKER.len() * 3) as u64);
    let file = File::create(path).expect("create large fixture");
    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, file);
    let mut block = [0_u8; BUFFER_BYTES];
    let mut state = seed.max(1);
    let mut remaining = size;
    while remaining != 0 {
        for byte in &mut block {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = b'a' + (state % 26) as u8;
        }
        let count = remaining.min(BUFFER_BYTES as u64) as usize;
        writer
            .write_all(&block[..count])
            .expect("stream fixture block");
        remaining -= count as u64;
    }
    writer.flush().expect("flush fixture");
    drop(writer);

    let marker_offsets = [0, size / 2, size - MARKER.len() as u64];
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("reopen fixture for markers");
    for offset in marker_offsets {
        file.seek(SeekFrom::Start(offset)).expect("seek marker");
        file.write_all(MARKER).expect("write marker");
    }
    file.sync_all().expect("sync fixture");

    let mut hasher = Sha256::new();
    let mut reader =
        BufReader::with_capacity(BUFFER_BYTES, File::open(path).expect("open fixture"));
    loop {
        let count = reader
            .read(&mut block)
            .expect("read fixture checksum block");
        if count == 0 {
            break;
        }
        hasher.update(&block[..count]);
    }
    LargeFileFixture {
        path: path.to_path_buf(),
        size,
        marker_offsets,
        sha256: format!("{:x}", hasher.finalize()),
    }
}

pub fn write_sparse_file(path: &Path, size: u64) -> LargeFileFixture {
    let file = File::create(path).expect("create sparse fixture");
    file.set_len(size).expect("size sparse fixture");
    drop(file);
    let marker_offsets = [0, size / 2, size - MARKER.len() as u64];
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open sparse fixture");
    for offset in marker_offsets {
        file.seek(SeekFrom::Start(offset)).expect("seek marker");
        file.write_all(MARKER).expect("write marker");
    }
    file.sync_all().expect("sync sparse fixture");
    LargeFileFixture {
        path: path.to_path_buf(),
        size,
        marker_offsets,
        sha256: "not-computed-for-sparse-fixture".to_owned(),
    }
}

pub fn write_tree(root: &Path, files: usize, depth: usize) {
    let depth = depth.max(1);
    for index in 0..files {
        let mut directory = root.to_path_buf();
        for level in 0..depth {
            directory.push(format!("d{:02}", (index + level) % 17));
        }
        fs::create_dir_all(&directory).expect("create fixture directory");
        fs::write(
            directory.join(format!("f{index:06}.txt")),
            format!("seed={index:06} alpha beta gamma\n"),
        )
        .expect("write fixture file");
    }
    fs::create_dir_all(root.join("node_modules")).expect("create ignored directory");
    fs::write(root.join("node_modules/ignored.txt"), b"needle\n").expect("write ignored fixture");
    fs::write(root.join(".gitignore"), b"node_modules/\n").expect("write ignore rules");
}
