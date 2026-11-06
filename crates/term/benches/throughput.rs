//! Throughput of the output path: the escape-sequence scanners that run on every PTY read, and a whole session
//! swallowing a large colourful file. `cargo bench -p mahzenterm-term`.
//!
//! PERFORMANCE BUDGET (CLAUDE.md): a change that touches this path must not slow it by more than 10 %.

use std::io::Write;
use std::time::{Duration, Instant};

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use mahzenterm_term::session::{Session, Size, SpawnOptions};
use mahzenterm_term::{gfx, osc133};

/// About 4 MB of what real programs print: coloured text lines with a few prompt marks.
fn sample() -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0u32;
    while out.len() < 4 * 1024 * 1024 {
        write!(
            out,
            "\x1b[3{}mline {i:08} \x1b[1;4{}mthe quick brown fox jumps over the lazy dog\x1b[0m \x1b[38;5;{}m0123456789\x1b[0m\r\n",
            i % 8,
            i % 7,
            i % 256
        )
        .ok();
        if i.is_multiple_of(500) {
            out.extend_from_slice(b"\x1b]133;A\x07$ \x1b]133;C\x07");
        }
        i += 1;
    }
    out
}

fn scanners(c: &mut Criterion) {
    let data = sample();
    let mut g = c.benchmark_group("scanners");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.bench_function("osc133 + osc7 (64 KiB reads)", |b| {
        b.iter(|| {
            let mut s = osc133::Scanner::default();
            let mut n = 0;
            for chunk in data.chunks(64 * 1024) {
                n += s.scan(black_box(chunk)).len();
            }
            n
        });
    });
    g.bench_function("graphics: kitty, sixel, iterm (64 KiB reads)", |b| {
        b.iter(|| {
            let mut s = gfx::Scanner::default();
            let mut n = 0;
            for chunk in data.chunks(64 * 1024) {
                n += s.scan(black_box(chunk)).len();
            }
            n
        });
    });
    g.finish();
}

/// Run `cat file` in a real session and wait until its last line is on screen.
fn session_swallows(path: &str) -> Duration {
    let opts = SpawnOptions {
        command: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("cat {path}; printf '__DONE__'; sleep 5"),
        ]),
        ..SpawnOptions::default()
    };
    let start = Instant::now();
    let s = Session::spawn(opts, Size::new(200, 50), || {}).expect("spawn");
    loop {
        let snap = s.snapshot();
        if (0..snap.rows).any(|r| snap.row_text(r).contains("__DONE__")) {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the session did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn session(c: &mut Criterion) {
    let data = sample();
    let dir = std::env::temp_dir().join(format!("mz-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("sample.txt");
    std::fs::write(&file, &data).expect("write sample");
    let path = file.display().to_string();
    let mut g = c.benchmark_group("session");
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(10));
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.bench_function("pty -> parser -> grid (4 MB coloured text)", |b| {
        b.iter_custom(|iters| (0..iters).map(|_| session_swallows(&path)).sum());
    });
    g.finish();
    let _ = std::fs::remove_dir_all(dir);
}

criterion_group!(benches, scanners, session);
criterion_main!(benches);
