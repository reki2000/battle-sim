//! 決定論的な擬似乱数。
//!
//! グローバルな RNG 状態を持たない。「誰が」「何の目的で」「いつ」引くかから
//! ストリームを導出する。これによりシステムの実行順序やスレッド数が変わっても
//! 結果が一致する（`docs/spec/02-simulation-core.md` 3 節）。

use crate::fixed::{Fx, FX_ONE};

/// 乱数を引く目的。ストリームの分離に使う。
///
/// 同じ兵士が同じティックに複数の判定をしても、目的が違えば独立になる。
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Spawn = 0,
    HitRoll = 1,
    DamageRoll = 2,
    HitLocation = 3,
    MoraleCheck = 4,
    RallyCheck = 5,
    TargetSelect = 6,
    DecisionNoise = 7,
    PathJitter = 8,
    ArrowSpread = 9,
    HorseRefusal = 10,
    CommanderAssess = 11,
    MessengerRisk = 12,
    WorkVariance = 13,
    Terrain = 14,
    Crush = 15,
    EngineerDanger = 16,
    /// 走り抜けようとして人や死体につまずく判定。
    Stumble = 17,
    /// 徒歩兵が「一旦下がって助走をつける」かどうかの判定。
    ChargeDecision = 18,
}

/// SplitMix64 のミキサ。
#[inline]
pub const fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut x = z;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// 決定論的な乱数ストリーム。
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    /// 生の状態から作る（テストと地形生成用）。
    #[inline]
    pub const fn from_state(state: u64) -> Self {
        Rng(state | 1)
    }

    /// エンティティ・目的・ティックから独立なストリームを導出する。
    ///
    /// 呼び出し順に依存しないので、並列化しても結果が変わらない。
    #[inline]
    pub const fn stream(world_seed: u64, entity: u32, purpose: Purpose, tick: u32) -> Self {
        let mut h = world_seed;
        h = splitmix64(h ^ (entity as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        h = splitmix64(h ^ (purpose as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        h = splitmix64(h ^ (tick as u64).wrapping_mul(0x94D0_49BB_1331_11EB));
        Rng(h | 1)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.0)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// `[lo, hi)` の整数。`hi <= lo` なら `lo`。
    #[inline]
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo) as u64;
        // 乗算による偏りは 2^32 に対して無視できる範囲。決定論は保たれる。
        lo + ((self.next_u32() as u64 * span) >> 32) as i32
    }

    /// `numerator / denom` の確率で真。
    #[inline]
    pub fn chance(&mut self, numerator: u32, denom: u32) -> bool {
        if denom == 0 {
            return false;
        }
        self.range(0, denom as i32) < numerator as i32
    }

    /// 1000 分率の確率で真。データファイルの確率はすべてこの形式。
    #[inline]
    pub fn chance_permille(&mut self, permille: u32) -> bool {
        self.chance(permille, 1000)
    }

    /// 0..=[`FX_ONE`] の一様な Fx。
    #[inline]
    pub fn unit_fx(&mut self) -> Fx {
        (self.next_u32() >> 22) as Fx // 0..=1023
    }

    /// 正規分布の近似（Irwin–Hall、4 個の一様和）。
    ///
    /// 真の正規分布ではないが ±2σ の範囲で十分に近く、裾が有界なので
    /// 能力値の生成に都合がよい（極端な外れ値が出ない）。
    pub fn normal_fx(&mut self, mean: Fx, stddev: Fx) -> Fx {
        let mut sum: i64 = 0;
        for _ in 0..4 {
            sum += self.unit_fx() as i64;
        }
        // 平均 2·FX_ONE、標準偏差 FX_ONE/sqrt(12)·2 = FX_ONE·0.577
        let centered = sum - 2 * FX_ONE as i64;
        mean + ((centered * stddev as i64) / (FX_ONE as i64 * 577 / 1000)) as Fx
    }

    /// 能力値（0..=255）を平均と標準偏差から生成する。
    pub fn attr(&mut self, mean: u8, stddev: u8) -> u8 {
        let v = self.normal_fx(
            crate::fixed::fx(mean as i32),
            crate::fixed::fx(stddev as i32),
        );
        crate::fixed::fx_floor_int(v).clamp(0, 255) as u8
    }

    /// スライスから 1 つ選ぶ。空なら `None`。
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.range(0, items.len() as i32) as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_are_reproducible() {
        let a = Rng::stream(42, 100, Purpose::HitRoll, 7).next_u64();
        let b = Rng::stream(42, 100, Purpose::HitRoll, 7).next_u64();
        assert_eq!(a, b);
    }

    #[test]
    fn streams_are_independent_across_dimensions() {
        let base = Rng::stream(42, 100, Purpose::HitRoll, 7).next_u64();
        assert_ne!(base, Rng::stream(43, 100, Purpose::HitRoll, 7).next_u64());
        assert_ne!(base, Rng::stream(42, 101, Purpose::HitRoll, 7).next_u64());
        assert_ne!(
            base,
            Rng::stream(42, 100, Purpose::MoraleCheck, 7).next_u64()
        );
        assert_ne!(base, Rng::stream(42, 100, Purpose::HitRoll, 8).next_u64());
    }

    #[test]
    fn range_stays_in_bounds() {
        let mut r = Rng::from_state(12345);
        for _ in 0..10_000 {
            let v = r.range(-5, 12);
            assert!((-5..12).contains(&v), "v={v}");
        }
        assert_eq!(r.range(3, 3), 3);
        assert_eq!(r.range(9, 2), 9);
    }

    #[test]
    fn chance_is_roughly_calibrated() {
        let mut r = Rng::from_state(99);
        let mut hits = 0;
        for _ in 0..20_000 {
            if r.chance_permille(250) {
                hits += 1;
            }
        }
        // 25% ± 2%
        assert!((4600..5400).contains(&hits), "hits={hits}");
    }

    #[test]
    fn attr_generation_is_bounded_and_centered() {
        let mut r = Rng::from_state(7);
        let mut sum = 0u32;
        const N: u32 = 5000;
        for _ in 0..N {
            let a = r.attr(175, 22) as u32;
            sum += a;
        }
        let mean = sum / N;
        assert!((168..182).contains(&mean), "mean={mean}");
    }

    #[test]
    fn attr_clamps_at_the_extremes() {
        let mut r = Rng::from_state(3);
        for _ in 0..2000 {
            // 平均 250、標準偏差 60 でも 255 を超えない
            let _ = r.attr(250, 60);
        }
    }

    #[test]
    fn unit_fx_stays_in_range() {
        let mut r = Rng::from_state(1);
        for _ in 0..10_000 {
            let v = r.unit_fx();
            assert!((0..=FX_ONE).contains(&v), "v={v}");
        }
    }

    #[test]
    fn pick_returns_none_for_empty() {
        let mut r = Rng::from_state(1);
        let empty: [u8; 0] = [];
        assert!(r.pick(&empty).is_none());
        assert!(r.pick(&[9u8]).is_some());
    }
}
