//! Transcript discovery + parsing.
//!
//! Claude Code writes one JSONL file per session under
//! `<config-dir>/projects/<munged-cwd>/<session-id>.jsonl`. Assistant
//! entries carry `.message.usage`. Two quirks matter for correctness,
//! both verified against real transcripts:
//!
//! 1. Streamed responses are written more than once with the same
//!    `message.id` + `requestId` — summing without dedup double-counts.
//! 2. Synthetic rows (`model: "<synthetic>"`) carry zeroed usage and no
//!    request id — they are skipped, not priced.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct RawEntry {
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
    /// Fable-class multi-pass turns (server-side tool iterations): the
    /// top-level cache fields aggregate all passes, but top-level
    /// input/output reflect only the FINAL pass — billing is the sum.
    /// Verified on real transcripts: input 4 vs iteration sum 142,478.
    iterations: Option<Vec<RawIteration>>,
}

#[derive(Deserialize, Default)]
struct RawIteration {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize, Default)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// One deduplicated, priced-later usage record.
pub struct Record {
    pub ts: DateTime<Utc>,
    pub model: String,
    pub project: String,
    pub session: String,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub cost_usd: Option<f64>,
}

/// Config dirs to scan: $CLAUDE_CONFIG_DIR (comma-separated) if set,
/// else ~/.claude and ~/.config/claude when they exist.
pub fn config_dirs() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
        let dirs: Vec<PathBuf> = v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    [home.join(".claude"), home.join(".config/claude")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

/// All session transcript files under the dirs' projects/ trees, each
/// paired with its project: the FIRST path component under projects/.
/// Subagent transcripts nest deeper (<project>/<session>/subagents/
/// agent-*.jsonl), so the nearest parent dir would mis-attribute every one
/// of them to a literal "subagents" project.
pub fn discover(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for dir in dirs {
        let root = dir.join("projects");
        let mut files = Vec::new();
        walk(&root, &mut files);
        for f in files {
            let project = f
                .strip_prefix(&root)
                .ok()
                .and_then(|rel| rel.components().next())
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((project, f));
        }
    }
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

pub type DedupKey = (String, String);

/// Parse all files in parallel, then merge sequentially in path-sorted
/// order with cross-file deduplication (a resumed session rewrites the
/// same messages into its continuation file).
pub fn parse_all(files: &[(String, PathBuf)]) -> Vec<Record> {
    use rayon::prelude::*;
    let per_file: Vec<Vec<(Option<DedupKey>, Record)>> =
        files.par_iter().map(|(p, f)| parse_file(f, p)).collect();
    let mut seen: HashMap<DedupKey, usize> = HashMap::new();
    let mut out = Vec::new();
    for (key, record) in per_file.into_iter().flatten() {
        match key {
            Some(key) => match seen.get(&key) {
                Some(&idx) => out[idx] = record, // later write wins
                None => {
                    seen.insert(key, out.len());
                    out.push(record);
                }
            },
            None => out.push(record),
        }
    }
    out
}

/// Parse one transcript file. Deduplication happens in `parse_all`'s
/// merge; this returns every usage row with its dedup key.
///
/// The LAST occurrence of a duplicate key wins, replacing the earlier
/// record in place: duplicate writes are not identical when a turn is
/// still accumulating iterations (the first write carries pass 1, later
/// writes carry passes 1+2+...), so first-seen undercounts input/output.
pub fn parse_file(path: &Path, project: &str) -> Vec<(Option<DedupKey>, Record)> {
    let mut out = Vec::new();
    let Ok(file) = std::fs::File::open(path) else { return out };
    let reader = std::io::BufReader::with_capacity(1 << 20, file);
    let session = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    use std::io::BufRead;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        // Cheap gate before the full JSON parse: only assistant entries
        // carry `"usage"`, and they are a small minority of lines in a
        // 2GB+ transcript corpus.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<RawEntry>(&line) else { continue };
        let Some(message) = entry.message else { continue };
        let Some(usage) = message.usage else { continue };
        let model = message.model.unwrap_or_default();
        if model.is_empty() || model == "<synthetic>" {
            continue;
        }
        let dedup_key = match (&message.id, &entry.request_id) {
            (Some(id), Some(rid)) => Some((id.clone(), rid.clone())),
            _ => None,
        };
        let Some(ts) = entry
            .timestamp
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let cc = usage.cache_creation.unwrap_or_default();
        let (input, output) = match usage.iterations.as_deref() {
            Some(iters) if !iters.is_empty() => (
                iters.iter().map(|i| i.input_tokens).sum(),
                iters.iter().map(|i| i.output_tokens).sum(),
            ),
            _ => (usage.input_tokens, usage.output_tokens),
        };
        let record = Record {
            ts: ts.with_timezone(&Utc),
            model,
            project: project.to_string(),
            session: entry.session_id.unwrap_or_else(|| session.clone()),
            input,
            output,
            cache_write: usage.cache_creation_input_tokens,
            cache_write_5m: cc.ephemeral_5m_input_tokens,
            cache_write_1h: cc.ephemeral_1h_input_tokens,
            cache_read: usage.cache_read_input_tokens,
            cost_usd: entry.cost_usd,
        };
        out.push((dedup_key, record));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"{"type":"assistant","timestamp":"2026-08-23T18:36:01.792Z","requestId":"req_1","sessionId":"sess-a","message":{"id":"msg_1","model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":63193,"cache_read_input_tokens":0,"output_tokens":115,"cache_creation":{"ephemeral_1h_input_tokens":63193,"ephemeral_5m_input_tokens":0}}}}"#;

    fn test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "ccusage-rs-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ))
    }

    /// Writes a transcript at the real layout (projects/proj/sess-a.jsonl)
    /// and runs it through discover + parse_all, so tests exercise the same
    /// path production takes.
    fn parse_str(lines: &str) -> Vec<Record> {
        let dir = test_dir();
        std::fs::create_dir_all(dir.join("projects/proj")).unwrap();
        std::fs::write(dir.join("projects/proj/sess-a.jsonl"), lines).unwrap();
        let out = parse_all(&discover(&[dir.clone()]));
        std::fs::remove_dir_all(&dir).ok();
        out
    }

    #[test]
    fn subagent_transcripts_attribute_to_their_project() {
        let dir = test_dir();
        let deep = dir.join("projects/proj/sess-a/subagents");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("agent-x.jsonl"), format!("{LINE}\n")).unwrap();
        let recs = parse_all(&discover(&[dir.clone()]));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].project, "proj", "not the literal 'subagents' dir");
    }

    #[test]
    fn parses_and_dedups_streamed_duplicates_last_wins() {
        // Second write of the same (msg id, request id) carries a larger
        // accumulated output — the later, more complete write must win.
        let later = LINE.replace("\"output_tokens\":115", "\"output_tokens\":515");
        let recs = parse_str(&format!("{LINE}\n{later}\n"));
        assert_eq!(recs.len(), 1, "same msg id + request id counted once");
        let r = &recs[0];
        assert_eq!(r.input, 2);
        assert_eq!(r.output, 515, "later duplicate write replaces earlier");
        assert_eq!(r.cache_write, 63193);
        assert_eq!(r.cache_write_1h, 63193);
        assert_eq!(r.project, "proj");
    }

    #[test]
    fn skips_synthetic_and_garbage() {
        let synthetic = r#"{"timestamp":"2026-08-23T00:00:00Z","message":{"model":"<synthetic>","usage":{"input_tokens":0}}}"#;
        let recs = parse_str(&format!("{synthetic}\nnot json\n"));
        assert!(recs.is_empty());
    }
}
