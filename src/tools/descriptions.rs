//! ツール説明テキストの集約管理（OpenCode知見: コードとプロンプトの分離）
//!
//! 全ツールのDESCRIPTIONをここに集約。Lab変異実験での書き換えはこのファイルのみ。
//! 各ツールは `super::descriptions::CONST` を参照する。

pub const FILE_READ: &str = "ファイルの内容を読み取る。pathパラメータにファイルパスを指定。";

pub const FILE_WRITE: &str = "ファイルに書き込む。全文置換(content)またはsearch/replace差分適用(old_text/new_text)。git管理下では自動スナップショット。";

pub const MULTI_EDIT: &str = "単一ファイルの複数箇所を一括編集する。全て成功するか、失敗時は元に戻す。editsに[{old_text, new_text}]の配列を指定。";

pub const SHELL: &str = "シェルコマンドを実行する。commandパラメータにコマンド文字列を指定。";

pub const GIT: &str = "Gitリポジトリを操作する。subcommandパラメータにstatus/diff/log/commit/add/branchを指定。commitにはmessageパラメータも必要。";

pub const WEB_SEARCH: &str =
    "Webを検索する。queryパラメータに検索クエリを指定。DuckDuckGo Instant Answer APIを使用。";

pub const WEB_FETCH: &str = "URLからWebページのテキスト内容を取得する。urlパラメータにURLを指定。";

pub const REPO_MAP: &str = "コード構造を要約。";

pub const ARXIV_SEARCH: &str = "arxiv論文を検索する。queryパラメータに検索クエリを指定。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_descriptions_non_empty() {
        let all = [
            FILE_READ,
            FILE_WRITE,
            MULTI_EDIT,
            SHELL,
            GIT,
            WEB_SEARCH,
            WEB_FETCH,
            REPO_MAP,
            ARXIV_SEARCH,
        ];
        for desc in &all {
            assert!(!desc.is_empty(), "空の説明文が存在");
        }
    }
}
