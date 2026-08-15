//! 角度（binary radian）と三角関数。
//!
//! 1 回転 = 65536 brad。1 brad ≈ 0.0055°。
//! `u16` なので加減算が自然に周回する（正規化を書き忘れる余地がない）。

use crate::fixed::{fx_div, fx_mul, Fx, FX_ONE};
use crate::trig_table::{QUARTER_ENTRIES, QUARTER_SHIFT, SIN_QUARTER};

/// 角度。1 回転 = 65536。
pub type Brad = u16;

/// 1/4 回転。
pub const BRAD_QUARTER: u32 = 16384;
/// 1/2 回転。
pub const BRAD_HALF: u32 = 32768;
/// 1 回転。
pub const BRAD_FULL: u32 = 65536;

/// 度からの変換（テストとデータ読み込み用）。
#[inline]
pub const fn brad_from_deg(deg: i32) -> Brad {
    let full = BRAD_FULL as i64;
    let mut v = (deg as i64 * full) / 360;
    v %= full;
    if v < 0 {
        v += full;
    }
    v as Brad
}

/// 度への変換（UI 表示用）。
#[inline]
pub const fn brad_to_deg(a: Brad) -> i32 {
    ((a as i64 * 360) / BRAD_FULL as i64) as i32
}

/// 第1象限のテーブルを線形補間して引く。`a` は 0..BRAD_QUARTER。
#[inline]
fn sin_quadrant(a: u32) -> Fx {
    debug_assert!(a <= BRAD_QUARTER);
    let idx = (a >> QUARTER_SHIFT) as usize;
    if idx >= QUARTER_ENTRIES {
        return SIN_QUARTER[QUARTER_ENTRIES] as Fx;
    }
    let lo = SIN_QUARTER[idx] as i32;
    let hi = SIN_QUARTER[idx + 1] as i32;
    let frac = (a & ((1 << QUARTER_SHIFT) - 1)) as i32;
    lo + (((hi - lo) * frac) >> QUARTER_SHIFT)
}

/// sin。戻り値は Fx（[`FX_ONE`] = 1.0）。
#[inline]
pub fn sin_fx(a: Brad) -> Fx {
    let a = a as u32;
    match a / BRAD_QUARTER {
        0 => sin_quadrant(a),
        1 => sin_quadrant(2 * BRAD_QUARTER - a),
        2 => -sin_quadrant(a - 2 * BRAD_QUARTER),
        _ => -sin_quadrant(BRAD_FULL - a),
    }
}

/// cos。
#[inline]
pub fn cos_fx(a: Brad) -> Fx {
    sin_fx(a.wrapping_add(BRAD_QUARTER as u16))
}

/// atan2。整数のみで計算する区分近似。最大誤差は約 0.1°。
///
/// 精度は「兵士がどちらを向くか」「敵が前方 60° 以内か」の判定に十分。
pub fn atan2_fx(y: Fx, x: Fx) -> Brad {
    if x == 0 && y == 0 {
        return 0;
    }
    let ax = x.unsigned_abs() as i64;
    let ay = y.unsigned_abs() as i64;

    // 第1象限に畳んでから、比 r = min/max に対する近似式を使う
    let (num, den) = if ay <= ax { (ay, ax) } else { (ax, ay) };
    let r = ((num << 10) / den) as Fx; // 0..=FX_ONE

    // atan(r) ≈ (π/4)·r − r·(r−1)·(0.2447 + 0.0663·r)   （0 ≤ r ≤ 1）
    // 最大誤差 0.0015 rad ≈ 16 brad。向きの判定には十分。
    const PI_4: Fx = 804; // 0.785398 × FX_ONE
    const C1: Fx = 251; // 0.2447 × FX_ONE
    const C2: Fx = 68; // 0.0663 × FX_ONE
    let atan_unit = fx_mul(PI_4, r) - fx_mul(fx_mul(r, r - FX_ONE), C1 + fx_mul(C2, r));
    // ラジアン → brad: brad = rad × 65536 / (2π) = rad × 10430.38
    let mut ang = ((atan_unit as i64 * 10430) / FX_ONE as i64) as u32;

    if ay > ax {
        ang = BRAD_QUARTER - ang;
    }
    let ang = if x >= 0 && y >= 0 {
        ang
    } else if x < 0 && y >= 0 {
        BRAD_HALF - ang
    } else if x < 0 {
        BRAD_HALF + ang
    } else {
        BRAD_FULL - ang
    };
    (ang % BRAD_FULL) as Brad
}

/// 2 つの角度の差を符号付きで返す（-32768..=32767）。
///
/// 「敵が前方 ±60° 以内か」のような判定に使う。
#[inline]
pub fn angle_diff(a: Brad, b: Brad) -> i32 {
    let d = a.wrapping_sub(b) as i32;
    if d >= BRAD_HALF as i32 {
        d - BRAD_FULL as i32
    } else {
        d
    }
}

/// `from` から `to` へ、1 ステップで最大 `max_step` brad だけ回す。
#[inline]
pub fn turn_toward(from: Brad, to: Brad, max_step: u32) -> Brad {
    let d = angle_diff(to, from);
    let step = d.clamp(-(max_step as i32), max_step as i32);
    from.wrapping_add(step as u16)
}

/// 単位ベクトルを Fx で返す。
#[inline]
pub fn unit_vec(a: Brad) -> (Fx, Fx) {
    (cos_fx(a), sin_fx(a))
}

/// ベクトルを角度だけ回転する。
#[inline]
pub fn rotate(x: Fx, y: Fx, a: Brad) -> (Fx, Fx) {
    let (c, s) = unit_vec(a);
    (fx_mul(x, c) - fx_mul(y, s), fx_mul(x, s) + fx_mul(y, c))
}

/// Fx の比を取り、0 除算を避ける（角度計算の補助）。
#[inline]
pub fn safe_ratio(num: Fx, den: Fx) -> Fx {
    if den == 0 {
        0
    } else {
        fx_div(num, den)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fx の量子化は 1/1024 ≈ 0.001。テーブル補間の誤差を含めて 3 を許容する。
    const TOL: i32 = 3;

    #[test]
    fn sin_cos_at_cardinal_angles() {
        assert!(sin_fx(0).abs() <= TOL);
        assert!((sin_fx(brad_from_deg(90)) - FX_ONE).abs() <= TOL);
        assert!(sin_fx(brad_from_deg(180)).abs() <= TOL);
        assert!((sin_fx(brad_from_deg(270)) + FX_ONE).abs() <= TOL);

        assert!((cos_fx(0) - FX_ONE).abs() <= TOL);
        assert!(cos_fx(brad_from_deg(90)).abs() <= TOL);
        assert!((cos_fx(brad_from_deg(180)) + FX_ONE).abs() <= TOL);
    }

    #[test]
    fn pythagorean_identity_holds_everywhere() {
        // sin² + cos² = 1 が全周で成り立つ
        for a in (0..65536u32).step_by(37) {
            let s = sin_fx(a as Brad);
            let c = cos_fx(a as Brad);
            let sum = fx_mul(s, s) + fx_mul(c, c);
            assert!((sum - FX_ONE).abs() <= 6, "a={a} sum={sum}");
        }
    }

    #[test]
    fn atan2_recovers_the_angle() {
        for a in (0..65536u32).step_by(101) {
            let (c, s) = unit_vec(a as Brad);
            let back = atan2_fx(s, c);
            let d = angle_diff(back, a as Brad).abs();
            // 0.1° = 18 brad 程度の誤差を許容する
            assert!(d <= 40, "a={a} back={back} diff={d}");
        }
    }

    #[test]
    fn atan2_quadrants() {
        assert_eq!(atan2_fx(0, 0), 0);
        assert!(angle_diff(atan2_fx(0, FX_ONE), brad_from_deg(0)).abs() <= 40);
        assert!(angle_diff(atan2_fx(FX_ONE, 0), brad_from_deg(90)).abs() <= 40);
        assert!(angle_diff(atan2_fx(0, -FX_ONE), brad_from_deg(180)).abs() <= 40);
        assert!(angle_diff(atan2_fx(-FX_ONE, 0), brad_from_deg(270)).abs() <= 40);
    }

    #[test]
    fn angle_diff_is_signed_and_wraps() {
        assert_eq!(angle_diff(100, 50), 50);
        assert_eq!(angle_diff(50, 100), -50);
        // 周回をまたぐ差
        assert_eq!(angle_diff(10, 65530), 16);
    }

    #[test]
    fn turn_toward_is_limited_and_converges() {
        let target = brad_from_deg(90);
        let mut a: Brad = 0;
        for _ in 0..100 {
            a = turn_toward(a, target, 512);
        }
        assert_eq!(a, target);
    }

    #[test]
    fn rotate_preserves_length() {
        let (x, y) = (crate::fixed::fx(3), crate::fixed::fx(4));
        for a in (0..65536u32).step_by(521) {
            let (rx, ry) = rotate(x, y, a as Brad);
            let len = crate::fixed::fx_hypot(rx, ry);
            assert!((len - crate::fixed::fx(5)).abs() <= 8, "a={a} len={len}");
        }
    }
}
