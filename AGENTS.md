# Repository agent instructions

## ローカルツールの起動

Rust、Node.js、npm、Wasm、Pythonなどのコマンドを実行する前に、リポジトリ直下の
`TOOLS.md` が存在すれば必ず読むこと。`TOOLS.md` はこの作業環境固有のパス、利用可能な
ランタイム、既知の制約を記録するローカル専用ファイルであり、Gitへ追加・コミットしては
ならない。

`TOOLS.md` が存在しない環境では、`command -v`、各ツールのバージョン表示、
`web/package.json` のscriptsを確認してから、その環境に合う起動方法を選ぶこと。
別環境の絶対パスを推測して使用しない。

## 並行作業

共有ワークツリーでは、既存の変更を利用者または別作業の所有物として扱う。特にM5などの
並行作業中は、明示的な依頼なしにcheckout、rebase、stash、reset、clean、一括整形、
生成物の全面再生成を行わない。独立したGit操作が必要なら別worktreeを使用する。
