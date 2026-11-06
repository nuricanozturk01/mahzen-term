//! Sample MahzenTerm plugin: a UTC clock card. See docs/plugins.md for the ABI.

use std::cell::UnsafeCell;
use std::fmt::Write as _;

#[link(wasm_import_module = "mz")]
extern "C" {
    fn now_ms() -> i64;
}

const BUF: usize = 4096;

struct Buf(UnsafeCell<[u8; BUF]>);
// SAFETY: a WebAssembly instance runs one thread; the host never calls into two exports at once.
unsafe impl Sync for Buf {}

static INPUT: Buf = Buf(UnsafeCell::new([0; BUF]));
static OUTPUT: Buf = Buf(UnsafeCell::new([0; BUF]));

/// The host asks for room for its input (`width=<n>`); the buffer is static and fixed size.
#[no_mangle]
pub extern "C" fn mz_alloc(len: i32) -> i32 {
    if usize::try_from(len).map_or(true, |l| l > BUF) {
        return 0;
    }
    INPUT.0.get() as i32
}

/// Draw the card: returns `(ptr << 32) | len` of the answer text.
#[no_mangle]
pub extern "C" fn mz_render(_ptr: i32, _len: i32) -> i64 {
    // SAFETY: see `Buf`; the output buffer is only written here and read by the host afterwards.
    let out = unsafe { &mut *OUTPUT.0.get() };
    let secs = unsafe { now_ms() } / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let mut text = String::new();
    let _ = write!(text, "title=Clock\nbadge=UTC\nrow=accent|time|{h:02}:{m:02}:{s:02}\nrow=dim|day|{}\n", secs / 86_400);
    let n = text.len().min(BUF);
    out[..n].copy_from_slice(&text.as_bytes()[..n]);
    ((out.as_ptr() as i64) << 32) | n as i64
}
