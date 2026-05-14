//! Frontier-based benchmark instrumentation (antirez/ds4 inspired、`.claude/plan/frontier-benchmark-impl.md`)。
//!
//! Lab 天井 7 連続 (v8-v17) で打破できなかった理由 = score/capability/efficiency/stability/retrieval の
//! 5 軸はすべて入力 context 長を fixed と仮定。本モジュールは **第 6 軸 = context-length axis** を
//! 開拓するための pure-function helper + struct を提供する。
//!
//! 設計:
//! - `frontier_bucket_for` = 累積 token を {0, 2K, 4K, 8K, 16K+} の 4 bucket に振り分ける純粋関数
//! - `parse_frontier_buckets_env` = `BONSAI_FRONTIER_BUCKETS=2048,4096,8192,16384` 解析
//! - `is_frontier_enabled` = env opt-in (default OFF、Cerememory 三本柱 pattern)
//! - `compute_frontier_bucket_scores` = task-bucket aggregation (post-hoc bucketing for 案 C)
//!
//! Phase 1 (Red) の本 module には実装ナシ (todo!())、test だけ先に書く。

use std::collections::BTreeMap;

/// Default frontier bucket boundaries (token counts)。
/// bucket 0 = [0, 2048), bucket 1 = [2048, 4096), bucket 2 = [4096, 8192), bucket 3 = [8192, ∞)。
pub const DEFAULT_FRONTIER_BUCKETS: &[usize] = &[2048, 4096, 8192];

/// Default frontier inject sizes (KB) for T6-LongHorizon filler variants (Sub-Phase 2E、案 C 2nd pillar)。
/// 0 は通常 run = baseline、4/8/16 KB は filler context inject variant。
/// 4 種 × T6 タスク N 件 = 4N runs を Phase 4 Smoke G-4b で実行する設計。
pub const DEFAULT_FRONTIER_INJECT_SIZES_KB: &[usize] = &[0, 4, 8, 16];

/// `BONSAI_FRONTIER_ENABLED=1` の場合のみ frontier metric を populate する。
/// 未指定 / `0` / 他値で `false` (default OFF / Cerememory 三本柱 pattern)。
pub fn is_frontier_enabled() -> bool {
    std::env::var("BONSAI_FRONTIER_ENABLED").ok().as_deref() == Some("1")
}

/// `BONSAI_FRONTIER_BUCKETS=2048,4096,8192,16384` を解析。
/// 未指定 / parse 失敗 / 空 / 非単調増加で [`DEFAULT_FRONTIER_BUCKETS`] を返す。
/// 返り値は **昇順に sort 済** で重複なし、最低 1 要素を保証。
pub fn parse_frontier_buckets_env() -> Vec<usize> {
    let Ok(raw) = std::env::var("BONSAI_FRONTIER_BUCKETS") else {
        return DEFAULT_FRONTIER_BUCKETS.to_vec();
    };
    if raw.trim().is_empty() {
        return DEFAULT_FRONTIER_BUCKETS.to_vec();
    }
    let parsed: Option<Vec<usize>> = raw
        .split(',')
        .map(|s| s.trim().parse::<usize>().ok())
        .collect();
    let Some(values) = parsed else {
        return DEFAULT_FRONTIER_BUCKETS.to_vec();
    };
    if values.is_empty() {
        return DEFAULT_FRONTIER_BUCKETS.to_vec();
    }
    // 厳格な単調増加 (重複も拒否) を要求、違反したら default fallback。
    // 「caller の意図不明な順序を sort して救う」よりも「invalid input は明示的に reject」が
    // observable な debug 体験を生む (env 値が想定と違うことに早く気付ける)。
    let is_strictly_increasing = values.windows(2).all(|w| w[0] < w[1]);
    if !is_strictly_increasing {
        return DEFAULT_FRONTIER_BUCKETS.to_vec();
    }
    values
}

/// 累積 token 数 (`token_count`) を bucket index (0-based) に振り分ける。
/// `boundaries` は昇順 sort 済 / 重複なしを前提 (`parse_frontier_buckets_env` の返り値を渡す)。
/// 戻り値: `Some(N)` = bucket N に該当、`None` = 振り分け不能 (boundaries 空)。
///
/// 例: boundaries=[2048, 4096, 8192]、token_count=1500 → Some(0)、token_count=3000 → Some(1)、
/// token_count=5000 → Some(2)、token_count=10000 → Some(3) (末尾 unbounded bucket)。
pub fn frontier_bucket_for(token_count: usize, boundaries: &[usize]) -> Option<usize> {
    if boundaries.is_empty() {
        return None;
    }
    // 半開区間 [boundaries[i-1], boundaries[i]) で bucket i-1 を表現。
    // boundaries[0]=2048 → bucket 0 = [0, 2048)、bucket 1 = [2048, 4096) etc.
    // 末尾 unbounded bucket index = boundaries.len()。
    let bucket = boundaries.iter().position(|&b| token_count < b);
    Some(bucket.unwrap_or(boundaries.len()))
}

/// `BONSAI_FRONTIER_INJECT_ENABLED=1` の場合のみ T6-LongHorizon filler inject runs を実行する。
/// 未指定 / `0` / 他値で `false` (default OFF / Cerememory 三本柱 pattern)。
/// `is_frontier_enabled` (bucketing 軸) とは独立 = 両者を組合せても良いし片方だけでも可。
pub fn is_frontier_inject_enabled() -> bool {
    std::env::var("BONSAI_FRONTIER_INJECT_ENABLED")
        .ok()
        .as_deref()
        == Some("1")
}

/// `BONSAI_FRONTIER_INJECT_SIZES_KB=0,4,8,16` を解析。
/// 未指定 / parse 失敗 / 空 / 非単調増加で [`DEFAULT_FRONTIER_INJECT_SIZES_KB`] を返す。
/// 0 KB (= baseline、no filler) も含めて返す: filler 0 KB 経路で baseline score を別途取得することで、
/// 4/8/16 KB の degradation curve を baseline と直接比較できる。
pub fn parse_frontier_inject_sizes_env() -> Vec<usize> {
    let Ok(raw) = std::env::var("BONSAI_FRONTIER_INJECT_SIZES_KB") else {
        return DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec();
    };
    if raw.trim().is_empty() {
        return DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec();
    }
    let parsed: Option<Vec<usize>> = raw
        .split(',')
        .map(|s| s.trim().parse::<usize>().ok())
        .collect();
    let Some(values) = parsed else {
        return DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec();
    };
    if values.is_empty() {
        return DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec();
    }
    // 厳格な単調増加 (重複も拒否) を要求、違反したら default fallback。
    // `parse_frontier_buckets_env` と同 contract で observable な debug 体験を統一する。
    let is_strictly_increasing = values.windows(2).all(|w| w[0] < w[1]);
    if !is_strictly_increasing {
        return DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec();
    }
    values
}

/// T6-LongHorizon タスクの description に **deterministic filler context** を inject する。
/// `size_kb` * 1024 byte ≈ size の filler 文字列を append、reproducibility 優先で乱数性なし。
///
/// filler 内容 = 同じ文字パターンの繰り返しで、LLM が「meaningful」と誤認しない workload を作る:
/// - 4 KB: `"\n[filler-context] ..."` × 約 100 回 (40 byte/line)
/// - 8 KB: 約 200 回 / 16 KB: 約 400 回 (線形 scaling)
///
/// `size_kb == 0` のときは入力 description を変更せず返す (baseline 経路の no-op)。
pub fn inject_filler_context(description: &str, size_kb: usize) -> String {
    if size_kb == 0 {
        return description.to_string();
    }
    // 40 byte / line × (size_kb * 1024 / 40) ≒ target byte 数
    // 1 line = `"\n[filler-context] padding padding padding"` (= 41 byte 厳密)
    const LINE_PATTERN: &str = "\n[filler-context] padding padding padding";
    let target_bytes = size_kb * 1024;
    let lines_needed = target_bytes / LINE_PATTERN.len() + 1;
    let mut buf = String::with_capacity(description.len() + lines_needed * LINE_PATTERN.len());
    buf.push_str(description);
    for _ in 0..lines_needed {
        buf.push_str(LINE_PATTERN);
    }
    buf
}

/// task ごとの (累積 token, score) ペアを受け取り、bucket 別の mean score を集計する。
/// `boundaries` は [`parse_frontier_buckets_env`] と同 contract (昇順 sort 済 / 重複なし)。
///
/// 戻り値: `BTreeMap<bucket_index, mean_score>` (bucket 0 から昇順)。
/// 該当 bucket に sample 0 件のときはその key は出力に含めない。
/// task ペア空 / boundaries 空のときは空 map を返す。
pub fn compute_frontier_bucket_scores(
    task_results: &[(usize, f64)],
    boundaries: &[usize],
) -> BTreeMap<usize, f64> {
    if task_results.is_empty() || boundaries.is_empty() {
        return BTreeMap::new();
    }
    // bucket → (sum, count) で集計後、最後に mean を計算。
    let mut acc: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
    for (tokens, score) in task_results {
        if let Some(bucket) = frontier_bucket_for(*tokens, boundaries) {
            let entry = acc.entry(bucket).or_insert((0.0, 0));
            entry.0 += score;
            entry.1 += 1;
        }
    }
    acc.into_iter()
        .map(|(bucket, (sum, count))| (bucket, sum / count as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// env mutex (項目 226 CRITIC_TEST_LOCK / 項目 225 PASS_K_T_TEST_LOCK と同 pattern)。
    /// `BONSAI_FRONTIER_*` env を読む test 間の race condition を防ぐ。
    static FRONTIER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn t_frontier_bucket_assignment_correct() {
        // boundaries=[2048, 4096, 8192] に対し、各 bucket の代表 token count が
        // 正しく Some(N) を返すこと。末尾 unbounded bucket (N=len(boundaries)) も検証。
        let b = &[2048, 4096, 8192];
        assert_eq!(frontier_bucket_for(0, b), Some(0));
        assert_eq!(frontier_bucket_for(1500, b), Some(0));
        assert_eq!(frontier_bucket_for(2048, b), Some(1));
        assert_eq!(frontier_bucket_for(3000, b), Some(1));
        assert_eq!(frontier_bucket_for(4096, b), Some(2));
        assert_eq!(frontier_bucket_for(5000, b), Some(2));
        assert_eq!(frontier_bucket_for(8192, b), Some(3));
        assert_eq!(frontier_bucket_for(20000, b), Some(3));
    }

    #[test]
    fn t_frontier_bucket_empty_boundaries_returns_none() {
        // boundaries 空 → None を返す (caller での fallback 経路を明示化)。
        assert_eq!(frontier_bucket_for(1500, &[]), None);
    }

    #[test]
    fn t_is_frontier_enabled_default_off() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap();
        // Safety: Rust 2024 で env var 操作は unsafe block 必須。
        unsafe { std::env::remove_var("BONSAI_FRONTIER_ENABLED") };
        assert!(!is_frontier_enabled());

        unsafe { std::env::set_var("BONSAI_FRONTIER_ENABLED", "0") };
        assert!(!is_frontier_enabled());

        unsafe { std::env::set_var("BONSAI_FRONTIER_ENABLED", "1") };
        assert!(is_frontier_enabled());

        unsafe { std::env::remove_var("BONSAI_FRONTIER_ENABLED") };
    }

    #[test]
    fn t_parse_frontier_buckets_env_default_when_unset() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("BONSAI_FRONTIER_BUCKETS") };
        let buckets = parse_frontier_buckets_env();
        assert_eq!(buckets, DEFAULT_FRONTIER_BUCKETS.to_vec());
    }

    #[test]
    fn t_parse_frontier_buckets_env_custom() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("BONSAI_FRONTIER_BUCKETS", "1000,5000,10000,20000") };
        let buckets = parse_frontier_buckets_env();
        assert_eq!(buckets, vec![1000, 5000, 10000, 20000]);
        unsafe { std::env::remove_var("BONSAI_FRONTIER_BUCKETS") };
    }

    #[test]
    fn t_parse_frontier_buckets_env_falls_back_to_default_on_bad_input() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap();
        // 非単調増加 → default fallback
        unsafe { std::env::set_var("BONSAI_FRONTIER_BUCKETS", "5000,1000,3000") };
        let buckets = parse_frontier_buckets_env();
        assert_eq!(buckets, DEFAULT_FRONTIER_BUCKETS.to_vec());

        // parse 失敗 → default fallback
        unsafe { std::env::set_var("BONSAI_FRONTIER_BUCKETS", "abc,def") };
        let buckets = parse_frontier_buckets_env();
        assert_eq!(buckets, DEFAULT_FRONTIER_BUCKETS.to_vec());

        // 空文字列 → default fallback
        unsafe { std::env::set_var("BONSAI_FRONTIER_BUCKETS", "") };
        let buckets = parse_frontier_buckets_env();
        assert_eq!(buckets, DEFAULT_FRONTIER_BUCKETS.to_vec());

        unsafe { std::env::remove_var("BONSAI_FRONTIER_BUCKETS") };
    }

    #[test]
    fn t_compute_frontier_bucket_scores_aggregation() {
        // boundaries=[2048, 4096, 8192] (= 4 bucket)、5 task の (token, score):
        //   - 1500 → bucket 0, score=0.8
        //   - 1800 → bucket 0, score=0.6  → bucket 0 mean = 0.7
        //   - 3000 → bucket 1, score=0.5  → bucket 1 mean = 0.5
        //   - 5000 → bucket 2, score=0.4  → bucket 2 mean = 0.4
        //   - 12000 → bucket 3, score=0.2 → bucket 3 mean = 0.2
        let b = &[2048, 4096, 8192];
        let pairs = vec![
            (1500usize, 0.8),
            (1800, 0.6),
            (3000, 0.5),
            (5000, 0.4),
            (12000, 0.2),
        ];
        let result = compute_frontier_bucket_scores(&pairs, b);
        assert_eq!(result.len(), 4);
        assert!((result[&0] - 0.7).abs() < 1e-9);
        assert!((result[&1] - 0.5).abs() < 1e-9);
        assert!((result[&2] - 0.4).abs() < 1e-9);
        assert!((result[&3] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn t_compute_frontier_bucket_scores_skips_empty_buckets() {
        // bucket 2 にサンプルなし → 出力 map に key 2 を含めない (sparse 表現)。
        let b = &[2048, 4096, 8192];
        let pairs = vec![(1500usize, 0.8), (3000, 0.5), (12000, 0.2)];
        let result = compute_frontier_bucket_scores(&pairs, b);
        assert_eq!(result.len(), 3);
        assert!(result.contains_key(&0));
        assert!(result.contains_key(&1));
        assert!(
            !result.contains_key(&2),
            "bucket 2 にサンプルなしなら key 不在"
        );
        assert!(result.contains_key(&3));
    }

    #[test]
    fn t_compute_frontier_bucket_scores_empty_inputs() {
        // 空 task pair / 空 boundaries → 空 map。
        assert!(compute_frontier_bucket_scores(&[], &[2048, 4096]).is_empty());
        assert!(compute_frontier_bucket_scores(&[(1500, 0.8)], &[]).is_empty());
    }

    // ── Sub-Phase 2E: filler context inject helpers ──

    #[test]
    fn t_is_frontier_inject_enabled_default_off() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("BONSAI_FRONTIER_INJECT_ENABLED") };
        assert!(!is_frontier_inject_enabled());

        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_ENABLED", "0") };
        assert!(!is_frontier_inject_enabled());

        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_ENABLED", "1") };
        assert!(is_frontier_inject_enabled());

        unsafe { std::env::remove_var("BONSAI_FRONTIER_INJECT_ENABLED") };
    }

    #[test]
    fn t_parse_frontier_inject_sizes_env_default_when_unset() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("BONSAI_FRONTIER_INJECT_SIZES_KB") };
        let sizes = parse_frontier_inject_sizes_env();
        assert_eq!(sizes, DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec());
    }

    #[test]
    fn t_parse_frontier_inject_sizes_env_custom() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_SIZES_KB", "0,2,8,32") };
        let sizes = parse_frontier_inject_sizes_env();
        assert_eq!(sizes, vec![0, 2, 8, 32]);
        unsafe { std::env::remove_var("BONSAI_FRONTIER_INJECT_SIZES_KB") };
    }

    #[test]
    fn t_parse_frontier_inject_sizes_env_falls_back_on_bad_input() {
        let _guard = FRONTIER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 非単調増加
        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_SIZES_KB", "16,4,8") };
        assert_eq!(
            parse_frontier_inject_sizes_env(),
            DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec()
        );
        // parse 失敗
        unsafe { std::env::set_var("BONSAI_FRONTIER_INJECT_SIZES_KB", "abc,def") };
        assert_eq!(
            parse_frontier_inject_sizes_env(),
            DEFAULT_FRONTIER_INJECT_SIZES_KB.to_vec()
        );
        unsafe { std::env::remove_var("BONSAI_FRONTIER_INJECT_SIZES_KB") };
    }

    #[test]
    fn t_inject_filler_context_zero_size_no_op() {
        // size_kb=0 → description 不変 (baseline 経路)
        let desc = "Original task description";
        let result = inject_filler_context(desc, 0);
        assert_eq!(result, desc);
    }

    #[test]
    fn t_inject_filler_context_size_proportional() {
        // size_kb=4 で約 4 KB の filler を append、original description が prefix で保持されること。
        let desc = "Original task description";
        let result_4 = inject_filler_context(desc, 4);
        let result_8 = inject_filler_context(desc, 8);
        let result_16 = inject_filler_context(desc, 16);
        assert!(result_4.starts_with(desc), "original prefix 保持");
        assert!(result_4.len() >= desc.len() + 4 * 1024);
        // 8 KB ≈ 2x の 4 KB、16 KB ≈ 4x の 4 KB の filler byte 量
        let filler_4 = result_4.len() - desc.len();
        let filler_8 = result_8.len() - desc.len();
        let filler_16 = result_16.len() - desc.len();
        // 線形 scaling 確認 (±10% 範囲、line pattern 長による端数許容)
        assert!(
            (filler_8 as f64 / filler_4 as f64 - 2.0).abs() < 0.1,
            "8 KB ≈ 2x の 4 KB filler"
        );
        assert!(
            (filler_16 as f64 / filler_4 as f64 - 4.0).abs() < 0.1,
            "16 KB ≈ 4x の 4 KB filler"
        );
    }

    #[test]
    fn t_inject_filler_context_deterministic() {
        // 同じ (description, size_kb) で 2 回呼んでも同じ結果 (reproducibility 確証)
        let desc = "Reproducibility test";
        let a = inject_filler_context(desc, 4);
        let b = inject_filler_context(desc, 4);
        assert_eq!(a, b);
    }
}
