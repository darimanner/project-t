//! Pane tree for tiling: each tab is a binary split tree with terminal leaves.
//!
//! `ponytail: binary splits with equal 50/50 sizing — no drag-to-resize, no
//! ratios. Upgrade path: store a ratio per split and hit-test a divider drag.`

use gpui::{App, Entity, Focusable, Window};

use crate::terminal_view::TerminalView;

/// Split orientation: `Row` places children side by side (vertical divider),
/// `Column` stacks them (horizontal divider).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Row,
    Column,
}

pub enum PaneNode {
    Leaf(Entity<TerminalView>),
    Split {
        axis: SplitAxis,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    /// All terminal leaves, depth-first (visual order).
    pub fn leaves(&self) -> Vec<Entity<TerminalView>> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<Entity<TerminalView>>) {
        match self {
            PaneNode::Leaf(view) => out.push(view.clone()),
            PaneNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// The leaf whose terminal currently owns window focus, if any.
    pub fn focused_leaf(&self, window: &Window, cx: &App) -> Option<Entity<TerminalView>> {
        self.leaves()
            .into_iter()
            .find(|leaf| leaf.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Replaces the `target` leaf with a split of (target, new). Returns false
    /// if `target` is not in this tree.
    pub fn split(
        &mut self,
        target: &Entity<TerminalView>,
        axis: SplitAxis,
        new: Entity<TerminalView>,
    ) -> bool {
        match self {
            PaneNode::Leaf(view) => {
                if view == target {
                    let old = PaneNode::Leaf(view.clone());
                    *self = PaneNode::Split {
                        axis,
                        first: Box::new(old),
                        second: Box::new(PaneNode::Leaf(new)),
                    };
                    true
                } else {
                    false
                }
            }
            PaneNode::Split { first, second, .. } => {
                first.split(target, axis, new.clone()) || second.split(target, axis, new)
            }
        }
    }

    /// Removes the `target` leaf, collapsing its parent split into the sibling.
    /// Returns false when `target` is the root leaf (caller closes the tab) or
    /// is not present.
    pub fn remove(&mut self, target: &Entity<TerminalView>) -> bool {
        let PaneNode::Split { first, second, .. } = self else {
            return false;
        };
        // The dummy leaf is immediately overwritten by `*self = *sibling`.
        let dummy = || Box::new(PaneNode::Leaf(target.clone()));
        if matches!(&**first, PaneNode::Leaf(v) if v == target) {
            let sibling = std::mem::replace(second, dummy());
            *self = *sibling;
            true
        } else if matches!(&**second, PaneNode::Leaf(v) if v == target) {
            let sibling = std::mem::replace(first, dummy());
            *self = *sibling;
            true
        } else {
            first.remove(target) || second.remove(target)
        }
    }
}
