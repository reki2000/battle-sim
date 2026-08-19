# Image assets

このディレクトリの PNG はビルド時に生成しない、事前生成済みの地形アセットです。

- `terrain-atlas.png`: 4x4 の地形アトラス。セル番号は `Surface` の並びに対応する。
  0=deep water, 1=shallow water, 2=ford, 3=marsh, 4=mud, 5=grass,
  6=meadow, 7=farmland, 8=scrub, 9=light forest, 10=dense forest,
  11=rock, 12=scree, 13=sand, 14=road, 15=bridge。

人物は全LODでRust/Wasmが生成するポリゴンを使用し、画像アセットは持ちません。
