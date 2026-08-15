//! 固定小数点の 2 次元ベクトル。

use crate::angle::{atan2_fx, Brad};
use crate::fixed::{fx_div, fx_mul, isqrt64, Fx};

/// 2 次元ベクトル。単位は m（Fx）。
// `len` はコレクションの要素数ではなくベクトルの長さなので、`is_empty` は不要。
#[allow(clippy::len_without_is_empty)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Vec2Fx {
    pub x: Fx,
    pub y: Fx,
}

impl Vec2Fx {
    pub const ZERO: Vec2Fx = Vec2Fx { x: 0, y: 0 };

    #[inline]
    pub const fn new(x: Fx, y: Fx) -> Self {
        Self { x, y }
    }

    #[inline]
    pub const fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }

    #[inline]
    pub const fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }

    #[inline]
    pub const fn scale(self, k: Fx) -> Self {
        Self::new(fx_mul(self.x, k), fx_mul(self.y, k))
    }

    /// 二乗長。距離の比較はこれで行い、平方根を避ける。
    #[inline]
    pub const fn len_sq(self) -> i64 {
        (self.x as i64) * (self.x as i64) + (self.y as i64) * (self.y as i64)
    }

    /// 長さ。必要な場面でのみ使う。
    #[inline]
    pub fn len(self) -> Fx {
        isqrt64(self.len_sq() as u64) as Fx
    }

    /// 単位ベクトル化。長さ 0 なら [`Vec2Fx::ZERO`]。
    #[inline]
    pub fn normalized(self) -> Self {
        let l = self.len();
        if l == 0 {
            Self::ZERO
        } else {
            Self::new(fx_div(self.x, l), fx_div(self.y, l))
        }
    }

    /// 長さを `max` に制限する。
    #[inline]
    pub fn clamp_len(self, max: Fx) -> Self {
        let l = self.len();
        if l <= max || l == 0 {
            self
        } else {
            self.normalized().scale(max)
        }
    }

    #[inline]
    pub const fn dot(self, o: Self) -> i64 {
        (self.x as i64) * (o.x as i64) + (self.y as i64) * (o.y as i64)
    }

    /// 外積の z 成分。左右どちら側にいるかの判定に使う。
    #[inline]
    pub const fn cross(self, o: Self) -> i64 {
        (self.x as i64) * (o.y as i64) - (self.y as i64) * (o.x as i64)
    }

    /// このベクトルが向いている方向。
    #[inline]
    pub fn angle(self) -> Brad {
        atan2_fx(self.y, self.x)
    }

    /// 線形補間。`t` は 0..=[`FX_ONE`]。
    #[inline]
    pub const fn lerp(self, o: Self, t: Fx) -> Self {
        Self::new(
            self.x + fx_mul(o.x - self.x, t),
            self.y + fx_mul(o.y - self.y, t),
        )
    }
}

/// 2 点間の二乗距離。近傍判定の基本演算。
#[inline]
pub const fn dist_sq(a: Vec2Fx, b: Vec2Fx) -> i64 {
    let dx = (a.x - b.x) as i64;
    let dy = (a.y - b.y) as i64;
    dx * dx + dy * dy
}

/// 2 点間の距離。
#[inline]
pub fn dist(a: Vec2Fx, b: Vec2Fx) -> Fx {
    isqrt64(dist_sq(a, b) as u64) as Fx
}

/// `半径 r` の円同士が重なっているか（二乗で比較）。
#[inline]
pub const fn circles_overlap(a: Vec2Fx, ra: Fx, b: Vec2Fx, rb: Fx) -> bool {
    let sum = (ra + rb) as i64;
    dist_sq(a, b) < sum * sum
}

/// `from` から見て `to` が前方 `half_angle` 以内にあるか。
///
/// 「敵が前方 ±60° 以内なら交戦できる」の判定に使う。
#[inline]
pub fn within_arc(facing: Brad, from: Vec2Fx, to: Vec2Fx, half_angle: u32) -> bool {
    let d = to.sub(from);
    if d.x == 0 && d.y == 0 {
        return true;
    }
    crate::angle::angle_diff(d.angle(), facing).unsigned_abs() <= half_angle
}

impl core::ops::Add for Vec2Fx {
    type Output = Vec2Fx;
    #[inline]
    fn add(self, o: Self) -> Self {
        Vec2Fx::add(self, o)
    }
}

impl core::ops::Sub for Vec2Fx {
    type Output = Vec2Fx;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Vec2Fx::sub(self, o)
    }
}

impl core::ops::Neg for Vec2Fx {
    type Output = Vec2Fx;
    #[inline]
    fn neg(self) -> Self {
        Vec2Fx::new(-self.x, -self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::angle::brad_from_deg;
    use crate::fixed::{fx, FX_ONE};

    #[test]
    fn length_of_triple() {
        assert_eq!(Vec2Fx::new(fx(3), fx(4)).len(), fx(5));
    }

    #[test]
    fn normalized_has_unit_length() {
        let v = Vec2Fx::new(fx(30), fx(-40)).normalized();
        assert!((v.len() - FX_ONE).abs() <= 2, "len={}", v.len());
    }

    #[test]
    fn normalized_zero_is_zero() {
        assert_eq!(Vec2Fx::ZERO.normalized(), Vec2Fx::ZERO);
    }

    #[test]
    fn clamp_len_limits_but_preserves_direction() {
        let v = Vec2Fx::new(fx(30), fx(40)); // 長さ 50
        let c = v.clamp_len(fx(10));
        // normalized() と scale() で 2 回量子化されるので、
        // 誤差は上限長に比例して最大 10/1024 m 程度になる。
        assert!(
            (c.len() - fx(10)).abs() <= 16,
            "len={} (期待 {})",
            c.len(),
            fx(10)
        );
        // 方向が保たれている
        assert!(crate::angle::angle_diff(c.angle(), v.angle()).abs() <= 40);
        // 上限より短いベクトルはそのまま
        assert_eq!(
            Vec2Fx::new(fx(1), 0).clamp_len(fx(10)),
            Vec2Fx::new(fx(1), 0)
        );
    }

    #[test]
    fn overlap_detection() {
        let a = Vec2Fx::new(fx(0), fx(0));
        let b = Vec2Fx::new(crate::fixed::fx_from_mm(600), 0);
        // 半径 0.35 m 同士は 0.6 m 離れていれば重なる（0.7 m 未満）
        assert!(circles_overlap(
            a,
            crate::fixed::fx_from_mm(350),
            b,
            crate::fixed::fx_from_mm(350)
        ));
        let c = Vec2Fx::new(fx(1), 0);
        assert!(!circles_overlap(
            a,
            crate::fixed::fx_from_mm(350),
            c,
            crate::fixed::fx_from_mm(350)
        ));
    }

    #[test]
    fn arc_test_matches_expectation() {
        let me = Vec2Fx::ZERO;
        let facing = brad_from_deg(0); // +x 方向
        let half = 60 * crate::angle::BRAD_FULL / 360;
        assert!(within_arc(facing, me, Vec2Fx::new(fx(5), 0), half));
        assert!(within_arc(facing, me, Vec2Fx::new(fx(5), fx(2)), half));
        assert!(!within_arc(facing, me, Vec2Fx::new(0, fx(5)), half)); // 真横 90°
        assert!(!within_arc(facing, me, Vec2Fx::new(fx(-5), 0), half)); // 真後ろ
    }

    #[test]
    fn cross_sign_tells_which_side() {
        let fwd = Vec2Fx::new(fx(1), 0);
        assert!(fwd.cross(Vec2Fx::new(0, fx(1))) > 0);
        assert!(fwd.cross(Vec2Fx::new(0, fx(-1))) < 0);
    }
}
