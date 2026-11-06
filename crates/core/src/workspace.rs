//! Workspace model: `Window -> Tab -> PaneTree`, plus reserved floating layer and docks.
//!
//! Everything here is pure data and pure operations (no I/O, no UI), serializable with serde so session
//! restore can persist the exact same structure.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Smallest and largest split ratio a node may hold.
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

/// Stable pane identifier, unique within a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(pub u64);

/// Stable tab identifier, unique within a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// How a split lays out its two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Children sit side by side (left | right).
    Horizontal,
    /// Children are stacked (top / bottom).
    Vertical,
}

/// Direction for keyboard focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// What a pane shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    /// A shell session. `None` fields mean "use the default".
    Terminal {
        cwd: Option<PathBuf>,
        shell: Option<String>,
        /// Arguments for `shell` (a pane that runs one program, such as the file viewer).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// A module-provided panel.
    Panel { module: String, panel: String },
}

impl PaneKind {
    /// A terminal with default shell and cwd.
    pub fn terminal() -> Self {
        Self::Terminal {
            cwd: None,
            shell: None,
            args: Vec::new(),
        }
    }
}

/// A leaf of the pane tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    /// User-assigned title; `None` falls back to the process title.
    pub title: Option<String>,
}

/// Pane tree node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(Pane),
    Split {
        axis: Axis,
        /// Share of the space given to `a`, always within `MIN_RATIO..=MAX_RATIO`.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// Rectangle in normalized tab coordinates (`0.0..=1.0`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// The whole tab area.
    pub const FULL: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    fn split(self, axis: Axis, ratio: f32) -> (Rect, Rect) {
        match axis {
            Axis::Horizontal => {
                let wa = self.w * ratio;
                (
                    Rect { w: wa, ..self },
                    Rect {
                        x: self.x + wa,
                        w: self.w - wa,
                        ..self
                    },
                )
            }
            Axis::Vertical => {
                let ha = self.h * ratio;
                (
                    Rect { h: ha, ..self },
                    Rect {
                        y: self.y + ha,
                        h: self.h - ha,
                        ..self
                    },
                )
            }
        }
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// A split of the pane tree as seen by a UI that draws draggable separators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitInfo {
    /// First leaf of the `a` child and of the `b` child; together they identify the split uniquely.
    pub a_first: PaneId,
    pub b_first: PaneId,
    pub axis: Axis,
    pub ratio: f32,
    /// Area covered by the whole split (both children).
    pub rect: Rect,
}

/// A pane floating above the tree (layout reserved; rendering arrives later).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatingPane {
    pub pane: Pane,
    pub rect: Rect,
}

/// Edge a dock is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

/// Reference to a module panel hosted in a dock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelRef {
    pub module: String,
    pub panel: String,
}

/// A dock: an edge strip hosting module panels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    pub panels: Vec<PanelRef>,
    /// Fraction of the window the dock takes when visible.
    pub size: f32,
    pub visible: bool,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            panels: Vec::new(),
            size: 0.25,
            visible: false,
        }
    }
}

/// Workspace operation errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkspaceError {
    #[error("pane {0:?} does not exist; pick an existing pane")]
    NoSuchPane(PaneId),
    #[error("tab {0:?} does not exist")]
    NoSuchTab(TabId),
    #[error("cannot close the last pane of a tab; close the tab instead")]
    LastPane,
    #[error("cannot close the last tab")]
    LastTab,
}

/// One tab: a pane tree plus optional floating panes and docks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub name: Option<String>,
    pub root: Node,
    pub focus: PaneId,
    pub zoomed: Option<PaneId>,
    #[serde(default)]
    pub floating: Vec<FloatingPane>,
    #[serde(default)]
    pub docks: BTreeMap<DockSide, Dock>,
    next_pane: u64,
}

impl Tab {
    /// A tab with a single pane of `kind`.
    pub fn new(id: TabId, kind: PaneKind) -> Self {
        let pane = Pane {
            id: PaneId(0),
            kind,
            title: None,
        };
        Self {
            id,
            name: None,
            root: Node::Leaf(pane),
            focus: PaneId(0),
            zoomed: None,
            floating: Vec::new(),
            docks: BTreeMap::new(),
            next_pane: 1,
        }
    }

    /// Ids of all tree panes in layout order (left/top first).
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_ids(&self.root, &mut out);
        out
    }

    /// Look up a tree pane.
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        find_pane(&self.root, id)
    }

    /// Mutable lookup of a tree pane.
    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        find_pane_mut(&mut self.root, id)
    }

    /// Normalized rectangle of every tree pane. A zoomed pane fills the tab alone.
    pub fn layout(&self) -> Vec<(PaneId, Rect)> {
        if let Some(z) = self.zoomed {
            return vec![(z, Rect::FULL)];
        }
        let mut out = Vec::new();
        layout_node(&self.root, Rect::FULL, &mut out);
        out
    }

    /// All splits with their areas, parents before children. Empty while a pane is zoomed.
    pub fn splits(&self) -> Vec<SplitInfo> {
        let mut out = Vec::new();
        if self.zoomed.is_none() {
            splits_node(&self.root, Rect::FULL, &mut out);
        }
        out
    }

    /// Set the ratio of the split identified by `(a_first, b_first)` (see [`SplitInfo`]); clamped.
    /// Returns false when no such split exists.
    pub fn set_split_ratio(&mut self, a_first: PaneId, b_first: PaneId, ratio: f32) -> bool {
        set_ratio_node(&mut self.root, a_first, b_first, ratio)
    }

    /// Split `target` along `axis`, placing a new pane of `kind` second. The new pane gets focus.
    pub fn split(
        &mut self,
        target: PaneId,
        axis: Axis,
        kind: PaneKind,
    ) -> Result<PaneId, WorkspaceError> {
        if self.pane(target).is_none() {
            return Err(WorkspaceError::NoSuchPane(target));
        }
        let new_id = PaneId(self.next_pane);
        self.next_pane += 1;
        let new_pane = Pane {
            id: new_id,
            kind,
            title: None,
        };
        split_node(&mut self.root, target, axis, new_pane);
        self.zoomed = None;
        self.focus = new_id;
        Ok(new_id)
    }

    /// Close a tree pane; its sibling takes the space. Focus moves to the sibling's first pane.
    pub fn close(&mut self, target: PaneId) -> Result<(), WorkspaceError> {
        if self.pane(target).is_none() {
            return Err(WorkspaceError::NoSuchPane(target));
        }
        if matches!(self.root, Node::Leaf(_)) {
            return Err(WorkspaceError::LastPane);
        }
        let root = std::mem::replace(&mut self.root, Node::Leaf(dummy_pane()));
        let (new_root, sibling_first) = remove_node(root, target);
        self.root = new_root.unwrap_or_else(|| Node::Leaf(dummy_pane()));
        if self.focus == target {
            if let Some(first) = sibling_first {
                self.focus = first;
            }
        }
        if self.zoomed == Some(target) {
            self.zoomed = None;
        }
        Ok(())
    }

    /// Move focus to the nearest pane in `dir`. Returns the new focus, or `None` if nothing lies that way.
    pub fn focus_dir(&mut self, dir: Direction) -> Option<PaneId> {
        let mut rects = Vec::new();
        layout_node(&self.root, Rect::FULL, &mut rects);
        let cur = rects.iter().find(|(id, _)| *id == self.focus)?.1;
        let (cx, cy) = cur.center();
        const EPS: f32 = 1e-4;
        let best = rects
            .iter()
            .filter(|(id, _)| *id != self.focus)
            .filter_map(|(id, r)| {
                let (rx, ry) = r.center();
                let overlap = match dir {
                    Direction::Left | Direction::Right => {
                        r.y < cur.y + cur.h - EPS && r.y + r.h > cur.y + EPS
                    }
                    Direction::Up | Direction::Down => {
                        r.x < cur.x + cur.w - EPS && r.x + r.w > cur.x + EPS
                    }
                };
                let ahead = match dir {
                    Direction::Left => rx < cx - EPS,
                    Direction::Right => rx > cx + EPS,
                    Direction::Up => ry < cy - EPS,
                    Direction::Down => ry > cy + EPS,
                };
                (overlap && ahead).then(|| {
                    let dist = (rx - cx).abs() + (ry - cy).abs();
                    (*id, dist)
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        self.focus = best.0;
        self.zoomed = None;
        Some(best.0)
    }

    /// Grow (`delta > 0`) or shrink the pane inside its nearest ancestor split on `axis`.
    /// `delta` is a ratio change; the result is clamped.
    pub fn resize(&mut self, target: PaneId, axis: Axis, delta: f32) -> Result<(), WorkspaceError> {
        if self.pane(target).is_none() {
            return Err(WorkspaceError::NoSuchPane(target));
        }
        resize_node(&mut self.root, target, axis, delta, true);
        Ok(())
    }

    /// Move the nearest border of `target` on `axis` by `delta` of the split's size: positive moves it
    /// right/down, negative left/up, whichever side of the border the pane is on. Result is clamped.
    pub fn move_border(
        &mut self,
        target: PaneId,
        axis: Axis,
        delta: f32,
    ) -> Result<(), WorkspaceError> {
        if self.pane(target).is_none() {
            return Err(WorkspaceError::NoSuchPane(target));
        }
        resize_node(&mut self.root, target, axis, delta, false);
        Ok(())
    }

    /// Toggle zoom on `target` (zoomed pane fills the tab).
    pub fn toggle_zoom(&mut self, target: PaneId) -> Result<(), WorkspaceError> {
        if self.pane(target).is_none() {
            return Err(WorkspaceError::NoSuchPane(target));
        }
        self.zoomed = if self.zoomed == Some(target) {
            None
        } else {
            self.focus = target;
            Some(target)
        };
        Ok(())
    }

    /// Swap the positions of two tree panes (ids stay attached to their content).
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> Result<(), WorkspaceError> {
        for id in [a, b] {
            if self.pane(id).is_none() {
                return Err(WorkspaceError::NoSuchPane(id));
            }
        }
        if a == b {
            return Ok(());
        }
        let (Some(pa), Some(pb)) = (self.pane(a).cloned(), self.pane(b).cloned()) else {
            return Ok(());
        };
        // Park slot `a` under a sentinel id so the two lookups below never see duplicate ids.
        let sentinel = PaneId(u64::MAX);
        if let Some(slot) = self.pane_mut(a) {
            slot.id = sentinel;
        }
        if let Some(slot) = self.pane_mut(b) {
            *slot = pa;
        }
        if let Some(slot) = self.pane_mut(sentinel) {
            *slot = pb;
        }
        Ok(())
    }
}

fn dummy_pane() -> Pane {
    Pane {
        id: PaneId(u64::MAX),
        kind: PaneKind::terminal(),
        title: None,
    }
}

fn collect_ids(n: &Node, out: &mut Vec<PaneId>) {
    match n {
        Node::Leaf(p) => out.push(p.id),
        Node::Split { a, b, .. } => {
            collect_ids(a, out);
            collect_ids(b, out);
        }
    }
}

fn find_pane(n: &Node, id: PaneId) -> Option<&Pane> {
    match n {
        Node::Leaf(p) => (p.id == id).then_some(p),
        Node::Split { a, b, .. } => find_pane(a, id).or_else(|| find_pane(b, id)),
    }
}

fn find_pane_mut(n: &mut Node, id: PaneId) -> Option<&mut Pane> {
    match n {
        Node::Leaf(p) => (p.id == id).then_some(p),
        Node::Split { a, b, .. } => find_pane_mut(a, id).or_else(|| find_pane_mut(b, id)),
    }
}

fn layout_node(n: &Node, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match n {
        Node::Leaf(p) => out.push((p.id, r)),
        Node::Split { axis, ratio, a, b } => {
            let (ra, rb) = r.split(*axis, *ratio);
            layout_node(a, ra, out);
            layout_node(b, rb, out);
        }
    }
}

fn splits_node(n: &Node, r: Rect, out: &mut Vec<SplitInfo>) {
    if let Node::Split { axis, ratio, a, b } = n {
        out.push(SplitInfo {
            a_first: first_id(a),
            b_first: first_id(b),
            axis: *axis,
            ratio: *ratio,
            rect: r,
        });
        let (ra, rb) = r.split(*axis, *ratio);
        splits_node(a, ra, out);
        splits_node(b, rb, out);
    }
}

fn set_ratio_node(n: &mut Node, a_first: PaneId, b_first: PaneId, new: f32) -> bool {
    let Node::Split { ratio, a, b, .. } = n else {
        return false;
    };
    if first_id(a) == a_first && first_id(b) == b_first {
        *ratio = new.clamp(MIN_RATIO, MAX_RATIO);
        return true;
    }
    set_ratio_node(a, a_first, b_first, new) || set_ratio_node(b, a_first, b_first, new)
}

fn contains(n: &Node, id: PaneId) -> bool {
    find_pane(n, id).is_some()
}

fn split_node(n: &mut Node, target: PaneId, axis: Axis, new_pane: Pane) -> bool {
    match n {
        Node::Leaf(p) if p.id == target => {
            let old = std::mem::replace(n, Node::Leaf(dummy_pane()));
            *n = Node::Split {
                axis,
                ratio: 0.5,
                a: Box::new(old),
                b: Box::new(Node::Leaf(new_pane)),
            };
            true
        }
        Node::Leaf(_) => false,
        Node::Split { a, b, .. } => {
            if contains(a, target) {
                split_node(a, target, axis, new_pane)
            } else {
                split_node(b, target, axis, new_pane)
            }
        }
    }
}

/// Remove `target`; returns the remaining node and the first pane id of the promoted sibling.
fn remove_node(n: Node, target: PaneId) -> (Option<Node>, Option<PaneId>) {
    match n {
        Node::Leaf(p) if p.id == target => (None, None),
        leaf @ Node::Leaf(_) => (Some(leaf), None),
        Node::Split { axis, ratio, a, b } => {
            if matches!(&*a, Node::Leaf(p) if p.id == target) {
                let first = first_id(&b);
                return (Some(*b), Some(first));
            }
            if matches!(&*b, Node::Leaf(p) if p.id == target) {
                let first = first_id(&a);
                return (Some(*a), Some(first));
            }
            if contains(&a, target) {
                let (na, f) = remove_node(*a, target);
                let a = na.map_or_else(|| Box::new(Node::Leaf(dummy_pane())), Box::new);
                (Some(Node::Split { axis, ratio, a, b }), f)
            } else {
                let (nb, f) = remove_node(*b, target);
                let b = nb.map_or_else(|| Box::new(Node::Leaf(dummy_pane())), Box::new);
                (Some(Node::Split { axis, ratio, a, b }), f)
            }
        }
    }
}

fn first_id(n: &Node) -> PaneId {
    match n {
        Node::Leaf(p) => p.id,
        Node::Split { a, .. } => first_id(a),
    }
}

/// Returns true once the adjustment was applied.
fn resize_node(n: &mut Node, target: PaneId, axis: Axis, delta: f32, relative: bool) -> bool {
    let Node::Split {
        axis: ax,
        ratio,
        a,
        b,
    } = n
    else {
        return false;
    };
    let in_a = contains(a, target);
    let child = if in_a { a } else { b };
    // Prefer the deepest matching ancestor.
    if resize_node(child, target, axis, delta, relative) {
        return true;
    }
    if *ax == axis {
        let signed = if relative && !in_a { -delta } else { delta };
        *ratio = (*ratio + signed).clamp(MIN_RATIO, MAX_RATIO);
        return true;
    }
    false
}

/// A window: ordered tabs and the active one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub tabs: Vec<Tab>,
    pub active: TabId,
    next_tab: u64,
}

impl Window {
    /// A window with one tab holding one terminal pane.
    pub fn new() -> Self {
        let tab = Tab::new(TabId(0), PaneKind::terminal());
        Self {
            active: tab.id,
            tabs: vec![tab],
            next_tab: 1,
        }
    }

    /// Open a new tab after the active one and activate it.
    pub fn new_tab(&mut self, kind: PaneKind) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        let at = self
            .index_of(self.active)
            .map_or(self.tabs.len(), |i| i + 1);
        self.tabs.insert(at, Tab::new(id, kind));
        self.active = id;
        id
    }

    /// Close a tab; the neighbour becomes active. The last tab cannot be closed.
    pub fn close_tab(&mut self, id: TabId) -> Result<(), WorkspaceError> {
        let idx = self.index_of(id).ok_or(WorkspaceError::NoSuchTab(id))?;
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::LastTab);
        }
        self.tabs.remove(idx);
        if self.active == id {
            self.active = self.tabs[idx.min(self.tabs.len() - 1)].id;
        }
        Ok(())
    }

    /// Activate a tab.
    pub fn activate(&mut self, id: TabId) -> Result<(), WorkspaceError> {
        self.index_of(id).ok_or(WorkspaceError::NoSuchTab(id))?;
        self.active = id;
        Ok(())
    }

    /// Activate the next (`step = 1`) or previous (`step = -1`) tab, wrapping.
    pub fn cycle(&mut self, step: i32) {
        let len = self.tabs.len() as i32;
        if let Some(i) = self.index_of(self.active) {
            let n = (i as i32 + step).rem_euclid(len) as usize;
            self.active = self.tabs[n].id;
        }
    }

    /// Move a tab to position `to` (clamped).
    pub fn move_tab(&mut self, id: TabId, to: usize) -> Result<(), WorkspaceError> {
        let idx = self.index_of(id).ok_or(WorkspaceError::NoSuchTab(id))?;
        let tab = self.tabs.remove(idx);
        self.tabs.insert(to.min(self.tabs.len()), tab);
        Ok(())
    }

    /// The active tab.
    pub fn active_tab(&self) -> &Tab {
        let i = self.index_of(self.active).unwrap_or(0);
        &self.tabs[i]
    }

    /// The active tab, mutably.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let i = self.index_of(self.active).unwrap_or(0);
        &mut self.tabs[i]
    }

    fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }
}

impl Window {
    /// Check the structural invariants a restored (possibly hand-edited or corrupt) window must satisfy:
    /// at least one tab, unique tab ids, a valid active tab, and per tab: unique pane ids, a focused pane that
    /// exists, a zoomed pane that exists, split ratios in range. Returns a description of the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.tabs.is_empty() {
            return Err("window has no tabs".into());
        }
        let mut tab_ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).collect();
        tab_ids.sort();
        tab_ids.dedup();
        if tab_ids.len() != self.tabs.len() {
            return Err("duplicate tab ids".into());
        }
        if self.index_of(self.active).is_none() {
            return Err("active tab does not exist".into());
        }
        for t in &self.tabs {
            let ids = t.pane_ids();
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            if sorted.len() != ids.len() {
                return Err(format!("tab {:?} has duplicate pane ids", t.id));
            }
            if !ids.contains(&t.focus) {
                return Err(format!("tab {:?} focuses a missing pane", t.id));
            }
            if t.zoomed.is_some_and(|z| !ids.contains(&z)) {
                return Err(format!("tab {:?} zooms a missing pane", t.id));
            }
            if !ratios_in_range(&t.root) {
                return Err(format!(
                    "tab {:?} has a split ratio outside {MIN_RATIO}..{MAX_RATIO}",
                    t.id
                ));
            }
            if ids.iter().any(|p| p.0 >= t.next_pane) {
                return Err(format!("tab {:?} has a pane id beyond its counter", t.id));
            }
        }
        if self.tabs.iter().any(|t| t.id.0 >= self.next_tab) {
            return Err("a tab id is beyond the window counter".into());
        }
        Ok(())
    }
}

fn ratios_in_range(n: &Node) -> bool {
    match n {
        Node::Leaf(_) => true,
        Node::Split { ratio, a, b, .. } => {
            (MIN_RATIO..=MAX_RATIO).contains(ratio) && ratios_in_range(a) && ratios_in_range(b)
        }
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn t() -> Tab {
        Tab::new(TabId(0), PaneKind::terminal())
    }

    fn ratios_ok(n: &Node) -> bool {
        match n {
            Node::Leaf(_) => true,
            Node::Split { ratio, a, b, .. } => {
                (MIN_RATIO..=MAX_RATIO).contains(ratio) && ratios_ok(a) && ratios_ok(b)
            }
        }
    }

    #[test]
    fn split_adds_pane_and_focuses_it() {
        let mut tab = t();
        let new = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        assert_eq!(tab.pane_ids(), vec![PaneId(0), new]);
        assert_eq!(tab.focus, new);
    }

    #[test]
    fn split_unknown_pane_errors() {
        let mut tab = t();
        assert_eq!(
            tab.split(PaneId(9), Axis::Vertical, PaneKind::terminal()),
            Err(WorkspaceError::NoSuchPane(PaneId(9)))
        );
    }

    #[test]
    fn close_promotes_sibling_and_moves_focus() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.close(b).unwrap();
        assert_eq!(tab.pane_ids(), vec![PaneId(0)]);
        assert_eq!(tab.focus, PaneId(0));
    }

    #[test]
    fn close_last_pane_is_refused() {
        let mut tab = t();
        assert_eq!(tab.close(PaneId(0)), Err(WorkspaceError::LastPane));
    }

    #[test]
    fn layout_halves_space() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let l = tab.layout();
        assert_eq!(
            l[0],
            (
                PaneId(0),
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0
                }
            )
        );
        assert_eq!(
            l[1],
            (
                b,
                Rect {
                    x: 0.5,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0
                }
            )
        );
    }

    #[test]
    fn focus_moves_across_grid() {
        // 0 | 1 on top, 2 under 1
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let c = tab.split(b, Axis::Vertical, PaneKind::terminal()).unwrap();
        tab.focus = c;
        assert_eq!(tab.focus_dir(Direction::Up), Some(b));
        assert_eq!(tab.focus_dir(Direction::Left), Some(PaneId(0)));
        assert_eq!(tab.focus_dir(Direction::Left), None);
        assert_eq!(tab.focus_dir(Direction::Right), Some(b));
    }

    #[test]
    fn resize_clamps() {
        let mut tab = t();
        tab.split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.resize(PaneId(0), Axis::Horizontal, 5.0).unwrap();
        let Node::Split { ratio, .. } = tab.root else {
            panic!("split expected")
        };
        assert_eq!(ratio, MAX_RATIO);
    }

    #[test]
    fn resize_second_child_inverts_delta() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.resize(b, Axis::Horizontal, 0.1).unwrap();
        let Node::Split { ratio, .. } = tab.root else {
            panic!("split expected")
        };
        assert!((ratio - 0.4).abs() < 1e-6);
    }

    #[test]
    fn splits_expose_areas_and_ratio_can_be_set() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        let c = tab.split(b, Axis::Vertical, PaneKind::terminal()).unwrap();
        let sp = tab.splits();
        assert_eq!(sp.len(), 2);
        assert_eq!((sp[0].a_first, sp[0].b_first), (PaneId(0), b));
        assert_eq!(
            sp[1].rect,
            Rect {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 1.0
            }
        );
        assert!(tab.set_split_ratio(b, c, 2.0));
        assert_eq!(tab.splits()[1].ratio, MAX_RATIO);
        assert!(tab.set_split_ratio(PaneId(0), b, 0.3));
        assert_eq!(tab.splits()[0].ratio, 0.3);
        assert!(!tab.set_split_ratio(c, b, 0.5));
        tab.toggle_zoom(b).unwrap();
        assert!(tab.splits().is_empty());
    }

    #[test]
    fn move_border_moves_the_same_way_from_either_side() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.move_border(PaneId(0), Axis::Horizontal, 0.1).unwrap();
        assert!((tab.splits()[0].ratio - 0.6).abs() < 1e-6);
        tab.move_border(b, Axis::Horizontal, 0.1).unwrap();
        assert!((tab.splits()[0].ratio - 0.7).abs() < 1e-6);
        tab.move_border(b, Axis::Horizontal, -0.4).unwrap();
        assert!((tab.splits()[0].ratio - 0.3).abs() < 1e-6);
    }

    #[test]
    fn zoom_toggles_and_fills() {
        let mut tab = t();
        let b = tab
            .split(PaneId(0), Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        tab.toggle_zoom(b).unwrap();
        assert_eq!(tab.layout(), vec![(b, Rect::FULL)]);
        tab.toggle_zoom(b).unwrap();
        assert_eq!(tab.layout().len(), 2);
    }

    #[test]
    fn swap_exchanges_content() {
        let mut tab = t();
        let b = tab
            .split(
                PaneId(0),
                Axis::Horizontal,
                PaneKind::Panel {
                    module: "m".into(),
                    panel: "p".into(),
                },
            )
            .unwrap();
        tab.swap(PaneId(0), b).unwrap();
        assert!(matches!(tab.pane_ids().first(), Some(&id) if id == b));
    }

    #[test]
    fn window_tab_lifecycle() {
        let mut w = Window::new();
        let t1 = w.new_tab(PaneKind::terminal());
        assert_eq!(w.active, t1);
        assert_eq!(w.tabs.len(), 2);
        w.cycle(1);
        assert_eq!(w.active, TabId(0));
        w.cycle(-1);
        assert_eq!(w.active, t1);
        w.close_tab(t1).unwrap();
        assert_eq!(w.active, TabId(0));
        assert_eq!(w.close_tab(TabId(0)), Err(WorkspaceError::LastTab));
    }

    #[test]
    fn validate_accepts_normal_windows_and_rejects_corrupt_ones() {
        let mut w = Window::new();
        w.new_tab(PaneKind::terminal());
        let b = w.active_tab().focus;
        w.active_tab_mut()
            .split(b, Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        assert_eq!(w.validate(), Ok(()));

        let mut bad = w.clone();
        bad.tabs.clear();
        assert!(bad.validate().is_err());
        let mut bad = w.clone();
        bad.active = TabId(99);
        assert!(bad.validate().unwrap_err().contains("active"));
        let mut bad = w.clone();
        bad.active_tab_mut().focus = PaneId(77);
        assert!(bad.validate().unwrap_err().contains("focuses"));
        let mut bad = w.clone();
        bad.active_tab_mut().zoomed = Some(PaneId(77));
        assert!(bad.validate().unwrap_err().contains("zooms"));
        let mut bad = w.clone();
        let tab = bad.active_tab_mut();
        let ids = tab.pane_ids();
        tab.set_split_ratio(ids[0], ids[1], 0.5);
        if let Node::Split { ratio, .. } = &mut tab.root {
            *ratio = 5.0;
        }
        assert!(bad.validate().unwrap_err().contains("ratio"));
        let mut bad = w.clone();
        bad.tabs[1] = bad.tabs[0].clone();
        assert!(bad.validate().unwrap_err().contains("duplicate"));
    }

    #[test]
    fn restored_window_keeps_working_after_a_round_trip() {
        let mut w = Window::new();
        let id = w.active_tab().focus;
        w.active_tab_mut()
            .split(id, Axis::Vertical, PaneKind::terminal())
            .unwrap();
        let text = toml::to_string(&w).unwrap();
        let mut back: Window = toml::from_str(&text).unwrap();
        assert_eq!(back.validate(), Ok(()));
        // The id counters survived: a new pane never reuses an existing id.
        let new = back
            .active_tab_mut()
            .split(id, Axis::Horizontal, PaneKind::terminal())
            .unwrap();
        assert_eq!(
            back.active_tab()
                .pane_ids()
                .iter()
                .filter(|p| **p == new)
                .count(),
            1
        );
        assert_eq!(back.validate(), Ok(()));
    }

    #[test]
    fn move_tab_reorders() {
        let mut w = Window::new();
        let t1 = w.new_tab(PaneKind::terminal());
        w.move_tab(t1, 0).unwrap();
        assert_eq!(w.tabs[0].id, t1);
    }

    #[test]
    fn serde_round_trip_keeps_reserved_layers() {
        let mut w = Window::new();
        let tab = w.active_tab_mut();
        tab.docks.insert(
            DockSide::Left,
            Dock {
                panels: vec![PanelRef {
                    module: "explorer".into(),
                    panel: "tree".into(),
                }],
                size: 0.2,
                visible: true,
            },
        );
        tab.floating.push(FloatingPane {
            pane: Pane {
                id: PaneId(50),
                kind: PaneKind::terminal(),
                title: Some("hud".into()),
            },
            rect: Rect {
                x: 0.2,
                y: 0.2,
                w: 0.5,
                h: 0.5,
            },
        });
        let s = toml::to_string(&w).unwrap();
        let back: Window = toml::from_str(&s).unwrap();
        assert_eq!(w, back);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Split(usize, bool),
        Close(usize),
        Resize(usize, bool, i8),
        Zoom(usize),
        Focus(u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..8usize, any::<bool>()).prop_map(|(i, h)| Op::Split(i, h)),
            (0..8usize).prop_map(Op::Close),
            (0..8usize, any::<bool>(), any::<i8>()).prop_map(|(i, h, d)| Op::Resize(i, h, d)),
            (0..8usize).prop_map(Op::Zoom),
            (0..4u8).prop_map(Op::Focus),
        ]
    }

    proptest! {
        #[test]
        fn tree_invariants_hold(ops in proptest::collection::vec(op(), 0..60)) {
            let mut tab = t();
            for o in ops {
                let ids = tab.pane_ids();
                let pick = |i: usize| ids[i % ids.len()];
                match o {
                    Op::Split(i, h) => {
                        let axis = if h { Axis::Horizontal } else { Axis::Vertical };
                        tab.split(pick(i), axis, PaneKind::terminal()).unwrap();
                    }
                    Op::Close(i) => { let _ = tab.close(pick(i)); }
                    Op::Resize(i, h, d) => {
                        let axis = if h { Axis::Horizontal } else { Axis::Vertical };
                        tab.resize(pick(i), axis, f32::from(d) / 50.0).unwrap();
                    }
                    Op::Zoom(i) => tab.toggle_zoom(pick(i)).unwrap(),
                    Op::Focus(d) => {
                        let dir = [Direction::Left, Direction::Right, Direction::Up, Direction::Down][d as usize];
                        tab.focus_dir(dir);
                    }
                }
                let ids = tab.pane_ids();
                prop_assert!(!ids.is_empty());
                prop_assert!(ids.contains(&tab.focus));
                prop_assert!(ratios_ok(&tab.root));
                let mut sorted = ids.clone();
                sorted.sort();
                sorted.dedup();
                prop_assert_eq!(sorted.len(), ids.len());
                // Unzoomed layout tiles the full area.
                let mut copy = tab.clone();
                copy.zoomed = None;
                let area: f32 = copy.layout().iter().map(|(_, r)| r.w * r.h).sum();
                prop_assert!((area - 1.0).abs() < 1e-3);
            }
        }
    }
}
