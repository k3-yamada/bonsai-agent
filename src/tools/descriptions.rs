//! ツール説明テキストの集約管理（OpenCode知見: コードとプロンプトの分離）
//!
//! 各ツールのDESCRIPTIONをここに集約し、Lab変異実験での書き換えを容易にする。
//! 将来的にはTOML/YAML外部ファイルからinclude_str!()で読込可能な構造。

pub const FILE_READ: &str =
    "ファイルの内容を読み取る。pathパラメータにファイルパスを指定。";

pub const FILE_WRITE: &str =
    "ファイルに書き込む。全文置換(content)またはsearch/replace差分適用(old_text/new_text)。git管理下では自動スナップショット。";

pub const MULTI_EDIT: &str =
    "単一ファイルの複数箇所を一括編集する。全て成功するか、失敗時は元に戻す。editsに[{old_text, new_text}]の配列を指定。";

pub const SHELL: &str =
    "シェルコマンドを実行する。commandパラメータにコマンド文字列を指定。";

pub const GIT: &str =
    "Gitリポジトリを操作する。subcommandパラメータにstatus/diff/log/commit/add/branchを指定。commitにはmessageパラメータも必要。";

pub const WEB_FETCH: &str =
    "指定URLの内容を取得する。urlパラメータにURLを指定。";

pub const WEB_SEARCH: &str =
    "Web検索を実行する。queryパラメータに検索クエリを指定。";

pub const REPO_MAP: &str =
    "リポジトリのファイル構造とシンボルマップを生成する。pathパラメータにルートディレクトリを指定。";

pub const ARXIV_SEARCH: &str =
    "arXiv論文を検索する。queryパラメータに検索クエリを指定。";
