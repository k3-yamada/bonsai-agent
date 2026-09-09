//! R-6: Rust `ModelProfile` registry (`src/domain/model_profile.rs`) と
//! `scripts/model.env` (shell 側 single source of truth の鏡) の drift 検出テスト。
//!
//! 2 方向をチェックする:
//! - forward: `known_profiles()` の各 profile について、`model.env` を
//!   `BONSAI_MODEL_ID=<id>` で source した結果が profile の値と一致すること。
//! - reverse: `model.env` の `case` 節に定義されている preset id が
//!   `known_profiles()` の id 集合と過不足なく一致すること (`*)` フォールバックは除外)。
//!
//! `sh` の spawn 自体が失敗する環境 (CI が macOS/Linux 以外等、`sh` 不在) でのみ panic せず
//! skip する。`sh` は起動できたが script が非ゼロ終了した場合や `scripts/model.env` が読めない
//! 場合は drift 検出漏れを防ぐため panic する (M-1)。

use std::process::Command;

use bonsai_agent::domain::model_profile::known_profiles;

/// `scripts/model.env` を `BONSAI_MODEL_ID=<id>` で source し、比較対象 7 値を
/// `|` 区切りで printf した文字列を返す。`sh` の spawn 自体が失敗した場合のみ `None`
/// (skip 対象)。`sh` は起動できたが script が非ゼロ終了した場合は panic する (M-1)。
fn source_model_env(id: &str) -> Option<String> {
    let script = concat!(
        ". scripts/model.env; ",
        r#"printf "%s|%s|%s|%s|%s|%s|%s" "#,
        r#""$BONSAI_MODEL_GGUF_REPO" "$BONSAI_MODEL_GGUF_FILE" "$BONSAI_MODEL_MLX_REPO" "#,
        r#""$BONSAI_MODEL_CTX" "$BONSAI_MODEL_TEMP" "$BONSAI_MODEL_TOP_P" "#,
        r#""$BONSAI_MODEL_REPEAT_PENALTY""#
    );

    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .env_clear()
        // model.env の `command -v llama-server` 解決や shell 実行に最低限必要な env のみ渡す。
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("BONSAI_MODEL_ID", id)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    match output {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).to_string()),
        // M-1: `sh` は起動できたが script が非ゼロ終了 = model.env 自体の bug の可能性が高い。
        // skip すると drift 検出が握り潰されるため panic する。
        Ok(out) => panic!(
            "scripts/model.env source (id={id}) が非ゼロ終了 ({}): stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(e) => {
            // skip して良いのは `sh` の spawn 自体が失敗する環境 (sh 不在) の場合のみ。
            eprintln!("[skip] `sh` が使えない環境のため model_env_sync テストを skip: {e}");
            None
        }
    }
}

fn parse_f64(field: &str, profile_id: &str, field_name: &str) -> f64 {
    field.parse::<f64>().unwrap_or_else(|e| {
        panic!(
            "profile {profile_id}: model.env の {field_name}={field:?} が f64 として parse できない: {e}"
        )
    })
}

#[test]
fn test_model_env_matches_known_profiles() {
    for p in known_profiles() {
        let Some(raw) = source_model_env(p.id) else {
            // `sh` 不在環境では skip (CI は macOS 想定で通常実行される)。
            return;
        };
        let fields: Vec<&str> = raw.split('|').collect();
        assert_eq!(
            fields.len(),
            7,
            "profile {}: model.env output のフィールド数が想定外 (raw={:?})",
            p.id,
            raw
        );
        let (gguf_repo, gguf_file, mlx_repo, ctx, temp, top_p, repeat_penalty) = (
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
        );

        assert_eq!(
            gguf_repo, p.gguf_repo,
            "profile {}: gguf_repo 不一致 (rust={}, model.env={})",
            p.id, p.gguf_repo, gguf_repo
        );
        assert_eq!(
            gguf_file, p.gguf_file,
            "profile {}: gguf_file 不一致 (rust={}, model.env={})",
            p.id, p.gguf_file, gguf_file
        );
        assert_eq!(
            mlx_repo, p.mlx_repo,
            "profile {}: mlx_repo 不一致 (rust={}, model.env={})",
            p.id, p.mlx_repo, mlx_repo
        );

        let ctx_env: u32 = ctx.parse().unwrap_or_else(|e| {
            panic!(
                "profile {}: model.env の BONSAI_MODEL_CTX={:?} が u32 として parse できない: {}",
                p.id, ctx, e
            )
        });
        assert_eq!(
            ctx_env, p.default_context,
            "profile {}: default_context 不一致 (rust={}, model.env={})",
            p.id, p.default_context, ctx_env
        );

        let temp_env = parse_f64(temp, p.id, "BONSAI_MODEL_TEMP");
        assert!(
            (temp_env - p.inference.temperature).abs() < 1e-9,
            "profile {}: temperature 不一致 (rust={}, model.env={})",
            p.id,
            p.inference.temperature,
            temp_env
        );

        let top_p_env = parse_f64(top_p, p.id, "BONSAI_MODEL_TOP_P");
        assert!(
            (top_p_env - p.inference.top_p).abs() < 1e-9,
            "profile {}: top_p 不一致 (rust={}, model.env={})",
            p.id,
            p.inference.top_p,
            top_p_env
        );

        let repeat_penalty_env = parse_f64(repeat_penalty, p.id, "BONSAI_MODEL_REPEAT_PENALTY");
        assert!(
            (repeat_penalty_env - p.inference.repeat_penalty).abs() < 1e-9,
            "profile {}: repeat_penalty 不一致 (rust={}, model.env={})",
            p.id,
            p.inference.repeat_penalty,
            repeat_penalty_env
        );
    }
}

/// `scripts/model.env` の `case "$BONSAI_MODEL_ID" in` 節から preset id (`*)` フォールバックを
/// 除く) を抽出する。行頭空白 + `id)` の形の行のみを対象にする。
fn model_env_case_ids(content: &str) -> Vec<String> {
    let re_like = |line: &str| -> Option<String> {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_suffix(')')?;
        if rest.is_empty() || rest == "*" {
            return None;
        }
        // L-3: charset を `[A-Za-z0-9._-]` に拡張 (大文字・`.`・`_` を含む preset id にも対応)。
        let valid = rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        valid.then(|| rest.to_string())
    };
    content.lines().filter_map(re_like).collect()
}

#[test]
fn test_model_env_case_ids_match_known_profiles() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/model.env");
    // M-1: `scripts/model.env` が読めない = drift 検出そのものが不可能な致命的状態のため
    // skip ではなく panic する (読めない原因を隠さない)。
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("scripts/model.env ({}) が読めない: {e}", path.display()));

    let mut env_ids = model_env_case_ids(&content);
    env_ids.sort();
    env_ids.dedup();

    let mut profile_ids: Vec<String> = known_profiles().iter().map(|p| p.id.to_string()).collect();
    profile_ids.sort();
    profile_ids.dedup();

    assert_eq!(
        env_ids, profile_ids,
        "scripts/model.env の case 節 id と known_profiles() の id が不一致 \
         (model.env={env_ids:?}, rust={profile_ids:?})"
    );
}
