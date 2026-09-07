//! DMN思考ループの結果型 (DmnOutcome)

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmnOutcome {
    /// 会話中またはタスク処理中のためスキップ (Gate 0)
    SkippedBusy,
    /// 待機時間未消化のためスキップ (Gate 1)
    SkippedNotDue,
    /// 内省を生成したが重要度が閾値未満のため沈黙ログとして記憶にのみ還元 (Gate 2: Silent)
    SilentReflection { insight: String },
    /// 新規性・重要度が高いため、記憶還元に加え発話メッセージを生成 (Gate 2: Spoken)
    SpokenReflection { insight: String, message: String },
}
