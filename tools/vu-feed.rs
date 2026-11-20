#![allow(clippy::unwrap_used, clippy::expect_used)]
// developer tools: a panic is an acceptable way for a one-off
// CLI tool to fail

//! dev feeder for squeezelite's `-v` visualizer shmem format
//! (requires a squeezelite built with -DVISEXPORT): writes a
//! modulated stereo sine into /dev/shm/squeezelite-dev so mestro's
//! VU reader can be exercised on machines without squeezelite.
//!
//! Layout per squeezelite's output_vis.c (64-bit Linux):
//!   rwlock[56] | buf_size u32 | buf_index u32 | running u8 |
//!   pad | rate u32 | pad | updated i64 | buffer s16[16384]

use std::fs::OpenOptions;
use std::os::unix::fs::FileExt;
use std::time::Duration;

const VIS_BUF_SIZE: usize = 16384;
const OFF_BUF_SIZE: u64 = 56;
const OFF_BUF_INDEX: u64 = 60;
const OFF_RUNNING: u64 = 64;
const OFF_RATE: u64 = 68;
const OFF_BUFFER: u64 = 80;
const RATE: u32 = 44100;

fn main() {
    let path = std::env::temp_dir().join("../dev/shm/squeezelite-dev");
    let path = if path.exists() {
        path
    } else {
        std::path::PathBuf::from("/dev/shm/squeezelite-dev")
    };
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .expect("open shm");
    f.set_len(OFF_BUFFER + (VIS_BUF_SIZE as u64) * 2)
        .expect("set shm size");

    f.write_all_at(&(VIS_BUF_SIZE as u32).to_le_bytes(), OFF_BUF_SIZE)
        .unwrap();
    f.write_all_at(&1u8.to_le_bytes(), OFF_RUNNING).unwrap();
    f.write_all_at(&RATE.to_le_bytes(), OFF_RATE).unwrap();

    let mut idx: usize = 0; // sample index, always pair-aligned
    let mut t = 0.0f32; // seconds
    loop {
        // one 33ms block of frames, envelope for visible movement
        for _ in 0..(RATE as usize / 30) {
            let env = 0.45 + 0.45 * (t * 0.5).sin();
            let l = (env * 0.9 * (t * 2.0 * std::f32::consts::PI * 440.0).sin() * 32767.0) as i16;
            let r = (env * 0.8 * (t * 2.0 * std::f32::consts::PI * 437.0).sin() * 32767.0) as i16;
            let pair = [l.to_le_bytes(), r.to_le_bytes()].concat();
            f.write_all_at(&pair, OFF_BUFFER + idx as u64 * 2).unwrap();
            idx = (idx + 2) % VIS_BUF_SIZE;
            t += 1.0 / RATE as f32;
        }
        f.write_all_at(&(idx as u32).to_le_bytes(), OFF_BUF_INDEX)
            .unwrap();
        std::thread::sleep(Duration::from_millis(33));
    }
}
