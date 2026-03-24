use egui::Rect;
use serde::{Deserialize, Serialize};

pub type PaneId = u64;

/// Divider thickness in logical pixels (split between the two children).
const DIVIDER_PX: f32 = 4.0;

/// Minimum pane dimension in pixels (prevents degenerate splits).
const MIN_PANE_PX: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// A divider between two panes, for hit-testing and dragging.
#[derive(Debug, Clone)]
pub struct Divider {
    pub rect: Rect,
    pub direction: SplitDirection,
    /// Path of child indices from root to the Split node that owns this divider.
    pub node_path: Vec<usize>,
}

/// Binary tree of panes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        /// Fraction of space allocated to `first` (0.0–1.0).
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

impl PaneTree {
    /// Create a tree with a single pane.
    pub fn new(id: PaneId) -> Self {
        PaneTree::Leaf(id)
    }

    /// Compute layout rects for all leaf panes within the given rect.
    pub fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.layout_inner(rect, &mut out);
        out
    }

    fn layout_inner(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            PaneTree::Leaf(id) => out.push((*id, rect)),
            PaneTree::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let half_div = DIVIDER_PX / 2.0;
                match direction {
                    SplitDirection::Horizontal => {
                        // Split left/right
                        let split_x = rect.left() + rect.width() * ratio;
                        let first_rect = Rect::from_min_max(
                            rect.min,
                            egui::pos2(split_x - half_div, rect.max.y),
                        );
                        let second_rect = Rect::from_min_max(
                            egui::pos2(split_x + half_div, rect.min.y),
                            rect.max,
                        );
                        first.layout_inner(first_rect, out);
                        second.layout_inner(second_rect, out);
                    }
                    SplitDirection::Vertical => {
                        // Split top/bottom
                        let split_y = rect.top() + rect.height() * ratio;
                        let first_rect = Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.max.x, split_y - half_div),
                        );
                        let second_rect = Rect::from_min_max(
                            egui::pos2(rect.min.x, split_y + half_div),
                            rect.max,
                        );
                        first.layout_inner(first_rect, out);
                        second.layout_inner(second_rect, out);
                    }
                }
            }
        }
    }

    /// Split a leaf pane into two. The existing pane becomes `first`,
    /// the new pane becomes `second`.
    pub fn split(&mut self, target: PaneId, direction: SplitDirection, new_id: PaneId) -> bool {
        match self {
            PaneTree::Leaf(id) if *id == target => {
                *self = PaneTree::Split {
                    direction,
                    ratio: 0.5,
                    first: Box::new(PaneTree::Leaf(target)),
                    second: Box::new(PaneTree::Leaf(new_id)),
                };
                true
            }
            PaneTree::Leaf(_) => false,
            PaneTree::Split { first, second, .. } => {
                first.split(target, direction, new_id) || second.split(target, direction, new_id)
            }
        }
    }

    /// Close a leaf pane. Returns the sibling's first leaf (new focus candidate).
    /// If this is the last pane, returns None.
    pub fn close(&mut self, target: PaneId) -> Option<PaneId> {
        self.close_inner(target)
    }

    fn close_inner(&mut self, target: PaneId) -> Option<PaneId> {
        match self {
            PaneTree::Leaf(_) => None,
            PaneTree::Split { first, second, .. } => {
                // Check if first child is the target leaf
                if matches!(first.as_ref(), PaneTree::Leaf(id) if *id == target) {
                    let sibling = second.as_ref().clone();
                    let focus = sibling.first_leaf();
                    *self = sibling;
                    return Some(focus);
                }
                // Check if second child is the target leaf
                if matches!(second.as_ref(), PaneTree::Leaf(id) if *id == target) {
                    let sibling = first.as_ref().clone();
                    let focus = sibling.first_leaf();
                    *self = sibling;
                    return Some(focus);
                }
                // Recurse
                first
                    .close_inner(target)
                    .or_else(|| second.close_inner(target))
            }
        }
    }

    /// Get all leaf pane IDs.
    pub fn iter_panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneTree::Leaf(id) => out.push(*id),
            PaneTree::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    /// Whether the tree contains a pane with the given ID.
    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            PaneTree::Leaf(leaf_id) => *leaf_id == id,
            PaneTree::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    /// Get the first leaf ID in the tree (leftmost/topmost).
    pub fn first_leaf(&self) -> PaneId {
        match self {
            PaneTree::Leaf(id) => *id,
            PaneTree::Split { first, .. } => first.first_leaf(),
        }
    }

    /// Compute divider rects for hit-testing. Each divider includes its node path
    /// so `resize_divider` can find the correct Split node.
    pub fn dividers(&self, rect: Rect) -> Vec<Divider> {
        let mut out = Vec::new();
        self.dividers_inner(rect, &mut Vec::new(), &mut out);
        out
    }

    fn dividers_inner(&self, rect: Rect, path: &mut Vec<usize>, out: &mut Vec<Divider>) {
        if let PaneTree::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            let half_div = DIVIDER_PX / 2.0;
            match direction {
                SplitDirection::Horizontal => {
                    let split_x = rect.left() + rect.width() * ratio;
                    let div_rect = Rect::from_min_max(
                        egui::pos2(split_x - half_div, rect.min.y),
                        egui::pos2(split_x + half_div, rect.max.y),
                    );
                    out.push(Divider {
                        rect: div_rect,
                        direction: *direction,
                        node_path: path.clone(),
                    });
                    let first_rect =
                        Rect::from_min_max(rect.min, egui::pos2(split_x - half_div, rect.max.y));
                    let second_rect =
                        Rect::from_min_max(egui::pos2(split_x + half_div, rect.min.y), rect.max);
                    path.push(0);
                    first.dividers_inner(first_rect, path, out);
                    path.pop();
                    path.push(1);
                    second.dividers_inner(second_rect, path, out);
                    path.pop();
                }
                SplitDirection::Vertical => {
                    let split_y = rect.top() + rect.height() * ratio;
                    let div_rect = Rect::from_min_max(
                        egui::pos2(rect.min.x, split_y - half_div),
                        egui::pos2(rect.max.x, split_y + half_div),
                    );
                    out.push(Divider {
                        rect: div_rect,
                        direction: *direction,
                        node_path: path.clone(),
                    });
                    let first_rect =
                        Rect::from_min_max(rect.min, egui::pos2(rect.max.x, split_y - half_div));
                    let second_rect =
                        Rect::from_min_max(egui::pos2(rect.min.x, split_y + half_div), rect.max);
                    path.push(0);
                    first.dividers_inner(first_rect, path, out);
                    path.pop();
                    path.push(1);
                    second.dividers_inner(second_rect, path, out);
                    path.pop();
                }
            }
        }
    }

    /// After a parent resize changed this subtree's allocation from `old_span` to
    /// `new_span` pixels (in `dir`), adjust ratios so the child on `stable_side`
    /// (0 = first, 1 = second) keeps its absolute pixel size. The other child
    /// absorbs the entire size change. Recurses so deeply nested splits also stay put.
    fn stabilize_after_resize(
        &mut self,
        dir: SplitDirection,
        old_span: f32,
        new_span: f32,
        stable_side: usize,
    ) {
        if let PaneTree::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            // Only applies to splits in the same direction as the resize.
            if *direction != dir || old_span <= 0.0 || new_span <= 0.0 {
                return;
            }
            let old_ratio = *ratio;
            let min_r = MIN_PANE_PX / new_span;
            let max_r = 1.0 - min_r;

            match stable_side {
                0 => {
                    // Keep first child at its original pixel size.
                    let old_first = old_span * old_ratio;
                    *ratio = (old_first / new_span).clamp(min_r, max_r);
                    // Second child absorbed the change — recurse keeping *its* first stable.
                    let old_second = old_span * (1.0 - old_ratio);
                    let new_second = new_span * (1.0 - *ratio);
                    second.stabilize_after_resize(dir, old_second, new_second, 0);
                }
                1 => {
                    // Keep second child at its original pixel size.
                    let old_second = old_span * (1.0 - old_ratio);
                    *ratio = (1.0 - old_second / new_span).clamp(min_r, max_r);
                    // First child absorbed the change — recurse keeping *its* second stable.
                    let old_first = old_span * old_ratio;
                    let new_first = new_span * *ratio;
                    first.stabilize_after_resize(dir, old_first, new_first, 1);
                }
                _ => {}
            }
        }
    }

    /// Resize a divider by adjusting the ratio of the Split node at the given path.
    /// `delta` is in pixels (positive = move right/down).
    /// `total_rect` is the full layout rect (needed to convert delta to ratio).
    pub fn resize_divider(&mut self, node_path: &[usize], delta: f32, total_rect: Rect) {
        self.resize_inner(node_path, delta, total_rect);
    }

    fn resize_inner(&mut self, path: &[usize], delta: f32, rect: Rect) {
        if let PaneTree::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            if path.is_empty() {
                // This is the target node — apply the delta
                let span = match direction {
                    SplitDirection::Horizontal => rect.width(),
                    SplitDirection::Vertical => rect.height(),
                };
                if span > 0.0 {
                    let old_ratio = *ratio;
                    let new_ratio = (*ratio + delta / span).clamp(0.0, 1.0);
                    let min_ratio = MIN_PANE_PX / span;
                    let max_ratio = 1.0 - min_ratio;
                    *ratio = new_ratio.clamp(min_ratio, max_ratio);

                    // Stabilize children: only the two panes directly adjacent
                    // to this divider should change size. Non-adjacent panes
                    // keep their absolute pixel size.
                    let old_first_span = span * old_ratio;
                    let new_first_span = span * *ratio;
                    let old_second_span = span * (1.0 - old_ratio);
                    let new_second_span = span * (1.0 - *ratio);

                    // In first child, keep the far side (first/left/top) stable
                    first.stabilize_after_resize(*direction, old_first_span, new_first_span, 0);
                    // In second child, keep the far side (second/right/bottom) stable
                    second.stabilize_after_resize(*direction, old_second_span, new_second_span, 1);
                }
                return;
            }

            // Navigate deeper
            let half_div = DIVIDER_PX / 2.0;
            let (first_rect, second_rect) = match direction {
                SplitDirection::Horizontal => {
                    let split_x = rect.left() + rect.width() * *ratio;
                    (
                        Rect::from_min_max(rect.min, egui::pos2(split_x - half_div, rect.max.y)),
                        Rect::from_min_max(egui::pos2(split_x + half_div, rect.min.y), rect.max),
                    )
                }
                SplitDirection::Vertical => {
                    let split_y = rect.top() + rect.height() * *ratio;
                    (
                        Rect::from_min_max(rect.min, egui::pos2(rect.max.x, split_y - half_div)),
                        Rect::from_min_max(egui::pos2(rect.min.x, split_y + half_div), rect.max),
                    )
                }
            };

            match path[0] {
                0 => first.resize_inner(&path[1..], delta, first_rect),
                1 => second.resize_inner(&path[1..], delta, second_rect),
                _ => {}
            }
        }
    }

    /// Find a neighboring pane in the given direction from `target`.
    /// Uses the layout rects to determine adjacency by center-point proximity.
    pub fn neighbor(&self, target: PaneId, direction: NavDirection, rect: Rect) -> Option<PaneId> {
        let layout = self.layout(rect);
        let target_rect = layout.iter().find(|(id, _)| *id == target)?.1;
        let center = target_rect.center();

        let mut best: Option<(PaneId, f32)> = None;

        for &(id, r) in &layout {
            if id == target {
                continue;
            }
            let other_center = r.center();
            let is_valid = match direction {
                NavDirection::Left => other_center.x < center.x,
                NavDirection::Right => other_center.x > center.x,
                NavDirection::Up => other_center.y < center.y,
                NavDirection::Down => other_center.y > center.y,
            };
            if !is_valid {
                continue;
            }

            // Distance weighted to prefer panes more directly aligned
            let dx = other_center.x - center.x;
            let dy = other_center.y - center.y;
            let dist = match direction {
                NavDirection::Left | NavDirection::Right => dx.abs() + dy.abs() * 2.0,
                NavDirection::Up | NavDirection::Down => dy.abs() + dx.abs() * 2.0,
            };

            if best.is_none() || dist < best.unwrap().1 {
                best = Some((id, dist));
            }
        }

        best.map(|(id, _)| id)
    }
}

/// Navigation direction for focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pane_layout() {
        let tree = PaneTree::new(1);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let layout = tree.layout(rect);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].0, 1);
        assert_eq!(layout[0].1, rect);
    }

    #[test]
    fn split_and_layout() {
        let mut tree = PaneTree::new(1);
        assert!(tree.split(1, SplitDirection::Horizontal, 2));
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let layout = tree.layout(rect);
        assert_eq!(layout.len(), 2);
        // First pane should be on the left, second on the right
        assert!(layout[0].1.max.x < layout[1].1.min.x);
    }

    #[test]
    fn close_pane() {
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        let focus = tree.close(2);
        assert_eq!(focus, Some(1));
        assert!(matches!(tree, PaneTree::Leaf(1)));
    }

    #[test]
    fn iter_panes_and_contains() {
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        tree.split(2, SplitDirection::Vertical, 3);
        let panes = tree.iter_panes();
        assert_eq!(panes, vec![1, 2, 3]);
        assert!(tree.contains(3));
        assert!(!tree.contains(99));
    }

    #[test]
    fn dividers_count() {
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        tree.split(2, SplitDirection::Vertical, 3);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let divs = tree.dividers(rect);
        assert_eq!(divs.len(), 2);
    }

    #[test]
    fn neighbor_navigation() {
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        assert_eq!(tree.neighbor(1, NavDirection::Right, rect), Some(2));
        assert_eq!(tree.neighbor(2, NavDirection::Left, rect), Some(1));
        assert_eq!(tree.neighbor(1, NavDirection::Left, rect), None);
    }

    #[test]
    fn resize_divider_clamps() {
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        // Move divider far to the right
        tree.resize_divider(&[], 10000.0, rect);
        if let PaneTree::Split { ratio, .. } = &tree {
            assert!(*ratio < 1.0);
            assert!(*ratio > 0.9);
        }
    }

    #[test]
    fn resize_parent_keeps_non_adjacent_pane_stable() {
        // Tree: Split(H, 0.5, Pane1, Split(H, 0.5, Pane2, Pane3))
        // Dragging the root divider right should only resize Pane1 and Pane2.
        // Pane3 (non-adjacent) must keep its pixel size.
        let mut tree = PaneTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        tree.split(2, SplitDirection::Horizontal, 3);

        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        // Record Pane3's width before
        let layout_before = tree.layout(rect);
        let pane3_before = layout_before.iter().find(|(id, _)| *id == 3).unwrap().1;

        // Drag root divider 100px to the right
        tree.resize_divider(&[], 100.0, rect);

        let layout_after = tree.layout(rect);
        let pane3_after = layout_after.iter().find(|(id, _)| *id == 3).unwrap().1;

        // Pane3's width should be unchanged (within floating-point tolerance)
        assert!(
            (pane3_after.width() - pane3_before.width()).abs() < 1.0,
            "Pane3 width changed: {} -> {}",
            pane3_before.width(),
            pane3_after.width()
        );
    }
}
