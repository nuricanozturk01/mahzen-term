//! Parser ceiling probe: feeds 200 MB straight into the VT state (no PTY, no UI).
//! Run: cargo run --release -p mahzenterm-term --example parse_ceiling

use std::time::Instant;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

struct Dims;
impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        40
    }
    fn screen_lines(&self) -> usize {
        40
    }
    fn columns(&self) -> usize {
        120
    }
}

fn main() {
    let mut line = vec![b'a'; 120];
    line.extend_from_slice(b"\r\n");
    let chunk: Vec<u8> = line.iter().cycle().take(64 * 1024).copied().collect();
    let mut term = Term::new(Config::default(), &Dims, VoidListener);
    let mut parser: Processor = Processor::new();
    let total = 200 * 1024 * 1024;
    let start = Instant::now();
    let mut fed = 0;
    while fed < total {
        parser.advance(&mut term, &chunk);
        fed += chunk.len();
    }
    let s = start.elapsed().as_secs_f64();
    println!("parser only: {:.0} MB/s", (fed as f64 / 1_048_576.0) / s);
}
