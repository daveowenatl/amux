//! Browser URL history — persists visited URLs for omnibar autocomplete.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A visited URL entry with visit count and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: u32,
    pub last_visited_ms: u64,
}

/// In-memory history store backed by a JSON file.
pub struct BrowserHistory {
    entries: Vec<HistoryEntry>,
    path: PathBuf,
    dirty: bool,
}

fn history_path() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/amux")
    } else {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("amux")
    };
    base.join("browser-history.json")
}

impl BrowserHistory {
    /// Load history from disk, or create empty if not found.
    pub fn load() -> Self {
        let path = history_path();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            entries,
            path,
            dirty: false,
        }
    }

    /// Record a URL visit. Updates existing entry or creates new one.
    pub fn record_visit(&mut self, url: &str, title: &str) {
        // Skip internal/blank URLs
        if url.is_empty() || url == "about:blank" || url.starts_with("data:") {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        if let Some(entry) = self.entries.iter_mut().find(|e| e.url == url) {
            entry.visit_count += 1;
            entry.last_visited_ms = now_ms;
            if !title.is_empty() {
                entry.title = title.to_string();
            }
        } else {
            self.entries.push(HistoryEntry {
                url: url.to_string(),
                title: title.to_string(),
                visit_count: 1,
                last_visited_ms: now_ms,
            });
        }

        // Cap at 5000 entries — remove oldest by last_visited_ms.
        // sort_by_key + Reverse is clippy-preferred over manual b.cmp(a).
        if self.entries.len() > 5000 {
            self.entries
                .sort_by_key(|e| std::cmp::Reverse(e.last_visited_ms));
            self.entries.truncate(5000);
        }

        self.dirty = true;
    }

    /// Search history for entries matching the query. Returns up to `limit`
    /// results sorted by relevance (visit count * recency).
    pub fn search(&self, query: &str, limit: usize) -> Vec<&HistoryEntry> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = query.to_lowercase();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut matches: Vec<(f64, &HistoryEntry)> = self
            .entries
            .iter()
            .filter(|e| {
                e.url.to_lowercase().contains(&lower) || e.title.to_lowercase().contains(&lower)
            })
            .map(|e| {
                // Score: visit_count * recency_weight
                let age_days = now_ms.saturating_sub(e.last_visited_ms) as f64 / 86_400_000.0;
                let recency = 1.0 / (1.0 + age_days * 0.1);
                let score = e.visit_count as f64 * recency;
                (score, e)
            })
            .collect();

        matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        matches.into_iter().take(limit).map(|(_, e)| e).collect()
    }

    /// Flush changes to disk if dirty.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Failed to create directory {}: {e}", parent.display());
            }
        }
        match serde_json::to_string(&self.entries) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    tracing::warn!("Failed to save browser history: {}", e);
                } else {
                    self.dirty = false;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize browser history: {}", e);
            }
        }
    }
}
