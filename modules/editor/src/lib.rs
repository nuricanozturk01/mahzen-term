//! Built-in editor, first slice (PLAN.md M8.1): a UI-free text buffer.
//!
//! Text lives in a rope. Cursors and selections are character indexes; movement and deletion work on grapheme
//! clusters, so an emoji with a skin tone or `e` plus a combining accent is one step. Every change is an [`Edit`]
//! recorded in an undo history, with typing coalesced into one step. Several selections are supported from the start.
//! Saving is atomic and refuses to overwrite a file that changed on disk since it was read.
//!
//! The editor pane, syntax colours and language servers come in later slices (see the M8 design document).

mod buffer;
mod history;
mod io;
mod keymap;
mod search;
mod state;
mod view;

pub use buffer::{Buffer, Move, Selection};
pub use history::Edit;
pub use io::{LineEnding, OpenError, SaveError, SaveJob, SavedStamp, MAX_FILE_BYTES};
pub use keymap::{apply, Applied, Keymap};
pub use search::{find_all, Match, SearchOptions};
pub use state::{
    CompletionItem, Completions, Diagnostic, EditorState, Effect, Mode, Severity, MAX_SUGGESTIONS,
    TAB_WIDTH,
};
pub use view::{char_at_visual, view, visual_col, EditorPane, Msg};

#[cfg(test)]
mod tests;
