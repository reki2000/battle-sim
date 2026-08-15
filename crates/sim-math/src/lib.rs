//! 決定論的な固定小数点数学。
//!
//! このクレートとその利用者（`sim-core`, `sim-terrain`）は**浮動小数点を一切
//! 使わない**。同じシードから常に同じ結果が出ることを、プラットフォームや
//! スレッド数に依らず保証するため。CI が `tools/check_no_float.sh` で機械的に
//! 検査する。
//!
//! 仕様は `docs/spec/02-simulation-core.md` を参照。

#![forbid(unsafe_code)]

pub mod angle;
pub mod fixed;
pub mod rng;
pub mod vec2;

mod trig_table;

pub use angle::{
    angle_diff, atan2_fx, brad_from_deg, brad_to_deg, cos_fx, rotate, sin_fx, turn_toward,
    unit_vec, Brad, BRAD_FULL, BRAD_HALF, BRAD_QUARTER,
};
pub use fixed::{
    fx, fx_clamp, fx_div, fx_floor_int, fx_from_mm, fx_hypot, fx_lerp, fx_mul, fx_scale_permille,
    fx_smoothstep, fx_to_mm, isqrt64, Fx, FX_HALF, FX_ONE, FX_SHIFT,
};
pub use rng::{splitmix64, Purpose, Rng};
pub use vec2::{circles_overlap, dist, dist_sq, within_arc, Vec2Fx};

/// シミュレーションの刻み。20 Hz = 50 ms。
pub const TICK_HZ: u32 = 20;
/// 1 tick のミリ秒。
pub const TICK_MS: u32 = 1000 / TICK_HZ;

/// 秒をティック数に変換する。
#[inline]
pub const fn secs_to_ticks(secs: u32) -> u32 {
    secs * TICK_HZ
}

/// ミリ秒をティック数に変換する（切り上げ）。
#[inline]
pub const fn ms_to_ticks(ms: u32) -> u32 {
    ms.div_ceil(TICK_MS)
}

/// 「毎秒 X」を「毎ティック X」に変換する（Fx）。
#[inline]
pub const fn per_sec_to_per_tick(v: Fx) -> Fx {
    v / TICK_HZ as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_conversions() {
        assert_eq!(TICK_MS, 50);
        assert_eq!(secs_to_ticks(3), 60);
        assert_eq!(ms_to_ticks(50), 1);
        assert_eq!(ms_to_ticks(51), 2);
        assert_eq!(ms_to_ticks(0), 0);
    }

    #[test]
    fn speed_conversion() {
        // 歩兵 1.4 m/s は 1 tick あたり 70 mm
        let per_tick = per_sec_to_per_tick(fx_from_mm(1400));
        assert!((fx_to_mm(per_tick) - 70).abs() <= 1);
    }
}
