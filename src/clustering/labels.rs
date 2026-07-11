//! Keyword extraction and label generation shared by document and edge clustering.
//!
//! Terms are unigrams plus adjacent-token bigrams. Scores use a smoothed IDF
//! (`ln((1+N)/(1+df)) + 1`) so no term ever scores exactly zero — this keeps
//! keyword order deterministic even for single-document or single-cluster
//! inputs, where the classic `ln(N/df)` collapses to zero for every term.

use std::collections::{HashMap, HashSet};

/// Common English stop words filtered out during keyword extraction.
const STOP_WORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any", "are",
    "aren't", "as", "at", "be", "because", "been", "before", "being", "below", "between", "both",
    "but", "by", "can", "can't", "cannot", "could", "couldn't", "did", "didn't", "do", "does",
    "doesn't", "doing", "don't", "down", "during", "each", "few", "for", "from", "further", "get",
    "got", "had", "hadn't", "has", "hasn't", "have", "haven't", "having", "he", "her", "here",
    "hers", "herself", "him", "himself", "his", "how", "i", "if", "in", "into", "is", "isn't",
    "it", "its", "itself", "just", "let", "like", "ll", "me", "might", "more", "most", "must",
    "mustn't", "my", "myself", "no", "nor", "not", "now", "of", "off", "on", "once", "only",
    "or", "other", "our", "ours", "ourselves", "out", "over", "own", "re", "s", "same", "shall",
    "shan't", "she", "should", "shouldn't", "so", "some", "such", "t", "than", "that", "the",
    "their", "theirs", "them", "themselves", "then", "there", "these", "they", "this", "those",
    "through", "to", "too", "under", "until", "up", "us", "ve", "very", "was", "wasn't", "we",
    "were", "weren't", "what", "when", "where", "which", "while", "who", "whom", "why", "will",
    "with", "won't", "would", "wouldn't", "you", "your", "yours", "yourself", "yourselves",
];

/// Check if a word is a stop word.
pub(crate) fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

/// Whether a lowercased token survives filtering: >= 3 chars and not a stop word.
fn keep_token(word: &str) -> bool {
    word.len() >= 3 && !is_stop_word(word)
}

/// Lowercased alphanumeric token stream, unfiltered (adjacency preserved).
fn raw_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Tokenize into terms: filtered unigrams plus bigrams of tokens that are
/// adjacent in the original text and both survive filtering.
pub(crate) fn terms_with_bigrams(text: &str) -> Vec<String> {
    let raw = raw_tokens(text);
    let mut terms: Vec<String> = Vec::with_capacity(raw.len() * 2);
    for (i, tok) in raw.iter().enumerate() {
        if keep_token(tok) {
            terms.push(tok.clone());
            if let Some(next) = raw.get(i + 1) {
                if keep_token(next) {
                    terms.push(format!("{tok} {next}"));
                }
            }
        }
    }
    terms
}

/// Smoothed inverse document frequency: `ln((1+n)/(1+df)) + 1`. Always > 0.
fn smoothed_idf(n: f64, df: f64) -> f64 {
    ((1.0 + n) / (1.0 + df)).ln() + 1.0
}

/// Rank terms by TF-IDF with deterministic ordering `(score desc, term asc)`.
fn rank_terms(tf: &HashMap<String, f64>, df: &HashMap<String, f64>, n_docs: f64) -> Vec<String> {
    let mut scores: Vec<(&String, f64)> = tf
        .iter()
        .map(|(term, &tf_val)| {
            let df_val = df.get(term).copied().unwrap_or(1.0);
            (term, tf_val * smoothed_idf(n_docs, df_val))
        })
        .collect();
    scores.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });
    scores.into_iter().map(|(term, _)| term.clone()).collect()
}

/// Extract top-N keywords from a set of texts using TF-IDF over those texts.
///
/// Terms are unigrams + adjacent bigrams; DF counts the texts containing a term.
pub(crate) fn extract_keywords(texts: &[&str], n: usize) -> Vec<String> {
    if texts.is_empty() || n == 0 {
        return Vec::new();
    }

    let tokenized: Vec<Vec<String>> = texts.iter().map(|t| terms_with_bigrams(t)).collect();
    let num_docs = tokenized.len() as f64;

    let mut tf: HashMap<String, f64> = HashMap::new();
    let mut df: HashMap<String, f64> = HashMap::new();
    for doc_terms in &tokenized {
        for term in doc_terms {
            *tf.entry(term.clone()).or_insert(0.0) += 1.0;
        }
        let unique: HashSet<&String> = doc_terms.iter().collect();
        for term in unique {
            *df.entry(term.clone()).or_insert(0.0) += 1.0;
        }
    }

    rank_terms(&tf, &df, num_docs).into_iter().take(n).collect()
}

/// Compute per-cluster keywords where IDF is computed **across clusters**, so
/// terms shared by many clusters are down-weighted and cluster-distinctive
/// terms are promoted. `cluster_texts[i]` holds the member texts of cluster i.
pub(crate) fn cross_cluster_keywords(cluster_texts: &[Vec<&str>], n: usize) -> Vec<Vec<String>> {
    if cluster_texts.is_empty() {
        return Vec::new();
    }

    let num_clusters = cluster_texts.len() as f64;

    let mut cluster_tfs: Vec<HashMap<String, f64>> = Vec::with_capacity(cluster_texts.len());
    let mut cross_df: HashMap<String, f64> = HashMap::new();

    for texts in cluster_texts {
        let mut tf: HashMap<String, f64> = HashMap::new();
        for text in texts {
            for term in terms_with_bigrams(text) {
                *tf.entry(term).or_insert(0.0) += 1.0;
            }
        }
        for term in tf.keys() {
            *cross_df.entry(term.clone()).or_insert(0.0) += 1.0;
        }
        cluster_tfs.push(tf);
    }

    cluster_tfs
        .iter()
        .map(|tf| {
            rank_terms(tf, &cross_df, num_clusters)
                .into_iter()
                .take(n)
                .collect()
        })
        .collect()
}

/// Generate a human-readable label from ranked keywords.
///
/// Picks up to 3 terms in rank order, skipping unigrams that already appear
/// as a word inside a previously chosen bigram (avoids "rust / rust programming").
pub(crate) fn generate_label(keywords: &[String]) -> String {
    let mut chosen: Vec<&str> = Vec::with_capacity(3);
    for kw in keywords {
        let is_unigram = !kw.contains(' ');
        let redundant = is_unigram
            && chosen
                .iter()
                .any(|c| c.contains(' ') && c.split(' ').any(|part| part == kw));
        if !redundant {
            chosen.push(kw.as_str());
        }
        if chosen.len() == 3 {
            break;
        }
    }
    if chosen.is_empty() {
        "Unlabeled".to_string()
    } else {
        chosen.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_words_contains_common_words() {
        assert!(is_stop_word("the"));
        assert!(is_stop_word("and"));
        assert!(is_stop_word("is"));
        assert!(!is_stop_word("clustering"));
        assert!(!is_stop_word("vector"));
    }

    #[test]
    fn tokenize_filters_short_and_stopwords() {
        let tokens = terms_with_bigrams("The big cat is on a mat");
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"on".to_string()));
        assert!(tokens.contains(&"big".to_string()));
        assert!(tokens.contains(&"cat".to_string()));
        assert!(tokens.contains(&"mat".to_string()));
    }

    #[test]
    fn terms_include_adjacent_bigrams() {
        let terms = terms_with_bigrams("rust programming makes systems safe");
        assert!(terms.contains(&"rust".to_string()));
        assert!(terms.contains(&"rust programming".to_string()));
        assert!(terms.contains(&"systems safe".to_string()));
    }

    #[test]
    fn bigrams_not_formed_across_stopwords() {
        // "quick" and "brown" are separated by the stop word "and" — no bigram.
        let terms = terms_with_bigrams("quick and brown");
        assert!(terms.contains(&"quick".to_string()));
        assert!(terms.contains(&"brown".to_string()));
        assert!(!terms.contains(&"quick brown".to_string()));
        assert!(!terms.iter().any(|t| t.contains("and")));
    }

    #[test]
    fn smoothed_idf_never_zero() {
        assert!(smoothed_idf(1.0, 1.0) > 0.0);
        assert!(smoothed_idf(100.0, 100.0) > 0.0);
        assert!(smoothed_idf(1.0, 0.0) > 0.0);
    }

    #[test]
    fn extract_keywords_no_stopwords() {
        let docs = vec![
            "The quick brown fox jumps over the lazy dog",
            "A brown fox is quick and nimble",
            "Foxes are brown animals that jump quickly",
        ];
        let keywords = extract_keywords(&docs, 5);
        for kw in &keywords {
            for part in kw.split(' ') {
                assert!(!is_stop_word(part), "keyword '{kw}' contains a stop word");
            }
        }
        assert!(!keywords.is_empty());
    }

    #[test]
    fn extract_keywords_empty_docs() {
        assert!(extract_keywords(&[], 5).is_empty());
    }

    #[test]
    fn extract_keywords_respects_n() {
        let docs = vec!["rust programming language systems performance memory safety"];
        let keywords = extract_keywords(&docs, 3);
        assert!(keywords.len() <= 3);
    }

    #[test]
    fn single_doc_keywords_deterministic_nonzero() {
        // With classic ln(N/df), a single document gives every term score 0 and
        // arbitrary HashMap order. Smoothed IDF + tie-break keeps this stable.
        let docs = vec!["zebra apple zebra mango apple zebra"];
        let a = extract_keywords(&docs, 3);
        let b = extract_keywords(&docs, 3);
        assert_eq!(a, b);
        // "zebra" (tf 3) must outrank "apple" (tf 2) and "mango" (tf 1).
        assert_eq!(a[0], "zebra");
    }

    #[test]
    fn cross_cluster_promotes_distinctive_terms() {
        let clusters = vec![
            vec!["rust cargo borrow checker rust cargo"],
            vec!["cooking recipe kitchen food recipe"],
            vec!["rust cooking shared words"],
        ];
        let keywords = cross_cluster_keywords(&clusters, 3);
        assert_eq!(keywords.len(), 3);
        for kws in &keywords {
            assert!(!kws.is_empty());
        }
    }

    #[test]
    fn cross_cluster_single_cluster_still_ranked() {
        // num_clusters == 1 used to make every IDF zero (arbitrary order).
        let clusters = vec![vec!["zebra zebra zebra apple apple mango"]];
        let keywords = cross_cluster_keywords(&clusters, 2);
        assert_eq!(keywords[0][0], "zebra");
    }

    #[test]
    fn generate_label_format() {
        let keywords = vec![
            "rust".to_string(),
            "programming".to_string(),
            "systems".to_string(),
            "extra".to_string(),
        ];
        assert_eq!(generate_label(&keywords), "rust / programming / systems");
    }

    #[test]
    fn generate_label_fewer_than_three() {
        let keywords = vec!["rust".to_string()];
        assert_eq!(generate_label(&keywords), "rust");
    }

    #[test]
    fn generate_label_empty() {
        assert_eq!(generate_label(&[]), "Unlabeled");
    }

    #[test]
    fn generate_label_skips_unigram_covered_by_bigram() {
        let keywords = vec![
            "rust programming".to_string(),
            "rust".to_string(),
            "cargo".to_string(),
            "tokio".to_string(),
        ];
        assert_eq!(generate_label(&keywords), "rust programming / cargo / tokio");
    }
}
