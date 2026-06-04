//! Working memory capacity 7±2 cap (Phase G、Cerememory `cerememory-store-working` 派生)。
//!
//! 由来: Cerememory README §"Five Memory Stores" の `cerememory-store-working`
//! (`Volatile, limited-capacity, high-speed active context cache`) と Miller の
//! 魔法数 7±2 (1956)。bonsai では `Session.messages: Vec<Message>` を hard cap し、
//! 項目 82 ContextOverflowGuard (token-burst-based) の上流で構造的に context 圧迫を防ぐ。
//!
//! ## 設計方針
//! - production default OFF (`BONSAI_WORKING_CAP_ENABLED` env unset で観測動作完全互換)
//! - env=1 で cap 強制、cap = `BONSAI_WORKING_CAP` env (default 9 = 7+2)
//! - **System message は protected** (evict 対象外、agent crash 防止 R1)
//! - **直近 3 件の Tool message は protected** (reasoning context 欠落防止 R2)
//! - eviction 順序: priority 昇順 (User → Assistant → Tool → System) → index 昇順
//! - 戻り値: evicted message count (0 = no-op、legacy 互換シグナル)
//!
//! ## roadmap §5.2 との差分
//! roadmap は `LoopState::max_active_messages` を target とするが、
//! `LoopState` には messages フィールドが存在せず、実際の messages は `Session` 側。
//! 本 module は `Session.messages` を target とする (roadmap 設計修正)。
//!
//! ## Phase 1 Red (TDD strict)
//! `cap_session_messages` は `todo!()` stub。Phase 2 Green で実装。

use crate::domain::conversation::{Message, Role, Session};

/// `BONSAI_WORKING_CAP` env で override されない場合の default cap (Miller 7±2 中央値+2)。
const DEFAULT_CAP: usize = 9;

/// `BONSAI_WORKING_CAP` env での最低値 (System + 1 User + 1 Assistant の最小三項組)。
/// 入力検証: cap < MIN_CAP は session 完全 evict による agent crash を引き起こすため
/// defensive に default に巻き戻す。
const MIN_CAP: usize = 3;

/// `BONSAI_WORKING_CAP_ENABLED=1` (or "true"、case-insensitive) で working memory cap opt-in。
///
/// production default = env unset = false 返却 = legacy 動作 (cap 無効、Session 無制限)。
/// 設計方針: 項目 214 (`BONSAI_ERL_ENABLED`) / 項目 217 (`BONSAI_DECAY_ENABLED`) /
/// 項目 218 (`BONSAI_REVIEW_ENABLED`) と env name 対称。
pub(crate) fn is_working_cap_enabled() -> bool {
    std::env::var("BONSAI_WORKING_CAP_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// `BONSAI_WORKING_CAP` env から cap 値を取得 (default 9、最低 3)。
///
/// 入力検証: 非数値 / cap < MIN_CAP は default に巻き戻す。
pub(crate) fn working_cap_from_env() -> usize {
    std::env::var("BONSAI_WORKING_CAP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= MIN_CAP)
        .unwrap_or(DEFAULT_CAP)
}

/// メッセージ役割の優先度 (大きいほど evict されにくい)。
///
/// - System (100): agent persona / SOUL.md / instructions、絶対 protected
/// - Tool (50): 推論コンテキストとして直近 3 件 protected (本関数では役割優先度のみ)
/// - Assistant (30): 中間 reasoning、必要に応じて summarize 可
/// - User (10): 旧質問は context として古いほど価値が下がる
pub(crate) fn priority_score(msg: &Message) -> u8 {
    match msg.role {
        Role::System => 100,
        Role::Tool => 50,
        Role::Assistant => 30,
        Role::User => 10,
    }
}

/// `Session.messages` の長さを `cap` 以下に削減 (eviction)。
///
/// 戻り値: evict された message 数 (0 = no-op、変更なしを示す)。
///
/// env unset で `is_working_cap_enabled()` が false → **常に 0 返却**で legacy 完全互換。
///
/// eviction policy:
/// 1. System message は絶対 protected
/// 2. 直近 3 件の Tool message は protected (reasoning context 欠落防止)
/// 3. 残り候補から priority 昇順 → index 昇順で evict (lowest priority + oldest 優先)
pub fn cap_session_messages(session: &mut Session, cap: usize) -> usize {
    if !is_working_cap_enabled() {
        return 0;
    }
    if session.messages.len() <= cap {
        return 0;
    }
    let total = session.messages.len();
    let target_evict = total - cap;

    // 直近 3 件の Tool index を集計 (reasoning context 保護)。
    let mut tool_indices: Vec<usize> = session
        .messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| (m.role == Role::Tool).then_some(i))
        .collect();
    tool_indices.reverse();
    let recent_tool_indices: std::collections::HashSet<usize> =
        tool_indices.iter().take(3).copied().collect();

    // evict 候補 = System protected + recent_tool protected を除外
    let mut candidates: Vec<(usize, u8)> = session
        .messages
        .iter()
        .enumerate()
        .filter(|(i, m)| m.role != Role::System && !recent_tool_indices.contains(i))
        .map(|(i, m)| (i, priority_score(m)))
        .collect();

    // priority 昇順 → index 昇順で sort (lowest priority + oldest を優先 evict)
    candidates.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

    let evict_set: std::collections::HashSet<usize> = candidates
        .iter()
        .take(target_evict)
        .map(|(i, _)| *i)
        .collect();
    let evicted = evict_set.len();

    let original = std::mem::take(&mut session.messages);
    session.messages = original
        .into_iter()
        .enumerate()
        .filter_map(|(i, m)| (!evict_set.contains(&i)).then_some(m))
        .collect();

    evicted
}

#[cfg(test)]
mod tests {
    use super::*;

    // env mutation race を避けるため module-local Mutex で serialize する
    // (項目 214 ERL_TEST_LOCK / 項目 217 DECAY_TEST_LOCK / 項目 218 REVIEW_TEST_LOCK と同パターン)。
    static WORKING_CAP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_working_cap_env() {
        unsafe {
            std::env::remove_var("BONSAI_WORKING_CAP_ENABLED");
            std::env::remove_var("BONSAI_WORKING_CAP");
        }
    }

    // ── env toggle (Phase 1 Red) ─────────────────────────────────────────

    #[test]
    fn t_is_working_cap_enabled_default_unset() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        assert!(
            !is_working_cap_enabled(),
            "env unset で false (production default OFF)"
        );
    }

    #[test]
    fn t_is_working_cap_enabled_explicit_1() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP_ENABLED", "1");
        }
        assert!(is_working_cap_enabled(), "env=1 で true");
        for value in ["true", "TRUE", "True"] {
            unsafe {
                std::env::set_var("BONSAI_WORKING_CAP_ENABLED", value);
            }
            assert!(
                is_working_cap_enabled(),
                "env={value} (case-insensitive) で true"
            );
        }
        reset_working_cap_env();
    }

    #[test]
    fn t_working_cap_default_is_9() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        assert_eq!(
            working_cap_from_env(),
            9,
            "env unset で default cap=9 (7+2 Miller)"
        );
    }

    #[test]
    fn t_working_cap_from_env_override() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP", "12");
        }
        assert_eq!(working_cap_from_env(), 12, "env=12 で cap=12");

        // 不正値 (< MIN_CAP=3) は default にフォールバック (defensive bound)
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP", "1");
        }
        assert_eq!(
            working_cap_from_env(),
            9,
            "env=1 (<3) は default 9 に巻き戻し (R1 crash 防止)"
        );

        // 非数値も default
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP", "abc");
        }
        assert_eq!(working_cap_from_env(), 9, "非数値は default 9");
        reset_working_cap_env();
    }

    // ── priority_score (4 cases) ─────────────────────────────────────────

    #[test]
    fn t_priority_score_system_highest() {
        let sys = Message::system("test");
        let user = Message::user("test");
        let asst = Message::assistant("test");
        let tool = Message::tool("test", "tc1");

        assert!(
            priority_score(&sys) > priority_score(&tool),
            "System > Tool"
        );
        assert!(
            priority_score(&tool) > priority_score(&asst),
            "Tool > Assistant"
        );
        assert!(
            priority_score(&asst) > priority_score(&user),
            "Assistant > User"
        );
        assert_eq!(priority_score(&sys), 100);
        assert_eq!(priority_score(&user), 10);
    }

    // ── cap_session_messages 統合 (Phase 1 Red で todo!() panic) ──────

    #[test]
    fn t_cap_session_messages_no_op_when_disabled() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();

        let mut session = Session::new();
        session.add_message(Message::system("sys"));
        session.add_message(Message::user("u1"));
        session.add_message(Message::user("u2"));
        session.add_message(Message::user("u3"));
        session.add_message(Message::user("u4"));
        session.add_message(Message::user("u5"));

        let evicted = cap_session_messages(&mut session, 3);
        assert_eq!(evicted, 0, "env disabled で 0 返却 (legacy 互換)");
        assert_eq!(session.messages.len(), 6, "messages 削減なし");
    }

    #[test]
    fn t_cap_session_messages_evicts_lowest_priority_oldest_first() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP_ENABLED", "1");
        }

        let mut session = Session::new();
        // 6 user message 投入、cap=3 → 3 件 evict 期待 (古い User から)
        for i in 0..6 {
            session.add_message(Message::user(format!("user_{i}")));
        }

        let evicted = cap_session_messages(&mut session, 3);
        reset_working_cap_env();

        assert_eq!(evicted, 3, "6→3 で 3 件 evict");
        assert_eq!(session.messages.len(), 3, "残り 3 件");
        // 残るのは新しい 3 件 (user_3, user_4, user_5)
        assert_eq!(session.messages[0].content, "user_3");
        assert_eq!(session.messages[1].content, "user_4");
        assert_eq!(session.messages[2].content, "user_5");
    }

    #[test]
    fn t_cap_session_messages_protects_system_message() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP_ENABLED", "1");
        }

        let mut session = Session::new();
        session.add_message(Message::system("persona"));
        for i in 0..5 {
            session.add_message(Message::user(format!("user_{i}")));
        }

        // cap=3 で 6→3、System は protected で必ず残る
        let evicted = cap_session_messages(&mut session, 3);
        reset_working_cap_env();

        assert_eq!(evicted, 3, "3 件 evict");
        assert_eq!(session.messages.len(), 3);
        assert!(
            session.messages.iter().any(|m| m.role == Role::System),
            "System message は protected で残る"
        );
        assert_eq!(
            session.messages[0].role,
            Role::System,
            "System は順序維持で先頭"
        );
    }

    #[test]
    fn t_cap_session_messages_protects_recent_tool_results() {
        let _g = WORKING_CAP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_working_cap_env();
        unsafe {
            std::env::set_var("BONSAI_WORKING_CAP_ENABLED", "1");
        }

        let mut session = Session::new();
        // System + 3 User + 4 Tool (合計 8 件)、cap=4 で 4 件 evict 期待
        session.add_message(Message::system("sys"));
        session.add_message(Message::user("user_old1"));
        session.add_message(Message::user("user_old2"));
        session.add_message(Message::user("user_old3"));
        session.add_message(Message::tool("tool_oldest", "t1"));
        session.add_message(Message::tool("tool_recent_3rd", "t2"));
        session.add_message(Message::tool("tool_recent_2nd", "t3"));
        session.add_message(Message::tool("tool_recent_1st", "t4"));

        let evicted = cap_session_messages(&mut session, 4);
        reset_working_cap_env();

        assert_eq!(evicted, 4, "8→4 で 4 件 evict");
        assert_eq!(session.messages.len(), 4);
        // System protected、recent 3 tools (t2/t3/t4) protected → User 3 + tool_oldest を全 evict
        assert!(
            session.messages.iter().any(|m| m.role == Role::System),
            "System protected"
        );
        let tool_ids: Vec<String> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.tool_call_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            tool_ids,
            vec!["t2".to_string(), "t3".to_string(), "t4".to_string()],
            "直近 3 件 Tool は protected (oldest tool t1 のみ evict)"
        );
        assert!(
            !session.messages.iter().any(|m| m.role == Role::User),
            "User は全 evict (lowest priority)"
        );
    }
}
