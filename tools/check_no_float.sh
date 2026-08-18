#!/usr/bin/env bash
# シミュレーション系クレートに浮動小数点が混入していないか検査する。
#
# 決定論はプラットフォームに依らない整数演算だけで保たれる（仕様 02 章 2.5 節）。
# f32/f64 が 1 つでも入ると、同じシードで違う結果が出うる。
#
# 例外は「描画用スナップショットの書き出し」だけで、これは sim-core の
# snapshot.rs に閉じ込めてある。
set -euo pipefail

CRATES=(sim-math sim-terrain sim-core)
EXEMPT="crates/sim-core/src/snapshot.rs"

fail=0
for c in "${CRATES[@]}"; do
  while IFS= read -r hit; do
    file="${hit%%:*}"
    if [[ "$file" == "$EXEMPT" ]]; then
      continue
    fi
    echo "浮動小数点が混入しています: $hit"
    fail=1
    # 行頭がコメント（`//`・`///`・`//!`）の行は対象外。生成器が f64 で動く
    # ことは仕様として説明する必要があり、その説明自体を検査対象にすると
    # 「浮動小数点の話を書けない」ことになってしまう。
  done < <(grep -rnE '\bf32\b|\bf64\b' --include='*.rs' "crates/$c/src" \
             | grep -vE '^[^:]+:[0-9]+: *//' || true)
done

if [[ $fail -ne 0 ]]; then
  echo
  echo "シミュレーションコアは整数演算のみで書いてください（docs/spec/02-simulation-core.md 2.5 節）。"
  echo "描画用の変換が必要なら crates/sim-core/src/snapshot.rs に置いてください。"
  exit 1
fi

echo "OK: シミュレーションコアに浮動小数点はありません"
