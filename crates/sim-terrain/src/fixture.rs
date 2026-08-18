//! 固定された地形バイナリの読み込み。
//!
//! 生成器（`web/src/terrain`）は f64 で動くので、走らせる環境によって結果が
//! 1 cm ずれることがありうる。Rust のテストと `sim-headless` が毎回同じ地形を
//! 見るように、シナリオの地形は `tools/gen_terrain_fixtures.mjs` で 1 度だけ
//! 生成し、`data/terrain/*.bin` に固定してリポジトリへ入れてある。
//!
//! 形式は `web/src/terrain/serialize.ts` のエンコーダと 1 対 1。
//! 並びを変えるときは [`VERSION`] を上げ、両方を直すこと。

use crate::{BattleSiteCandidate, Terrain, TerrainGrids};

/// "BSTR"（little endian の u32）。
const MAGIC: u32 = 0x5254_5342;
/// 対応するバイナリの版。
pub const VERSION: u16 = 1;

/// ヘッダの長さ（バイト）。16 bit グリッドが偶数境界に来るよう詰めてある。
const HEADER_BYTES: usize = 28;
/// 会戦地候補 1 件のバイト数。
const SITE_BYTES: usize = 20;

/// 読み込みに失敗した理由。
#[derive(Debug, PartialEq, Eq)]
pub enum FixtureError {
    /// ヘッダが読めるだけの長さがない
    TooShort,
    /// マジックが違う（地形バイナリではない）
    BadMagic,
    /// 版が違う。生成器と読み手のどちらかが古い
    VersionMismatch { found: u16, expected: u16 },
    /// 宣言された寸法と実際の長さが合わない
    LengthMismatch { found: usize, expected: usize },
}

impl core::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FixtureError::TooShort => write!(f, "地形バイナリが短すぎる"),
            FixtureError::BadMagic => write!(f, "地形バイナリのマジックが違う"),
            FixtureError::VersionMismatch { found, expected } => write!(
                f,
                "地形バイナリの版が違う: {found}（期待は {expected}）。\
                 tools/gen_terrain_fixtures.mjs を回して再生成すること"
            ),
            FixtureError::LengthMismatch { found, expected } => {
                write!(
                    f,
                    "地形バイナリの長さが合わない: {found}（期待は {expected}）"
                )
            }
        }
    }
}

impl std::error::Error for FixtureError {}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]);
        self.pos += 2;
        v
    }

    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }

    fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    fn u64(&mut self) -> u64 {
        let v = u64::from_le_bytes(self.bytes[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        v
    }

    fn i16_grid(&mut self, n: usize) -> Vec<i16> {
        let v = self.bytes[self.pos..self.pos + n * 2]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        self.pos += n * 2;
        v
    }

    fn u16_grid(&mut self, n: usize) -> Vec<u16> {
        let v = self.bytes[self.pos..self.pos + n * 2]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        self.pos += n * 2;
        v
    }

    fn u8_grid(&mut self, n: usize) -> Vec<u8> {
        let v = self.bytes[self.pos..self.pos + n].to_vec();
        self.pos += n;
        v
    }
}

/// バイナリからグリッド一式を復元する。
pub fn decode_grids(bytes: &[u8]) -> Result<TerrainGrids, FixtureError> {
    if bytes.len() < HEADER_BYTES {
        return Err(FixtureError::TooShort);
    }
    let mut r = Reader { bytes, pos: 0 };
    if r.u32() != MAGIC {
        return Err(FixtureError::BadMagic);
    }
    let version = r.u16();
    if version != VERSION {
        return Err(FixtureError::VersionMismatch {
            found: version,
            expected: VERSION,
        });
    }
    let _reserved = r.u16();
    let dim = r.u32();
    let cell_m = r.u32();
    let seed = r.u64();
    let site_count = r.u32() as usize;

    let n = (dim as usize) * (dim as usize);
    let expected = HEADER_BYTES + n * 9 + site_count * SITE_BYTES;
    if bytes.len() != expected {
        return Err(FixtureError::LengthMismatch {
            found: bytes.len(),
            expected,
        });
    }

    let height = r.i16_grid(n);
    let water = r.u16_grid(n);
    let ground = r.u8_grid(n);
    let vegetation = r.u8_grid(n);
    let overlay = r.u8_grid(n);
    let water_kind = r.u8_grid(n);
    let moisture = r.u8_grid(n);

    let mut battle_sites = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        battle_sites.push(BattleSiteCandidate {
            x_m: r.i32(),
            y_m: r.i32(),
            score: r.i32(),
            passable_permille: r.u16(),
            asymmetry_permille: r.u16(),
            openness_permille: r.u16(),
            bottleneck_count: r.u16(),
        });
    }

    Ok(TerrainGrids {
        dim,
        cell_m,
        seed,
        height,
        ground,
        vegetation,
        overlay,
        water,
        water_kind,
        moisture,
        battle_sites,
    })
}

/// バイナリから地形を組み立てる（導出グリッドまで計算する）。
pub fn decode(bytes: &[u8]) -> Result<Terrain, FixtureError> {
    decode_grids(bytes).map(Terrain::from_grids)
}

/// シナリオ ID から固定地形を読む。
///
/// パスは `data/terrain/<id>.bin`。テストと `sim-headless` が使う。
pub fn load_scenario(id: &str) -> Result<Terrain, String> {
    let path = scenario_path(id);
    let bytes = std::fs::read(&path).map_err(|e| {
        format!("{path} を読めない: {e}。tools/gen_terrain_fixtures.mjs で生成すること")
    })?;
    decode(&bytes).map_err(|e| format!("{path}: {e}"))
}

/// フィクスチャのパス。リポジトリのルートからの相対で組み立てる。
///
/// `CARGO_MANIFEST_DIR` は `crates/sim-terrain` を指すので、そこから 2 つ上が
/// リポジトリのルートになる。テストの作業ディレクトリに依らず同じ場所を指す。
pub fn scenario_path(id: &str) -> String {
    format!(
        "{}/../../data/terrain/{}.bin",
        env!("CARGO_MANIFEST_DIR"),
        id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ground, Vegetation};

    #[test]
    fn rejects_a_foreign_blob() {
        assert_eq!(decode_grids(&[0u8; 64]), Err(FixtureError::BadMagic));
        assert_eq!(decode_grids(&[0u8; 4]), Err(FixtureError::TooShort));
    }

    #[test]
    fn loads_every_scenario_fixture() {
        for id in ["agincourt_1415", "bannockburn_1314", "crecy_1346"] {
            let t = load_scenario(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(t.dim >= 500, "{id}: dim が小さすぎる ({})", t.dim);
            assert_eq!(t.cell_m, 2);
            assert_eq!(t.height.len(), (t.dim as usize).pow(2));
            crate::validate(&t).unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    /// シナリオの整形が実際に焼き込まれているか。ここが空振りしていると、
    /// 「森に挟まれた泥濘の耕地」がただの野原になってしまう。
    #[test]
    fn agincourt_fixture_carries_its_shaping() {
        let t = load_scenario("agincourt_1415").unwrap();
        let s = crate::stats(&t);
        assert!(
            s.vegetation_counts[Vegetation::Farmland as usize] > 10_000,
            "耕地が焼き込まれていない"
        );
        assert!(
            s.vegetation_counts[Vegetation::Dense as usize] > 50_000,
            "森の帯が焼き込まれていない"
        );
        assert!(s.ground_counts[Ground::Mud as usize] > 10_000, "泥濘がない");
        // 南（英軍側）が高い斜面。シナリオの座標系では y が小さいほど南
        // （`data/scenarios/agincourt_1415.toml` の冒頭）。
        let mid_x = t.dim / 2;
        let south = t.height_at_cell(mid_x as i32, 40) as i32;
        let north = t.height_at_cell(mid_x as i32, (t.dim - 40) as i32) as i32;
        assert!(
            south > north,
            "英軍側が高くない: 南 {south} cm / 北 {north} cm"
        );
    }

    #[test]
    fn fixtures_are_stable_across_loads() {
        let a = load_scenario("crecy_1346").unwrap();
        let b = load_scenario("crecy_1346").unwrap();
        assert_eq!(a.hash(), b.hash());
    }
}
