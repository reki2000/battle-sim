// 固定小数点の sin テーブルを生成する。
// 生成結果は crates/sim-math/src/trig_table.rs にコミットされている。
// シミュレーションコアが実行時に浮動小数点を一切使わないようにするため、
// テーブルはビルド時ではなく事前に生成してリポジトリに含める
// （プラットフォーム間で libm の丸めが揺れる可能性を排除する）。
//
//   node tools/gen_trig.mjs
import { writeFileSync } from "node:fs";

const N = 1024; // 1/4 周期あたりのエントリ数
const SCALE = 1024; // Fx: 1.0 = FX_ONE

const vals = [];
for (let i = 0; i <= N; i++) {
  vals.push(Math.round(Math.sin((Math.PI / 2) * (i / N)) * SCALE));
}

let out = "";
out += "// このファイルは tools/gen_trig.mjs により生成されています。手で編集しないこと。\n";
out += "// sin(x) の第1象限テーブル。1.0 = FX_ONE (1024)。\n";
out += "// index i は角度 i * 16 brad に対応する。\n";
out += "// 端点を含めるため長さは QUARTER_ENTRIES+1。\n\n";
out += `pub const QUARTER_ENTRIES: usize = ${N};\n`;
out += "pub const QUARTER_SHIFT: u32 = 4; // 1 エントリあたり 16 brad\n\n";
out += "pub static SIN_QUARTER: [i16; QUARTER_ENTRIES + 1] = [\n";
for (let i = 0; i < vals.length; i += 16) {
  out += "    " + vals.slice(i, i + 16).map((v) => String(v).padStart(4)).join(", ") + ",\n";
}
out += "];\n";

writeFileSync(new URL("../crates/sim-math/src/trig_table.rs", import.meta.url), out);
console.log(`wrote ${vals.length} entries`);
