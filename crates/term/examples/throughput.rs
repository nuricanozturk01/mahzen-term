//! Parser throughput probe: pipes a large amount of text through a real PTY session.
//! Run: cargo run --release -p mahzenterm-term --example throughput

use std::time::{Duration, Instant};

use mahzenterm_term::{Session, Size, SpawnOptions};

fn main() {
    let opts = SpawnOptions {
        command: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            "head -c 200000000 /dev/zero | tr '\\0' 'a' | fold -w 120".into(),
        ]),
        ..SpawnOptions::default()
    };
    let start = Instant::now();
    let session = Session::spawn(opts, Size::new(120, 40), || {}).expect("spawn");
    while !session.exited() && start.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "200 MB in {secs:.2}s = {:.0} MB/s (includes fold/tr producer)",
        200.0 / secs
    );
}
