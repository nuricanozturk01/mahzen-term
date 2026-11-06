//! Undo history: edits are recorded with what they replaced, so each one can be reversed.

/// One change: at character index `at`, `removed` was replaced by `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Edit {
    /// The edit that undoes this one.
    pub fn inverse(&self) -> Edit {
        Edit {
            at: self.at,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }
}

/// What kind of change a step was; consecutive typing (or consecutive backspaces) joins into one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Typing,
    Deleting,
    Other,
}

/// A group of edits that undo and redo together, with the selections to restore.
#[derive(Debug, Clone)]
pub(crate) struct Step {
    /// Unique id, kept when the step moves between the undo and redo stacks.
    pub id: u64,
    pub edits: Vec<Edit>,
    pub kind: Kind,
    pub before: Vec<(usize, usize)>,
    pub after: Vec<(usize, usize)>,
}

/// Undo and redo stacks.
#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
}

/// Most steps kept.
const MAX_STEPS: usize = 10_000;

impl History {
    /// Record a step. A coalescable step extends the previous one when it directly follows it.
    pub fn push(&mut self, mut step: Step, join: bool) {
        self.redo.clear();
        if join {
            if let Some(last) = self.undo.last_mut() {
                if last.kind == step.kind && step.kind != Kind::Other {
                    last.edits.extend(step.edits);
                    last.after = step.after;
                    return;
                }
            }
        }
        self.next_id += 1;
        step.id = self.next_id;
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    pub fn pop_undo(&mut self) -> Option<Step> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, step: Step) {
        self.redo.push(step);
    }

    pub fn pop_redo(&mut self) -> Option<Step> {
        self.redo.pop()
    }

    pub fn push_undo_keep_redo(&mut self, step: Step) {
        self.undo.push(step);
    }

    /// Stop joining: the next edit starts a new step.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.kind = Kind::Other;
        }
    }

    /// Id of the newest undoable step (`None` when there is nothing to undo): identifies the state of the text.
    pub fn top_id(&self) -> Option<u64> {
        self.undo.last().map(|s| s.id)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
