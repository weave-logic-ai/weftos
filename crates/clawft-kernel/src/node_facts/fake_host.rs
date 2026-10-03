//! A scripted [`ProbeHost`] for probe tests: canned tool output, files,
//! directories and paths, so any machine can be simulated.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::host::ProbeHost;

#[derive(Debug, Default, Clone)]
pub struct FakeHost {
    pub os: String,
    pub arch: String,
    pub tools: BTreeSet<String>,
    /// Key: `"tool arg1 arg2"`.
    pub outputs: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
    pub dirs: BTreeMap<String, Vec<String>>,
    pub paths: BTreeSet<String>,
    /// Every `run` command, in order (shared across clones).
    pub log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeHost {
    pub fn new(os: &str, arch: &str) -> Self {
        Self {
            os: os.into(),
            arch: arch.into(),
            ..Self::default()
        }
    }
    pub fn tool(mut self, t: &str) -> Self {
        self.tools.insert(t.into());
        self
    }
    /// Register `cmd` (`"tool arg ..."`) with its stdout; also installs the tool.
    pub fn out(mut self, cmd: &str, stdout: &str) -> Self {
        let tool = cmd.split(' ').next().unwrap().to_string();
        self.tools.insert(tool);
        self.outputs.insert(cmd.into(), stdout.into());
        self
    }
    pub fn file(mut self, path: &str, body: &str) -> Self {
        self.files.insert(path.into(), body.into());
        self.paths.insert(path.into());
        self
    }
    pub fn dir(mut self, path: &str, names: &[&str]) -> Self {
        self.dirs
            .insert(path.into(), names.iter().map(|s| s.to_string()).collect());
        self
    }
    pub fn path(mut self, p: &str) -> Self {
        self.paths.insert(p.into());
        self
    }
}

impl ProbeHost for FakeHost {
    fn os(&self) -> String {
        self.os.clone()
    }
    fn arch(&self) -> String {
        self.arch.clone()
    }
    fn which(&self, tool: &str) -> Option<PathBuf> {
        self.tools
            .contains(tool)
            .then(|| PathBuf::from(format!("/fake/bin/{tool}")))
    }
    fn run(&self, tool: &str, args: &[&str]) -> Option<String> {
        if !self.tools.contains(tool) {
            return None;
        }
        let key = std::iter::once(tool)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.log.lock().unwrap().push(key.clone());
        self.outputs.get(&key).cloned()
    }
    fn run_all(&self, tool: &str, args: &[&str]) -> Option<String> {
        self.run(tool, args)
    }
    fn read_file(&self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }
    fn list_dir(&self, path: &str) -> Option<Vec<String>> {
        self.dirs.get(path).cloned()
    }
    fn exists(&self, path: &str) -> bool {
        self.paths.contains(path) || self.dirs.contains_key(path)
    }
}
