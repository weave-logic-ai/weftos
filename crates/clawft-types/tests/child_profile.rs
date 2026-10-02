//! `RuntimePaths::resolve()` honours the process-wide child profile. Its own
//! test binary because the profile is process state.

use clawft_types::runtime_paths::{RootSource, RuntimePaths, child_profile, set_child_profile};

const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

#[test]
fn resolve_returns_the_child_root_once_set() {
    assert!(!set_child_profile(Some(RuntimePaths::at("/not/a/child"))));
    assert!(child_profile().is_none());
    let c = RuntimePaths::child_with(std::path::Path::new("/h"), ID, "/work/p").unwrap();
    assert!(set_child_profile(Some(c.clone())));
    assert_eq!(RuntimePaths::resolve(), c);
    assert_eq!(RuntimePaths::resolve().project_key(), c.project_key());
    assert!(set_child_profile(None));
    assert!(!matches!(
        RuntimePaths::resolve().source(),
        RootSource::Child { .. }
    ));
}
