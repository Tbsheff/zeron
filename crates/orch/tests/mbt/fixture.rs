use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use zeron_orch::{Config, ThreadId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    RunLifecycle,
    Delegation,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub params: Vec<Value>,
}

pub struct Fixture {
    pub name: String,
    pub module: Module,
    pub constants: Map<String, Value>,
    pub threads: Vec<(String, ThreadId)>,
    pub init: usize,
    pub vars: Vec<String>,
    pub values: Map<String, Value>,
    pub states: Vec<Vec<usize>>,
    pub actions: Vec<Action>,
    pub edges: Vec<[usize; 3]>,
}

pub fn fixture_paths() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("mbt")
        .join("fixtures");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

fn usize_of(v: &Value) -> usize {
    usize::try_from(v.as_u64().expect("index")).expect("usize")
}

impl Fixture {
    pub fn load(path: &Path) -> Fixture {
        let text = std::fs::read_to_string(path).expect("read fixture");
        let j: Value = serde_json::from_str(&text).expect("parse fixture");
        let module = match j["module"].as_str() {
            Some("RunLifecycle") => Module::RunLifecycle,
            _ => Module::Delegation,
        };
        let constants = j["constants"].as_object().cloned().unwrap_or_default();
        let threads = thread_names(&constants);
        Fixture {
            name: j["name"].as_str().unwrap_or_default().to_string(),
            module,
            constants,
            threads,
            init: usize_of(&j["init"]),
            vars: j["vars"]
                .as_array()
                .expect("vars")
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            values: j["values"].as_object().cloned().unwrap_or_default(),
            states: rows(&j["states"]),
            actions: j["actions"]
                .as_array()
                .expect("actions")
                .iter()
                .map(action)
                .collect(),
            edges: rows(&j["edges"])
                .into_iter()
                .map(|e| [e[0], e[1], e[2]])
                .collect(),
        }
    }

    pub fn constant(&self, k: &str) -> i64 {
        let v = &self.constants[k];
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0)
    }

    pub fn flag(&self, k: &str) -> bool {
        self.constants[k].as_str() == Some("TRUE")
    }

    pub fn max_runs(&self) -> u32 {
        u32::try_from(self.constant("MaxRuns")).unwrap_or(0)
    }

    pub fn config(&self) -> Config {
        let n = |k: &str| usize::try_from(self.constant(k)).unwrap_or(0);
        match self.module {
            Module::RunLifecycle => Config {
                max_depth: 2,
                max_fanout: 8,
                max_queue: n("MaxQueue"),
                max_retry: u32::try_from(self.constant("MaxRetry")).unwrap_or(0),
                run_window: n("MaxRuns"),
            },
            Module::Delegation => Config {
                max_depth: u8::try_from(self.constant("MaxDepth")).unwrap_or(0),
                max_fanout: n("MaxFanout"),
                max_queue: n("MaxQueue"),
                max_retry: 0,
                run_window: n("MaxRuns"),
            },
        }
    }

    pub fn id_of(&self, name: &str) -> ThreadId {
        self.threads
            .iter()
            .find(|(n, _)| n == name)
            .map_or(ThreadId(u32::MAX), |(_, id)| *id)
    }

    pub fn name_of(&self, id: Option<ThreadId>) -> String {
        id.and_then(|id| self.threads.iter().find(|(_, x)| *x == id))
            .map_or("NoThread".to_string(), |(n, _)| n.clone())
    }

    pub fn spec_state(&self, node: usize) -> Map<String, Value> {
        let mut m = Map::new();
        for (i, var) in self.vars.iter().enumerate() {
            let v = &self.values[var.as_str()][self.states[node][i]];
            m.insert(var.clone(), super::abstraction::canon(v.clone()));
        }
        m
    }
}

fn thread_names(constants: &Map<String, Value>) -> Vec<(String, ThreadId)> {
    let root = constants["Root"].as_str().unwrap_or_default().to_string();
    let list = constants["Threads"].as_str().unwrap_or_default();
    let mut names: Vec<String> = list
        .trim_matches(|c| c == '{' || c == '}')
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    names.retain(|n| *n != root);
    names.insert(0, root);
    names
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, ThreadId(u32::try_from(i).unwrap_or(0))))
        .collect()
}

fn rows(v: &Value) -> Vec<Vec<usize>> {
    v.as_array()
        .expect("rows")
        .iter()
        .map(|r| r.as_array().expect("row").iter().map(usize_of).collect())
        .collect()
}

fn action(v: &Value) -> Action {
    Action {
        name: v["name"].as_str().unwrap_or_default().to_string(),
        params: v["params"].as_array().cloned().unwrap_or_default(),
    }
}
