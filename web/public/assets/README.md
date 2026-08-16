# ImageGen assets

このディレクトリの PNG はビルド時に生成しない、事前生成済みのゲームアセットです。

- `terrain-atlas.png`: 4x4 の地形アトラス。セル番号は `Surface` の並びに対応する。
  0=deep water, 1=shallow water, 2=ford, 3=marsh, 4=mud, 5=grass,
  6=meadow, 7=farmland, 8=scrub, 9=light forest, 10=dense forest,
  11=rock, 12=scree, 13=sand, 14=road, 15=bridge。
- `soldier-blue.png`: 青陣営の人物ソース画像。ImageGen 出力に残るチェッカー風背景は
  `generated-assets.ts` が起動時に外周連結領域として透明化する。橙陣営は同じ人物アセットへ
  軽い色味を重ねて表示する。

生成時の主な指定は、地形が「4x4 のトップダウン地形タイルアトラス」、人物が
「14世紀西欧歩兵、青いサーコート、槍と小盾、全身、戦略ゲーム用スプライト」です。
