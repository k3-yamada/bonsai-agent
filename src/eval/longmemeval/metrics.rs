//! LongMemEval retrieval metrics: recall_any@K / NDCG@K / MRR.
//!
//! `retrieved` / `gold` は session_id の Vec<String> として扱う。

use std::collections::HashSet;

pub fn recall_any_at_k(retrieved: &[String], gold: &[String], k: usize) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let gold_set: HashSet<&String> = gold.iter().collect();
    let hit = retrieved.iter().take(k).any(|r| gold_set.contains(r));
    if hit { 1.0 } else { 0.0 }
}

pub fn ndcg_at_k(retrieved: &[String], gold: &[String], k: usize) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let gold_set: HashSet<&String> = gold.iter().collect();
    let dcg: f64 = retrieved
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, id)| gold_set.contains(*id))
        .map(|(i, _)| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    let ideal_size = gold.len().min(k);
    let idcg: f64 = (0..ideal_size)
        .map(|i| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

pub fn mrr(retrieved: &[String], gold: &[String]) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let gold_set: HashSet<&String> = gold.iter().collect();
    retrieved
        .iter()
        .position(|id| gold_set.contains(id))
        .map(|i| 1.0 / (i as f64 + 1.0))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn test_recall_any_at_k_hit_at_top() {
        let retrieved = s(&["g1", "x1", "x2"]);
        let gold = s(&["g1"]);
        assert_eq!(recall_any_at_k(&retrieved, &gold, 5), 1.0);
    }

    #[test]
    fn test_recall_any_at_k_miss() {
        let retrieved = s(&["x1", "x2", "x3"]);
        let gold = s(&["g1"]);
        assert_eq!(recall_any_at_k(&retrieved, &gold, 5), 0.0);
    }

    #[test]
    fn test_recall_any_at_k_hit_outside_k() {
        let mut retrieved: Vec<String> = (0..10).map(|i| format!("x{i}")).collect();
        retrieved.push("g1".to_string());
        let gold = s(&["g1"]);
        assert_eq!(recall_any_at_k(&retrieved, &gold, 5), 0.0);
        assert_eq!(recall_any_at_k(&retrieved, &gold, 20), 1.0);
    }

    #[test]
    fn test_ndcg_at_10_perfect_ranking() {
        let retrieved = s(&["g1", "x1", "x2"]);
        let gold = s(&["g1"]);
        let v = ndcg_at_k(&retrieved, &gold, 10);
        assert!((v - 1.0).abs() < 1e-9, "expected 1.0, got {v}");
    }

    #[test]
    fn test_ndcg_at_10_partial() {
        // gold at 0-indexed rank 5 → DCG = 1 / log2(5+2) = 1/log2(7)
        // IDCG (1 gold) = 1 / log2(2) = 1.0
        let mut retrieved = s(&["x0", "x1", "x2", "x3", "x4"]);
        retrieved.push("g1".to_string());
        let gold = s(&["g1"]);
        let v = ndcg_at_k(&retrieved, &gold, 10);
        let expected = 1.0 / (7f64.log2());
        assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
    }

    #[test]
    fn test_mrr_first_hit_rank_3() {
        // gold at 0-indexed rank 2 → MRR = 1/(2+1) = 1/3
        let retrieved = s(&["x0", "x1", "g1", "x3"]);
        let gold = s(&["g1"]);
        let v = mrr(&retrieved, &gold);
        assert!((v - 1.0 / 3.0).abs() < 1e-9, "expected 1/3, got {v}");
    }

    #[test]
    fn test_mrr_no_hit() {
        let retrieved = s(&["x0", "x1"]);
        let gold = s(&["g1"]);
        assert_eq!(mrr(&retrieved, &gold), 0.0);
    }
}
