//! 既知モデルの静的プロファイル registry — model 切替の single source of truth。
//!
//! `ModelConfig` (config 層) はここから既定値を導出する (`from_profile`)。
//! domain 層は依存ゼロの純粋型のみを置く方針 (DEP-001) のため、config 型には依存しない。

/// 既知モデルの静的プロファイル。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelProfile {
    /// config.toml `model_id` の短縮名。例 "minicpm5-2b"
    pub id: &'static str,
    /// 表示名。例 "MiniCPM5-2B"
    pub display_name: &'static str,
    /// GGUF 配布元の Hugging Face repo。例 "openbmb/MiniCPM5-2B-GGUF"
    pub gguf_repo: &'static str,
    /// GGUF ファイル名。例 "MiniCPM5-2B-Q4_K_M.gguf"
    pub gguf_file: &'static str,
    /// MLX 配布元の Hugging Face repo。未提供モデルは空文字列。
    pub mlx_repo: &'static str,
    /// model card 記載の max_position_embeddings。
    pub native_context: u32,
    /// 推奨 context_length (config 既定値)。
    pub default_context: u32,
    /// 推論パラメータの推奨初期値。
    pub inference: InferenceDefaults,
    /// 1 行メモ (legacy 等)。`notes` が非空なら `--diagnose` の profile 行に表示される。
    pub notes: &'static str,
}

/// 推論パラメータの推奨初期値。
/// `config::InferenceParams` とは別の純粋型 (domain は config に依存しない)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InferenceDefaults {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub min_p: f64,
    pub max_tokens: u32,
    pub repeat_penalty: f64,
}

/// 既定モデル id。
pub const DEFAULT_MODEL_ID: &str = "minicpm5-2b";

const PROFILES: &[ModelProfile] = &[
    ModelProfile {
        id: "minicpm5-2b",
        display_name: "MiniCPM5-2B",
        gguf_repo: "openbmb/MiniCPM5-2B-GGUF",
        gguf_file: "MiniCPM5-2B-Q4_K_M.gguf",
        mlx_repo: "openbmb/MiniCPM5-2B-MLX",
        native_context: 131_072,
        default_context: 32_768,
        inference: InferenceDefaults {
            temperature: 0.6,
            top_p: 0.95,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 2048,
            repeat_penalty: 1.05,
        },
        notes: "既定モデル。初期推奨値は未検証、Lab paired evidence (ADR-003) で後日チューニング",
    },
    ModelProfile {
        id: "minicpm5-1b",
        display_name: "MiniCPM5-1B",
        gguf_repo: "openbmb/MiniCPM5-1B-GGUF",
        gguf_file: "MiniCPM5-1B-Q4_K_M.gguf",
        mlx_repo: "openbmb/MiniCPM5-1B-MLX",
        native_context: 131_072,
        default_context: 32_768,
        inference: InferenceDefaults {
            temperature: 0.6,
            top_p: 0.95,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 2048,
            repeat_penalty: 1.05,
        },
        notes: "file 名は HF で要確認 (推定)",
    },
    ModelProfile {
        id: "bonsai-8b",
        display_name: "Bonsai-8B",
        gguf_repo: "prism-ml/Bonsai-8B-gguf",
        gguf_file: "Bonsai-8B.gguf",
        mlx_repo: "",
        native_context: 16_384,
        default_context: 16_384,
        inference: InferenceDefaults {
            temperature: 0.5,
            top_p: 0.85,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 1024,
            repeat_penalty: 1.15,
        },
        notes: "legacy: 1-bit Qwen3-8B、Bonsai-demo の llama-server fork が必要",
    },
    ModelProfile {
        id: "ternary-bonsai-8b",
        display_name: "Ternary-Bonsai-8B",
        gguf_repo: "prism-ml/Ternary-Bonsai-8B-GGUF",
        gguf_file: "Ternary-Bonsai-8B.gguf",
        mlx_repo: "prism-ml/Ternary-Bonsai-8B-mlx-2bit",
        native_context: 65_536,
        default_context: 65_536,
        inference: InferenceDefaults {
            temperature: 0.3,
            top_p: 0.9,
            top_k: 20,
            min_p: 0.05,
            max_tokens: 1024,
            repeat_penalty: 1.1,
        },
        notes: "legacy: PrismML MLX fork 必要",
    },
];

/// 登録済み全プロファイル。
pub fn known_profiles() -> &'static [ModelProfile] {
    PROFILES
}

/// 既定プロファイル (`id == DEFAULT_MODEL_ID`)。
pub fn default_profile() -> &'static ModelProfile {
    // 不変条件: PROFILES は DEFAULT_MODEL_ID を必ず含む
    // (test_default_profile_id_matches_default_model_id で保証)。
    known_profiles()
        .iter()
        .find(|p| p.id == DEFAULT_MODEL_ID)
        .expect("DEFAULT_MODEL_ID must exist in PROFILES")
}

/// GGUF ファイル名から拡張子 `.gguf` を除いた stem を返す。
fn gguf_stem(file: &str) -> &str {
    file.strip_suffix(".gguf").unwrap_or(file)
}

/// `model_id` を id / gguf_repo / mlx_repo / gguf_file (拡張子除く) のいずれかと
/// case-insensitive に一致するプロファイルとして解決する。
/// 一致しなければ `None` (未知モデルは自由文字列として許容する呼び出し側の責務)。
pub fn find_profile(model_id: &str) -> Option<&'static ModelProfile> {
    let input_stem = gguf_stem(model_id);
    known_profiles().iter().find(|p| {
        p.id.eq_ignore_ascii_case(model_id)
            || p.gguf_repo.eq_ignore_ascii_case(model_id)
            || (!p.mlx_repo.is_empty() && p.mlx_repo.eq_ignore_ascii_case(model_id))
            || gguf_stem(p.gguf_file).eq_ignore_ascii_case(input_stem)
    })
}

/// model_id 解決順序 (main.rs から純粋関数として抽出、テスト可能に):
/// 1. `--model` CLI  2. `BONSAI_MODEL` env  3. `UNSLOTH_MODEL` env (後方互換)
/// 4. backend == Unsloth かつ config の model_id が `DEFAULT_MODEL_ID`（現行既定 "minicpm5-2b"）
///    または `"bonsai-8b"`（旧既定、後方互換）のままなら `"Aratako/Qwen3-8B-ERP-v0.1-GGUF"`
///    (既存挙動維持)
/// 5. config の model_id
///
/// CLI / env は空文字列・空白のみを「未指定」として扱う (trim 後 empty は次の優先順位に fall through)。
pub fn resolve_model_id(
    cli_model: Option<&str>,
    env_bonsai_model: Option<&str>,
    env_unsloth_model: Option<&str>,
    backend_is_unsloth: bool,
    config_model_id: &str,
) -> String {
    if let Some(m) = non_blank(cli_model) {
        return m.to_string();
    }
    if let Some(m) = non_blank(env_bonsai_model) {
        return m.to_string();
    }
    if let Some(m) = non_blank(env_unsloth_model) {
        return m.to_string();
    }
    if backend_is_unsloth && (config_model_id == DEFAULT_MODEL_ID || config_model_id == "bonsai-8b")
    {
        return "Aratako/Qwen3-8B-ERP-v0.1-GGUF".to_string();
    }
    config_model_id.to_string()
}

/// 空文字列・空白のみの `Some` を「未指定」扱いにする正規化ヘルパー (LOW-3)。
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// `BONSAI_LAB_MLX_ONLY=1` で primary backend を MLX に切替える際の model_id 置換 (L-6)。
///
/// main.rs から純粋関数として抽出 (テスト可能に、SIZE-001 800 行制約対応)。
///
/// - `current` が未知 model_id (`find_profile` が `None`) → `current` を **そのまま保持**
///   (書き換えない)。未知の自由文字列 model_id を無関係な既定 profile の mlx_repo で
///   上書きしないため。
/// - ヒットしたが `mlx_repo` が空文字列 (例 "bonsai-8b") → `default_profile().mlx_repo`
///   に置換 (legacy profile は MLX 未提供のため、既定 profile の MLX 実装で代替する)。
/// - ヒットして `mlx_repo` が非空 → その `mlx_repo`。
pub fn mlx_only_model_id(current: &str) -> String {
    match find_profile(current) {
        None => current.to_string(),
        Some(p) if p.mlx_repo.is_empty() => default_profile().mlx_repo.to_string(),
        Some(p) => p.mlx_repo.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profile_id_matches_default_model_id() {
        assert_eq!(default_profile().id, DEFAULT_MODEL_ID);
    }

    #[test]
    fn test_known_profiles_no_duplicate_ids() {
        let profiles = known_profiles();
        let mut ids: Vec<&str> = profiles.iter().map(|p| p.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "known_profiles() に重複 id がある");
    }

    #[test]
    fn test_find_profile_by_id() {
        let p = find_profile("minicpm5-2b").expect("見つかるはず");
        assert_eq!(p.display_name, "MiniCPM5-2B");
    }

    #[test]
    fn test_find_profile_by_id_case_insensitive() {
        let p = find_profile("MiniCPM5-2B").expect("見つかるはず");
        assert_eq!(p.id, "minicpm5-2b");
    }

    #[test]
    fn test_find_profile_by_gguf_repo() {
        let p = find_profile("openbmb/MiniCPM5-2B-GGUF").expect("見つかるはず");
        assert_eq!(p.id, "minicpm5-2b");
    }

    #[test]
    fn test_find_profile_by_mlx_repo() {
        let p = find_profile("openbmb/MiniCPM5-2B-MLX").expect("見つかるはず");
        assert_eq!(p.id, "minicpm5-2b");
    }

    #[test]
    fn test_find_profile_by_gguf_file_stem() {
        let p = find_profile("MiniCPM5-2B-Q4_K_M").expect("見つかるはず");
        assert_eq!(p.id, "minicpm5-2b");
        // 拡張子つきでも一致する
        let p2 = find_profile("MiniCPM5-2B-Q4_K_M.gguf").expect("見つかるはず");
        assert_eq!(p2.id, "minicpm5-2b");
    }

    #[test]
    fn test_find_profile_unknown_returns_none() {
        assert!(find_profile("totally-unknown-model-xyz").is_none());
    }

    #[test]
    fn test_find_profile_legacy_bonsai_empty_mlx_repo_not_matched_by_empty_input() {
        // bonsai-8b.mlx_repo == "" のため、空文字列 model_id が誤って一致しないこと
        assert!(find_profile("").is_none());
    }

    #[test]
    fn test_resolve_model_id_cli_wins() {
        let resolved = resolve_model_id(
            Some("cli-model"),
            Some("env-bonsai-model"),
            Some("env-unsloth-model"),
            true,
            DEFAULT_MODEL_ID,
        );
        assert_eq!(resolved, "cli-model");
    }

    #[test]
    fn test_resolve_model_id_bonsai_env_wins_over_unsloth_env() {
        let resolved = resolve_model_id(
            None,
            Some("env-bonsai-model"),
            Some("env-unsloth-model"),
            false,
            DEFAULT_MODEL_ID,
        );
        assert_eq!(resolved, "env-bonsai-model");
    }

    #[test]
    fn test_resolve_model_id_unsloth_env_wins_over_default_substitution() {
        let resolved = resolve_model_id(
            None,
            None,
            Some("env-unsloth-model"),
            true,
            DEFAULT_MODEL_ID,
        );
        assert_eq!(resolved, "env-unsloth-model");
    }

    #[test]
    fn test_resolve_model_id_unsloth_backend_default_substitution() {
        let resolved = resolve_model_id(None, None, None, true, DEFAULT_MODEL_ID);
        assert_eq!(resolved, "Aratako/Qwen3-8B-ERP-v0.1-GGUF");
    }

    #[test]
    fn test_resolve_model_id_falls_back_to_config_model_id() {
        let resolved = resolve_model_id(None, None, None, false, "custom-model-id");
        assert_eq!(resolved, "custom-model-id");
    }

    #[test]
    fn test_resolve_model_id_unsloth_backend_non_default_config_kept() {
        // backend==Unsloth でも config の model_id が DEFAULT_MODEL_ID でなければ置換しない
        let resolved = resolve_model_id(None, None, None, true, "custom-model-id");
        assert_eq!(resolved, "custom-model-id");
    }

    // MEDIUM-3: 旧既定 "bonsai-8b" が config に残っている環境 (旧 config.toml 未更新)
    // でも Unsloth backend 切替時の後方互換代替が効くこと。
    #[test]
    fn test_resolve_model_id_legacy_bonsai_8b_unsloth_substitution() {
        let resolved = resolve_model_id(None, None, None, true, "bonsai-8b");
        assert_eq!(resolved, "Aratako/Qwen3-8B-ERP-v0.1-GGUF");
    }

    // LOW-3: 空文字列・空白のみの CLI/env は「未指定」として扱い、次点にフォールスルーする。
    #[test]
    fn test_resolve_model_id_blank_cli_and_env_ignored() {
        let resolved = resolve_model_id(
            Some(""),
            Some("   "),
            Some("env-unsloth-model"),
            false,
            DEFAULT_MODEL_ID,
        );
        assert_eq!(resolved, "env-unsloth-model");
    }

    #[test]
    fn test_resolve_model_id_all_blank_falls_back_to_config() {
        let resolved = resolve_model_id(Some("  "), Some(""), Some(""), false, "custom-model-id");
        assert_eq!(resolved, "custom-model-id");
    }

    // --- L-6: mlx_only_model_id ---

    #[test]
    fn test_mlx_only_model_id_unknown_model_id_kept() {
        assert_eq!(
            mlx_only_model_id("totally-unknown-model-xyz"),
            "totally-unknown-model-xyz"
        );
    }

    #[test]
    fn test_mlx_only_model_id_empty_mlx_repo_falls_back_to_default_profile() {
        // bonsai-8b.mlx_repo == "" のため、既定 profile (minicpm5-2b) の mlx_repo で代替する
        assert_eq!(mlx_only_model_id("bonsai-8b"), default_profile().mlx_repo);
    }

    #[test]
    fn test_mlx_only_model_id_uses_matched_profile_mlx_repo() {
        assert_eq!(
            mlx_only_model_id("ternary-bonsai-8b"),
            "prism-ml/Ternary-Bonsai-8B-mlx-2bit"
        );
    }
}
