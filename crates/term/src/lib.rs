//! Terminal engine: PTY session, VT state (via `alacritty_terminal`), keyboard encoding.
//!
//! UI-independent: the renderer consumes [`Snapshot`]s and never touches the VT internals.

pub mod gfx;
pub mod keys;
pub mod osc133;
pub mod palette;
pub mod session;
pub mod sixel;

pub use keys::{encode_key, Key, Mods};
pub use palette::{Palette, Rgb};
pub use session::{
    is_openable, Cell, CellFlags, Cursor, LinkSpan, Mark, Session, SessionError, Size, Snapshot,
    SpawnOptions,
};
