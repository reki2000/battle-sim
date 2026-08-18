/**
 * 会戦シナリオの地形定義。
 *
 * 生成が JS 側に移ったことで、シナリオの `[terrain]` と地勢の焼き込みは
 * ここが正本になる（軍の編成と初期配置は Rust の `sim-core::scenario` が
 * 持ち続ける）。`data/scenarios/*.toml` は同じ内容を人間向けに書いたもので、
 * 数値を変えるときは両方を直すこと。
 *
 * 属性が直交したので、整形は触りたい軸だけを指定する。「刈り取りの済んだ
 * 耕地」は植生を `FARMLAND` にするだけで、下の土は生成器の決めたままにする。
 */

import { Ground, Vegetation, type TerrainParams } from "./types";
import type { Stamp } from "./shaping";

export interface ScenarioTerrain {
  id: string;
  params: TerrainParams;
  stamps: Stamp[];
}

/** 旧 `Surface` の地表に対応する、よく使う塗りの組み合わせ。 */
const FARMLAND = { ground: Ground.SOIL, vegetation: Vegetation.FARMLAND };
const MUD = { ground: Ground.MUD, vegetation: Vegetation.NONE };
const MARSH = { ground: Ground.MARSH, vegetation: Vegetation.NONE, drain: true };
const MEADOW = { ground: Ground.GRASS, vegetation: Vegetation.MEADOW };
// 森と湿地は「ここはこういう地面だ」という宣言なので、手続き生成が置いた
// 湖があれば消す。会戦場を挟む森が湖で途切れると、地勢そのものが崩れる。
const DENSE_FOREST = { ground: Ground.SOIL, vegetation: Vegetation.DENSE, drain: true };

const AG_SIZE_M = 1200;
const BB_SIZE_M = 1200;
const CR_SIZE_M = 1400;

/**
 * アジンクールの会戦（1415）。
 *
 * 適用順に意味がある。耕地を敷いてから泥濘の帯を重ね、最後に森を置くことで、
 * 森の縁が耕地に侵食されない。
 */
export const AGINCOURT_1415: ScenarioTerrain = {
  id: "agincourt_1415",
  params: {
    seed: 0x4a17_c0ff_1415_0001n,
    sizeM: AG_SIZE_M,
    cellM: 2,
    // 起伏はほぼ無い平野。高低差は下の height_ramp で与える
    relief: 0,
    baseHeightM: 8,
    thermalIterations: 6,
    // 会戦場を横切る川は無い（テルヌワーズ川は地図の外）
    riverDensity: 0,
    roadCount: 1,
    seaEdge: "none",
    seaLevelCm: 0,
    // 会戦場そのものは下の整形で塗り替わる。ここは周囲の林の量
    forestCover: 300,
    // 秋の長雨。会戦場の外も水を含んでいる
    marshBias: 250,
    sandBias: 0,
  },
  stamps: [
    // まず会戦場の水を抜く。手続き生成が窪地に作った湖が両軍の展開地に
    // かかっていると、そこだけ騎兵が動けず、隊列が押し合いで自壊する。
    { kind: "drain", ...FARMLAND, xM: 380, yM: 150, wM: 440, hM: 950 },
    // 会戦場は刈り取りの済んだ耕地。
    { kind: "cover_rect", ...FARMLAND, xM: 380, yM: 150, wM: 440, hM: 950 },
    // 両軍の間と仏軍の展開地は、前夜からの雨と数千の足で捏ねられた泥。
    { kind: "cover_rect", ...MUD, xM: 430, yM: 400, wM: 340, hM: 360 },
    // 西のアジャンクール村の森。南（英軍側）へ行くほど東へ張り出す。
    { kind: "cover_belt", ...DENSE_FOREST, axM: 300, ayM: 1200, bxM: 400, byM: 0, halfWidthM: 130 },
    // 東のトラムクール村の森。2 つ合わせて、会戦場は北の 340 m から
    // 英軍正面の 210 m まで狭まる。
    { kind: "cover_belt", ...DENSE_FOREST, axM: 900, ayM: 1200, bxM: 800, byM: 0, halfWidthM: 130 },
    // 英軍の立つ南側がわずかに高い。仏軍は緩い登りを泥の中で進むことになる。
    { kind: "height_ramp", axis: "y", fromM: 350, toM: 750, fromCm: 500, toCm: 0 },
  ],
};

/**
 * バノックバーンの会戦（1314）。
 *
 * カース（低湿地）を敷き、その左右から小川の湿地で挟み込み、
 * 南のスコットランド側を高くする。
 */
export const BANNOCKBURN_1314: ScenarioTerrain = {
  id: "bannockburn_1314",
  params: {
    seed: 0x8a11_0cb0_1314_0001n,
    sizeM: BB_SIZE_M,
    cellM: 2,
    relief: 0,
    baseHeightM: 8,
    thermalIterations: 6,
    riverDensity: 0,
    roadCount: 1,
    seaEdge: "none",
    seaLevelCm: 0,
    // 周囲はニューパークの森
    forestCover: 350,
    // 6 月とはいえカースは水を含んでいる
    marshBias: 400,
    sandBias: 0,
  },
  stamps: [
    // 会戦場の水を抜く。低湿地そのものは下の湿地で表現するので、
    // ここで消すのは手続き生成が作った湖（騎兵が完全に止まる）。
    { kind: "drain", ...MEADOW, xM: 330, yM: 280, wM: 540, hM: 800 },
    // スコットランド側は森を出た先の牧草地。
    { kind: "cover_rect", ...MEADOW, xM: 330, yM: 300, wM: 540, hM: 360 },
    // イングランド軍が夜を明かしたカース。踏めば沈む軟らかい地面で、
    // 騎兵は速度 4 割・歩兵は疲労 1.9 倍になる。
    //
    // ここを湿地にはしない。湿地は騎兵の速度倍率が 0——つまり馬が 1 mm も
    // 動けない——ので、部隊が固まったまま押し合って圧死する。「動けるが
    // 最悪の足場」は泥が担う。湿地は下の 2 本の小川、すなわち**入ったら
    // 終わりの縁**にだけ使う。
    { kind: "cover_rect", ...MUD, xM: 380, yM: 660, wM: 440, hM: 400 },
    // 西のバノック川。北へ行くほど内側へ食い込み、退路を狭める。
    { kind: "cover_belt", ...MARSH, axM: 330, ayM: 620, bxM: 450, byM: 1100, halfWidthM: 110 },
    // 東のペルストリーム川。
    { kind: "cover_belt", ...MARSH, axM: 870, ayM: 620, bxM: 750, byM: 1100, halfWidthM: 110 },
    // 南（スコットランド側）が高い。降りてくる側にモメンタムが乗る。
    { kind: "height_ramp", axis: "y", fromM: 400, toM: 800, fromCm: 700, toCm: 0 },
  ],
};

/**
 * クレシーの会戦（1346）。
 *
 * 開けた耕地と、南（英軍側）へ登る緩斜面。挟む森は西側だけ（クレシーの森）で、
 * 東は開いたまま——アジンクールとの対比になる。
 */
export const CRECY_1346: ScenarioTerrain = {
  id: "crecy_1346",
  params: {
    seed: 0xc5ec_1346_0000_0001n,
    sizeM: CR_SIZE_M,
    cellM: 2,
    relief: 0,
    baseHeightM: 8,
    thermalIterations: 6,
    riverDensity: 0,
    roadCount: 1,
    seaEdge: "none",
    seaLevelCm: 0,
    forestCover: 250,
    marshBias: 120,
    sandBias: 0,
  },
  stamps: [
    // 会戦場の水を抜いてから耕地を敷く。
    { kind: "drain", ...FARMLAND, xM: 350, yM: 200, wM: 700, hM: 960 },
    { kind: "cover_rect", ...FARMLAND, xM: 350, yM: 200, wM: 700, hM: 960 },
    // 8 月の刈り入れ後。踏み固められてはいるが泥濘ではない。
    { kind: "cover_rect", ...MEADOW, xM: 420, yM: 480, wM: 560, hM: 240 },
    // 西のクレシーの森。英軍の右翼を守る。
    { kind: "cover_belt", ...DENSE_FOREST, axM: 300, ayM: 1400, bxM: 340, byM: 0, halfWidthM: 120 },
    // 斜面。英軍の立つ南が 9 m 高い。登る側は疲れ、下る側の射撃は届く。
    { kind: "height_ramp", axis: "y", fromM: 300, toM: 900, fromCm: 900, toCm: 0 },
  ],
};

/**
 * シナリオ index。
 *
 * **`sim-core::scenario::SCENARIOS` と同じ並びでなければならない。**
 * wasm 境界を越えるのは index だけなので、並びがずれると別の会戦の地形が
 * 別の会戦の軍の下に敷かれる。ワーカーが起動時に id を突き合わせて検査する。
 */
export const SCENARIO_TERRAINS: readonly ScenarioTerrain[] = [
  AGINCOURT_1415,
  CRECY_1346,
  BANNOCKBURN_1314,
];

export function scenarioTerrainById(id: string): ScenarioTerrain | undefined {
  return SCENARIO_TERRAINS.find((s) => s.id === id);
}

export function scenarioTerrainByIndex(index: number): ScenarioTerrain | undefined {
  return SCENARIO_TERRAINS[index];
}
