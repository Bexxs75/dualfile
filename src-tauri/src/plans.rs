//! Delete plans live on the Rust side. A permanent delete only runs with the token of a
//! stored plan, and every plan is valid exactly once.

use dualfile_core::ops::DeletePlan;
use dualfile_core::vfs::Scheme;
use std::collections::VecDeque;
use std::sync::Mutex;

use crate::convert::{token_text, JobTitleDto};

/// Unconfirmed plans kept at most; the oldest is dropped first.
pub const MAX_PLANS: usize = 32;

/// A plan plus what the confirmed job needs later (its name and the folders to refresh).
pub struct StoredPlan {
    pub plan: DeletePlan,
    pub name: String,
    pub title: JobTitleDto,
    pub dirs: Vec<String>,
    /// The server connection the plan was made on (`None` = local); it only runs there.
    pub place: Option<(Scheme, String)>,
}

#[derive(Default)]
pub struct PlanStore {
    plans: Mutex<VecDeque<(String, StoredPlan)>>,
}

impl PlanStore {
    /// Remembers the plan and returns its token text.
    pub fn insert(
        &self,
        plan: DeletePlan,
        name: String,
        title: JobTitleDto,
        dirs: Vec<String>,
        place: Option<(Scheme, String)>,
    ) -> String {
        let token = token_text(plan.token);
        let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        plans.retain(|(known, _)| *known != token);
        plans.push_back((
            token.clone(),
            StoredPlan {
                plan,
                name,
                title,
                dirs,
                place,
            },
        ));
        while plans.len() > MAX_PLANS {
            plans.pop_front();
        }
        token
    }

    /// Removes and returns the plan; a second call with the same token returns `None`.
    pub fn take(&self, token: &str) -> Option<StoredPlan> {
        let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        let index = plans.iter().position(|(known, _)| known == token)?;
        plans.remove(index).map(|(_, stored)| stored)
    }

    #[cfg(test)]
    pub fn contains(&self, token: &str) -> bool {
        let plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        plans.iter().any(|(known, _)| known == token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::ops::plan_delete;
    use dualfile_core::vfs::{LocalVfs, VPath};

    fn plan_for(dir: &std::path::Path, name: &str) -> DeletePlan {
        std::fs::write(dir.join(name), b"x").unwrap();
        plan_delete(&LocalVfs, &[VPath::local(dir.join(name)).unwrap()])
    }

    #[test]
    fn a_plan_can_be_taken_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::default();
        let token = store.insert(
            plan_for(dir.path(), "a"),
            "n".into(),
            JobTitleDto::new("delete", "n", 0),
            vec![],
            None,
        );
        assert!(store.contains(&token));
        let plan = store.take(&token).expect("first take succeeds");
        assert_eq!(token_text(plan.plan.token), token);
        assert_eq!(plan.name, "n");
        assert!(store.take(&token).is_none());
        assert!(!store.contains(&token));
    }

    #[test]
    fn unknown_tokens_are_refused_and_keep_other_plans() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::default();
        let token = store.insert(
            plan_for(dir.path(), "a"),
            "n".into(),
            JobTitleDto::new("delete", "n", 0),
            vec![],
            None,
        );
        assert!(store.take("deadbeef").is_none());
        assert!(store.take("").is_none());
        assert!(store.contains(&token));
    }

    #[test]
    fn the_oldest_plans_are_dropped_beyond_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::default();
        let first = store.insert(
            plan_for(dir.path(), "first"),
            "n".into(),
            JobTitleDto::new("delete", "n", 0),
            vec![],
            None,
        );
        let mut last = String::new();
        for i in 0..MAX_PLANS {
            last = store.insert(
                plan_for(dir.path(), &format!("f{i}")),
                "n".into(),
                JobTitleDto::new("delete", "n", 0),
                vec![],
                None,
            );
        }
        assert!(!store.contains(&first));
        assert!(store.contains(&last));
    }
}
