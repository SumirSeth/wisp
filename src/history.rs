//! Launch history, used to rank frequently and recently used apps higher.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Usage {
    count: u32,
    last: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    apps: HashMap<String, Usage>,
    #[serde(skip)]
    path: PathBuf,
}

impl History {
    pub fn load() -> Self {
        let path = util::data_dir().join("history.toml");
        let mut history: History = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        history.path = path;
        history
    }

    pub fn record(&mut self, id: &str) {
        let usage = self.apps.entry(id.to_string()).or_default();
        usage.count += 1;
        usage.last = util::now_secs();
        // Write-then-rename, so a crash mid-write can't wipe the existing history.
        if let Ok(text) = toml::to_string(self) {
            let tmp = self.path.with_extension("toml.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Score bonus for an app: grows with launch count, weighted by recency.
    pub fn boost(&self, id: &str, now: u64) -> u32 {
        let Some(usage) = self.apps.get(id) else { return 0 };
        let recency = match now.saturating_sub(usage.last) {
            age if age < 60 * 60 => 4.0,
            age if age < 24 * 60 * 60 => 2.0,
            age if age < 7 * 24 * 60 * 60 => 1.0,
            _ => 0.5,
        };
        ((1.0 + usage.count as f64).ln() * 12.0 * recency) as u32
    }
}
