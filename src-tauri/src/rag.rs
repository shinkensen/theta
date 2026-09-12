//! Local retrieval store — the RAG half of the assistant.
//!
//! Deliberately embedding-free: nothing to download, no network call on the
//! retrieval path. Ranking fuses two cheap signals that fail differently:
//!
//! * **BM25** over stemmed word tokens — strong on exact terminology.
//! * **Character-trigram cosine** — survives the mangled words a small Vosk
//!   model produces ("kubernetes" → "cooper netties").
//!
//! The two rankings are combined with reciprocal rank fusion and then nudged
//! by recency. Only documents are persisted; the index is rebuilt at load,
//! which keeps the file small and lets the tokenizer change without a
//! migration step.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Target characters per chunk, and the overlap that keeps a fact spanning a
/// boundary retrievable from either side.
const CHUNK_CHARS: usize = 900;
const CHUNK_OVERLAP: usize = 150;

/// Standard BM25 tuning; not tuned per-corpus.
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// RRF damping constant from Cormack et al.
const RRF_K: f32 = 60.0;

/// Candidates each ranker contributes before fusion.
const CANDIDATES: usize = 40;

/// Refuse to ingest anything larger than this, to keep the index in memory.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "do", "for", "from", "had", "has",
    "have", "i", "if", "in", "into", "is", "it", "its", "me", "my", "no", "not", "of", "on", "or",
    "so", "than", "that", "the", "their", "them", "then", "there", "these", "they", "this", "to",
    "was", "were", "what", "when", "which", "who", "will", "with", "you", "your",
];

/// Strips one suffix, longest-match first. `None` when nothing applies.
fn strip_once(w: &str) -> Option<&str> {
    for suffix in ["edly", "ing", "ly", "es", "ed", "s"] {
        // Keep at least three characters so short words aren't gutted.
        if w.len() <= suffix.len() + 2 || !w.ends_with(suffix) {
            continue;
        }
        let base = &w[..w.len() - suffix.len()];
        // "address" and "press" must keep their double s. Note this only
        // blocks the bare `s`: for `es` a trailing s is the "classes" →"class"
        // case, which is exactly what we want.
        if suffix == "s" && base.ends_with('s') {
            continue;
        }
        return Some(base);
    }
    None
}

/// Very light suffix stripping — enough to make "meetings"/"meeting" and
/// "running"/"run" collide without pulling in a full Porter stemmer.
///
/// Two passes, because one is not enough: "meetings" only loses its plural on
/// the first pass, while "meeting" goes straight to "meet", so a single-pass
/// stemmer makes the two spellings *fail* to collide.
fn stem(word: &str) -> String {
    let mut w = word;
    for _ in 0..2 {
        match strip_once(w) {
            Some(shorter) => w = shorter,
            None => break,
        }
    }

    // Porter's undoubling rule: "running" → "runn" → "run". Vowels plus l/s/z
    // are excluded because "see", "fall", "press" and "buzz" are real words.
    // Both bytes being ASCII means len-1 is a valid char boundary.
    let bytes = w.as_bytes();
    if bytes.len() > 3 {
        let (prev, last) = (bytes[bytes.len() - 2], bytes[bytes.len() - 1]);
        let undoubles = prev == last
            && last.is_ascii_alphabetic()
            && !matches!(last, b'a' | b'e' | b'i' | b'o' | b'u' | b'l' | b's' | b'z');
        if undoubles {
            return w[..w.len() - 1].to_string();
        }
    }
    w.to_string()
}

/// Lowercase, split on non-alphanumerics, drop stopwords, stem.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !STOPWORDS.contains(t))
        .map(stem)
        .collect()
}

/// Character trigrams of the whitespace-collapsed lowercase text.
fn trigrams(text: &str) -> HashMap<String, f32> {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let chars: Vec<char> = cleaned.chars().collect();
    let mut counts: HashMap<String, f32> = HashMap::new();
    if chars.len() < 3 {
        if !cleaned.is_empty() {
            counts.insert(cleaned, 1.0);
        }
        return counts;
    }
    for window in chars.windows(3) {
        *counts.entry(window.iter().collect()).or_insert(0.0) += 1.0;
    }
    // L2-normalize so cosine is a plain dot product later.
    let norm = counts.values().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in counts.values_mut() {
            *v /= norm;
        }
    }
    counts
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Doc {
    pub id: String,
    /// Where this came from: a file path, `"note"`, `"conversation"`, …
    pub source: String,
    /// Human-facing label shown in the memory panel.
    pub title: String,
    pub text: String,
    /// Unix seconds.
    pub created: i64,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Per-document derived state. Rebuilt on load, never serialized.
struct Indexed {
    term_freqs: HashMap<String, f32>,
    len: f32,
    trigrams: HashMap<String, f32>,
}

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    docs: Vec<Doc>,
}

pub struct RagState {
    inner: Mutex<RagIndex>,
    path: PathBuf,
}

#[derive(Default)]
struct RagIndex {
    docs: Vec<Doc>,
    indexed: Vec<Indexed>,
    /// term → document positions, for BM25 scoring without a full scan.
    postings: HashMap<String, Vec<usize>>,
    avg_len: f32,
}

impl RagIndex {
    fn rebuild(&mut self) {
        self.indexed.clear();
        self.postings.clear();
        let mut total_len = 0.0;

        for (i, doc) in self.docs.iter().enumerate() {
            let tokens = tokenize(&doc.text);
            let mut term_freqs: HashMap<String, f32> = HashMap::new();
            for t in &tokens {
                *term_freqs.entry(t.clone()).or_insert(0.0) += 1.0;
            }
            for term in term_freqs.keys() {
                self.postings.entry(term.clone()).or_default().push(i);
            }
            total_len += tokens.len() as f32;
            self.indexed.push(Indexed {
                term_freqs,
                len: tokens.len() as f32,
                // Index the title too — it's often where the useful keyword is.
                trigrams: trigrams(&format!("{} {}", doc.title, doc.text)),
            });
        }

        self.avg_len = if self.docs.is_empty() {
            0.0
        } else {
            total_len / self.docs.len() as f32
        };
    }

    fn bm25(&self, query_terms: &[String]) -> Vec<(usize, f32)> {
        let n = self.docs.len() as f32;
        let mut scores: HashMap<usize, f32> = HashMap::new();

        for term in query_terms {
            let Some(posting) = self.postings.get(term) else {
                continue;
            };
            let df = posting.len() as f32;
            // BM25 IDF, floored at 0 so terms in most documents can't
            // subtract from a score.
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln().max(0.0);
            for &doc_i in posting {
                let entry = &self.indexed[doc_i];
                let tf = entry.term_freqs.get(term).copied().unwrap_or(0.0);
                let norm = if self.avg_len > 0.0 {
                    1.0 - BM25_B + BM25_B * (entry.len / self.avg_len)
                } else {
                    1.0
                };
                *scores.entry(doc_i).or_insert(0.0) +=
                    idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * norm);
            }
        }

        let mut ranked: Vec<(usize, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(CANDIDATES);
        ranked
    }

    fn trigram_cosine(&self, query: &str) -> Vec<(usize, f32)> {
        let q = trigrams(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut ranked: Vec<(usize, f32)> = self
            .indexed
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                // Both sides are L2-normalized, so the dot product is cosine.
                // Iterate the shorter map.
                let (small, large) = if q.len() < entry.trigrams.len() {
                    (&q, &entry.trigrams)
                } else {
                    (&entry.trigrams, &q)
                };
                let score = small
                    .iter()
                    .filter_map(|(gram, w)| large.get(gram).map(|w2| w * w2))
                    .sum::<f32>();
                (i, score)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(CANDIDATES);
        ranked
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_id() -> String {
    use rand::Rng;
    let n: u64 = rand::rng().random();
    format!("{:x}{:x}", now_secs(), n & 0xffff_ffff)
}

impl RagState {
    /// Loads the store from `<app-data>/rag.json`, or starts empty.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("rag.json");
        let docs = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Persisted>(&raw).ok())
            .map(|p| p.docs)
            .unwrap_or_default();

        let mut index = RagIndex {
            docs,
            ..Default::default()
        };
        index.rebuild();
        eprintln!(
            "[theta] RAG store: {} chunks from {}",
            index.docs.len(),
            path.display()
        );
        Self {
            inner: Mutex::new(index),
            path,
        }
    }

    fn save(&self, index: &RagIndex) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let payload = Persisted {
            docs: index.docs.clone(),
        };
        match serde_json::to_string(&payload) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    eprintln!("[theta] failed to persist RAG store: {e}");
                }
            }
            Err(e) => eprintln!("[theta] failed to serialize RAG store: {e}"),
        }
    }
}

/// Splits text into overlapping chunks, preferring paragraph and then
/// sentence boundaries so a chunk rarely starts mid-thought.
fn chunk_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= CHUNK_CHARS {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0usize;

    while start < chars.len() {
        let hard_end = (start + CHUNK_CHARS).min(chars.len());
        let mut end = hard_end;

        if hard_end < chars.len() {
            // Look back over the last third for a clean break.
            let window_start = start + (CHUNK_CHARS * 2 / 3);
            let slice = &chars[window_start..hard_end];
            let break_at = slice.iter().rposition(|&c| c == '\n').or_else(|| {
                slice
                    .iter()
                    .rposition(|&c| c == '.' || c == '!' || c == '?')
            });
            if let Some(offset) = break_at {
                end = window_start + offset + 1;
            }
        }

        let chunk: String = chars[start..end].iter().collect();
        let chunk = chunk.trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
    }

    chunks
}

#[derive(Serialize)]
pub struct IngestResult {
    pub chunks: usize,
    pub total_docs: usize,
    pub title: String,
}

/// Chunks and indexes raw text. `title`/`source`/`tags` are metadata only.
#[tauri::command]
pub fn rag_ingest_text(
    text: String,
    title: Option<String>,
    source: Option<String>,
    tags: Option<Vec<String>>,
    state: tauri::State<'_, RagState>,
) -> Result<IngestResult, String> {
    let chunks = chunk_text(&text);
    if chunks.is_empty() {
        return Err("Nothing to ingest — the text was empty.".into());
    }

    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            // Fall back to the first line, clipped.
            let first = text
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("note");
            first.chars().take(60).collect()
        });
    let source = source.unwrap_or_else(|| "note".to_string());
    let tags = tags.unwrap_or_default();
    let created = now_secs();

    let mut index = state.inner.lock().map_err(|e| e.to_string())?;
    let multi = chunks.len() > 1;
    for (i, chunk) in chunks.iter().enumerate() {
        index.docs.push(Doc {
            id: new_id(),
            source: source.clone(),
            title: if multi {
                format!("{title} ({}/{})", i + 1, chunks.len())
            } else {
                title.clone()
            },
            text: chunk.clone(),
            created,
            tags: tags.clone(),
        });
    }
    index.rebuild();
    state.save(&index);

    Ok(IngestResult {
        chunks: chunks.len(),
        total_docs: index.docs.len(),
        title,
    })
}

/// Reads a UTF-8 text file off disk and ingests it, tagged with its path.
#[tauri::command]
pub fn rag_ingest_file(
    path: String,
    state: tauri::State<'_, RagState>,
) -> Result<IngestResult, String> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| format!("Can't read '{path}': {e}"))?;
    if !meta.is_file() {
        return Err(format!("'{path}' is not a file."));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "'{path}' is {:.1} MB — over the {} MB ingest limit.",
            meta.len() as f64 / 1_048_576.0,
            MAX_FILE_BYTES / 1_048_576
        ));
    }
    let text = std::fs::read_to_string(p)
        .map_err(|e| format!("Can't read '{path}' as UTF-8 text: {e}"))?;
    let title = p
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    // Re-ingesting a file replaces its previous chunks rather than duplicating.
    {
        let mut index = state.inner.lock().map_err(|e| e.to_string())?;
        let before = index.docs.len();
        index.docs.retain(|d| d.source != path);
        if index.docs.len() != before {
            index.rebuild();
        }
    }

    rag_ingest_text(
        text,
        Some(title),
        Some(path),
        Some(vec!["file".into()]),
        state,
    )
}

#[derive(Serialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub source: String,
    pub text: String,
    pub score: f32,
    pub created: i64,
}

/// Hybrid retrieval: BM25 + trigram cosine, fused by reciprocal rank, then
/// nudged toward recent documents.
#[tauri::command]
pub fn rag_search(
    query: String,
    limit: Option<usize>,
    state: tauri::State<'_, RagState>,
) -> Result<Vec<SearchHit>, String> {
    let index = state.inner.lock().map_err(|e| e.to_string())?;
    if index.docs.is_empty() {
        return Ok(Vec::new());
    }

    let terms = tokenize(&query);
    let lexical = index.bm25(&terms);
    let fuzzy = index.trigram_cosine(&query);

    // Reciprocal rank fusion: 1/(k + rank). Robust to the two rankers having
    // completely different score scales.
    let mut fused: HashMap<usize, f32> = HashMap::new();
    for (rank, (doc_i, _)) in lexical.iter().enumerate() {
        *fused.entry(*doc_i).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    for (rank, (doc_i, _)) in fuzzy.iter().enumerate() {
        *fused.entry(*doc_i).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    if fused.is_empty() {
        return Ok(Vec::new());
    }

    // Recency: full boost for today, decaying to nothing over ~60 days.
    let now = now_secs();
    let mut ranked: Vec<(usize, f32)> = fused
        .into_iter()
        .map(|(doc_i, score)| {
            let age_days = ((now - index.docs[doc_i].created).max(0) as f32) / 86_400.0;
            let recency = (1.0 - age_days / 60.0).clamp(0.0, 1.0);
            (doc_i, score * (1.0 + 0.15 * recency))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let limit = limit.unwrap_or(5).clamp(1, 25);
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(doc_i, score)| {
            let doc = &index.docs[doc_i];
            SearchHit {
                id: doc.id.clone(),
                title: doc.title.clone(),
                source: doc.source.clone(),
                text: doc.text.clone(),
                score: (score * 10_000.0).round() / 10_000.0,
                created: doc.created,
            }
        })
        .collect())
}

#[derive(Serialize)]
pub struct RagStats {
    pub docs: usize,
    pub sources: usize,
    pub terms: usize,
    pub avg_chunk_tokens: f32,
    pub store_path: String,
}

#[tauri::command]
pub fn rag_stats(state: tauri::State<'_, RagState>) -> Result<RagStats, String> {
    let index = state.inner.lock().map_err(|e| e.to_string())?;
    let sources: HashSet<&str> = index.docs.iter().map(|d| d.source.as_str()).collect();
    Ok(RagStats {
        docs: index.docs.len(),
        sources: sources.len(),
        terms: index.postings.len(),
        avg_chunk_tokens: (index.avg_len * 10.0).round() / 10.0,
        store_path: state.path.to_string_lossy().to_string(),
    })
}

#[derive(Serialize)]
pub struct DocSummary {
    pub id: String,
    pub title: String,
    pub source: String,
    pub created: i64,
    pub preview: String,
    pub tags: Vec<String>,
}

/// Newest-first listing for the memory panel.
#[tauri::command]
pub fn rag_list(
    limit: Option<usize>,
    state: tauri::State<'_, RagState>,
) -> Result<Vec<DocSummary>, String> {
    let index = state.inner.lock().map_err(|e| e.to_string())?;
    let mut docs: Vec<&Doc> = index.docs.iter().collect();
    docs.sort_by_key(|d| std::cmp::Reverse(d.created));
    Ok(docs
        .into_iter()
        .take(limit.unwrap_or(100).clamp(1, 1000))
        .map(|d| DocSummary {
            id: d.id.clone(),
            title: d.title.clone(),
            source: d.source.clone(),
            created: d.created,
            preview: d.text.chars().take(160).collect(),
            tags: d.tags.clone(),
        })
        .collect())
}

/// Deletes by document id, or every chunk sharing a source. Exactly one of
/// the two must be supplied.
#[tauri::command]
pub fn rag_forget(
    id: Option<String>,
    source: Option<String>,
    state: tauri::State<'_, RagState>,
) -> Result<usize, String> {
    let mut index = state.inner.lock().map_err(|e| e.to_string())?;
    let before = index.docs.len();

    match (id.as_deref(), source.as_deref()) {
        (Some(id), _) => index.docs.retain(|d| d.id != id),
        (None, Some(src)) => index.docs.retain(|d| d.source != src),
        (None, None) => return Err("Pass either an id or a source to forget.".into()),
    }

    let removed = before - index.docs.len();
    if removed > 0 {
        index.rebuild();
        state.save(&index);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_all_text_with_overlap() {
        let text = "sentence one. ".repeat(300);
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1, "long text should split");
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_CHARS + CHUNK_OVERLAP);
            assert!(!c.is_empty());
        }
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hello there").len(), 1);
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn stemming_collapses_common_suffixes() {
        // The point of the two-pass design: both spellings land on one stem.
        assert_eq!(stem("meetings"), stem("meeting"));
        assert_eq!(stem("meeting"), "meet");
        assert_eq!(stem("running"), "run");
        assert_eq!(stem("deployments"), stem("deployment"));

        // Short words and awkward endings are left alone.
        assert_eq!(stem("run"), "run");
        assert_eq!(stem("is"), "is");
        assert_eq!(stem("address"), "address");
        assert_eq!(stem("class"), "class");
        assert_eq!(stem("classes"), "class");
    }

    #[test]
    fn trigram_cosine_is_self_similar() {
        let a = trigrams("kubernetes deployment");
        let dot: f32 = a.iter().map(|(g, w)| w * a.get(g).unwrap()).sum();
        assert!((dot - 1.0).abs() < 1e-4, "normalized self-similarity is 1");
    }

    #[test]
    fn bm25_ranks_the_matching_doc_first() {
        let mut index = RagIndex::default();
        for (i, text) in [
            "the quarterly budget review covers marketing spend",
            "kubernetes deployment rollout strategy and replicas",
            "grocery list milk eggs bread",
        ]
        .iter()
        .enumerate()
        {
            index.docs.push(Doc {
                id: format!("d{i}"),
                source: "test".into(),
                title: format!("doc {i}"),
                text: (*text).into(),
                created: 0,
                tags: vec![],
            });
        }
        index.rebuild();

        let ranked = index.bm25(&tokenize("kubernetes replicas"));
        assert_eq!(ranked.first().map(|(i, _)| *i), Some(1));
    }

    #[test]
    fn trigrams_survive_a_mangled_query() {
        let mut index = RagIndex::default();
        index.docs.push(Doc {
            id: "d0".into(),
            source: "test".into(),
            title: "notes".into(),
            text: "kubernetes deployment strategy".into(),
            created: 0,
            tags: vec![],
        });
        index.docs.push(Doc {
            id: "d1".into(),
            source: "test".into(),
            title: "notes".into(),
            text: "grocery list milk eggs".into(),
            created: 0,
            tags: vec![],
        });
        index.rebuild();

        // BM25 gets nothing from a misheard token; trigrams still rank it.
        assert!(index.bm25(&tokenize("kubernete deploymen")).is_empty());
        let fuzzy = index.trigram_cosine("kubernete deploymen");
        assert_eq!(fuzzy.first().map(|(i, _)| *i), Some(0));
    }
}
