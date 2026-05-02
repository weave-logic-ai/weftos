//! Goal alignment types for the Paperclip Patterns integration.
//!
//! Models hierarchical goals with parent-child relationships, status
//! tracking, and key metrics. Used alongside the resource tree and
//! org-chart to align agent behaviour with business objectives.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Status of a goal in its lifecycle.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Goal has been defined but not yet started.
    #[default]
    Pending,
    /// Goal is actively being pursued.
    Active,
    /// Goal has been achieved.
    Complete,
    /// Goal was abandoned or could not be met.
    Failed,
}

impl std::fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalStatus::Pending => write!(f, "pending"),
            GoalStatus::Active => write!(f, "active"),
            GoalStatus::Complete => write!(f, "complete"),
            GoalStatus::Failed => write!(f, "failed"),
        }
    }
}

/// A single goal in a goal hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    /// Unique goal identifier.
    pub id: String,
    /// Human-readable description of the goal.
    pub description: String,
    /// Parent goal ID (`None` for top-level goals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_goal: Option<String>,
    /// Current status.
    #[serde(default)]
    pub status: GoalStatus,
    /// Key-value metrics for tracking progress (e.g. "completion_pct" -> "75").
    #[serde(default)]
    pub metrics: HashMap<String, String>,
}

impl Goal {
    /// Create a new top-level goal in pending status.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            parent_goal: None,
            status: GoalStatus::Pending,
            metrics: HashMap::new(),
        }
    }

    /// Create a sub-goal under the given parent.
    pub fn child(
        id: impl Into<String>,
        description: impl Into<String>,
        parent_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            parent_goal: Some(parent_id.into()),
            status: GoalStatus::Pending,
            metrics: HashMap::new(),
        }
    }
}

/// A tree of goals with parent-child traversal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalTree {
    /// All goals in the tree (flat storage, linked by `parent_goal`).
    pub goals: Vec<Goal>,
}

impl GoalTree {
    /// Create an empty goal tree.
    pub fn new() -> Self {
        Self { goals: Vec::new() }
    }

    /// Add a goal to the tree.
    pub fn add(&mut self, goal: Goal) {
        self.goals.push(goal);
    }

    /// Find a goal by ID.
    pub fn find(&self, id: &str) -> Option<&Goal> {
        self.goals.iter().find(|g| g.id == id)
    }

    /// Find a goal by ID (mutable).
    pub fn find_mut(&mut self, id: &str) -> Option<&mut Goal> {
        self.goals.iter_mut().find(|g| g.id == id)
    }

    /// Return all root goals (those with no parent).
    pub fn roots(&self) -> Vec<&Goal> {
        self.goals.iter().filter(|g| g.parent_goal.is_none()).collect()
    }

    /// Return all direct children of the given goal.
    pub fn children(&self, parent_id: &str) -> Vec<&Goal> {
        self.goals
            .iter()
            .filter(|g| g.parent_goal.as_deref() == Some(parent_id))
            .collect()
    }

    /// Return all descendants of the given goal (depth-first).
    pub fn descendants(&self, parent_id: &str) -> Vec<&Goal> {
        let mut result = Vec::new();
        let mut stack: Vec<&str> = vec![parent_id];
        while let Some(current) = stack.pop() {
            for child in self.children(current) {
                result.push(child);
                stack.push(&child.id);
            }
        }
        result
    }

    /// Return goals filtered by status.
    pub fn by_status(&self, status: GoalStatus) -> Vec<&Goal> {
        self.goals.iter().filter(|g| g.status == status).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_status_default_is_pending() {
        assert_eq!(GoalStatus::default(), GoalStatus::Pending);
    }

    #[test]
    fn goal_status_serde() {
        let statuses = [
            (GoalStatus::Pending, "\"pending\""),
            (GoalStatus::Active, "\"active\""),
            (GoalStatus::Complete, "\"complete\""),
            (GoalStatus::Failed, "\"failed\""),
        ];
        for (status, expected) in &statuses {
            let json = serde_json::to_string(status).unwrap();
            assert_eq!(&json, expected);
            let restored: GoalStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, status);
        }
    }

    #[test]
    fn goal_status_display() {
        assert_eq!(GoalStatus::Active.to_string(), "active");
        assert_eq!(GoalStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn goal_new_top_level() {
        let g = Goal::new("g1", "Increase revenue");
        assert_eq!(g.id, "g1");
        assert!(g.parent_goal.is_none());
        assert_eq!(g.status, GoalStatus::Pending);
        assert!(g.metrics.is_empty());
    }

    #[test]
    fn goal_child() {
        let g = Goal::child("g2", "Hire sales team", "g1");
        assert_eq!(g.parent_goal.as_deref(), Some("g1"));
    }

    #[test]
    fn goal_serde_roundtrip() {
        let mut g = Goal::new("g1", "Ship v2");
        g.status = GoalStatus::Active;
        g.metrics.insert("pct".into(), "50".into());
        let json = serde_json::to_string(&g).unwrap();
        let restored: Goal = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "g1");
        assert_eq!(restored.status, GoalStatus::Active);
        assert_eq!(restored.metrics.get("pct").unwrap(), "50");
    }

    #[test]
    fn goal_omits_none_parent() {
        let g = Goal::new("g1", "top");
        let json = serde_json::to_string(&g).unwrap();
        assert!(!json.contains("parent_goal"));
    }

    #[test]
    fn goal_tree_empty() {
        let tree = GoalTree::new();
        assert!(tree.goals.is_empty());
        assert!(tree.roots().is_empty());
    }

    #[test]
    fn goal_tree_add_and_find() {
        let mut tree = GoalTree::new();
        tree.add(Goal::new("g1", "Revenue"));
        assert!(tree.find("g1").is_some());
        assert!(tree.find("nonexistent").is_none());
    }

    #[test]
    fn goal_tree_find_mut() {
        let mut tree = GoalTree::new();
        tree.add(Goal::new("g1", "Revenue"));
        tree.find_mut("g1").unwrap().status = GoalStatus::Complete;
        assert_eq!(tree.find("g1").unwrap().status, GoalStatus::Complete);
    }

    #[test]
    fn goal_tree_roots_and_children() {
        let mut tree = GoalTree::new();
        tree.add(Goal::new("root1", "Strategic goal A"));
        tree.add(Goal::new("root2", "Strategic goal B"));
        tree.add(Goal::child("sub1", "Sub-goal 1", "root1"));
        tree.add(Goal::child("sub2", "Sub-goal 2", "root1"));
        tree.add(Goal::child("sub3", "Sub-goal 3", "root2"));

        let roots = tree.roots();
        assert_eq!(roots.len(), 2);

        let children = tree.children("root1");
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|g| g.id == "sub1"));
        assert!(children.iter().any(|g| g.id == "sub2"));

        let r2_children = tree.children("root2");
        assert_eq!(r2_children.len(), 1);
    }

    #[test]
    fn goal_tree_descendants() {
        let mut tree = GoalTree::new();
        tree.add(Goal::new("r", "Root"));
        tree.add(Goal::child("a", "Child A", "r"));
        tree.add(Goal::child("b", "Child B", "r"));
        tree.add(Goal::child("a1", "Grandchild A1", "a"));
        tree.add(Goal::child("a2", "Grandchild A2", "a"));

        let desc = tree.descendants("r");
        assert_eq!(desc.len(), 4);

        let a_desc = tree.descendants("a");
        assert_eq!(a_desc.len(), 2);

        let leaf_desc = tree.descendants("a1");
        assert!(leaf_desc.is_empty());
    }

    #[test]
    fn goal_tree_by_status() {
        let mut tree = GoalTree::new();
        let mut g1 = Goal::new("g1", "Active goal");
        g1.status = GoalStatus::Active;
        let mut g2 = Goal::new("g2", "Complete goal");
        g2.status = GoalStatus::Complete;
        tree.add(g1);
        tree.add(g2);
        tree.add(Goal::new("g3", "Pending goal"));

        assert_eq!(tree.by_status(GoalStatus::Active).len(), 1);
        assert_eq!(tree.by_status(GoalStatus::Pending).len(), 1);
        assert_eq!(tree.by_status(GoalStatus::Complete).len(), 1);
        assert_eq!(tree.by_status(GoalStatus::Failed).len(), 0);
    }

    #[test]
    fn goal_tree_serde_roundtrip() {
        let mut tree = GoalTree::new();
        tree.add(Goal::new("g1", "Top"));
        tree.add(Goal::child("g2", "Sub", "g1"));
        let json = serde_json::to_string(&tree).unwrap();
        let restored: GoalTree = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.goals.len(), 2);
        assert_eq!(restored.find("g2").unwrap().parent_goal.as_deref(), Some("g1"));
    }
}
