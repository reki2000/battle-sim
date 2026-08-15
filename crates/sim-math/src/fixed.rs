//! 固定小数点数。
//!
//! 1 単位 = 1/1024 m ≈ 0.977 mm、表現範囲 ±2,097,152 m。
//! 詳細は `docs/spec/02-simulation-core.md` の 2 節を参照。
//!
//! 丸めは常に負の無限大方向（算術シフトの挙動）に統一する。
//! 場所によって丸め方を変えると決定論の検証が難しくなるため。

/// 固定小数点のスカラ。1.0 = [`FX_ONE`]。
pub type Fx = i32;

/// 小数部のビット数。
pub const FX_SHIFT: u32 = 10;

/// 1.0 に相当する値。
pub const FX_ONE: Fx = 1 << FX_SHIFT;

/// 0.5 に相当する値。
pub const FX_HALF: Fx = FX_ONE / 2;

/// 整数から変換する。
#[inline]
pub const fn fx(n: i32) -> Fx {
    n << FX_SHIFT
}

/// ミリメートル単位の整数から変換する。
#[inline]
pub const fn fx_from_mm(mm: i32) -> Fx {
    ((mm as i64 * FX_ONE as i64) / 1000) as Fx
}

/// ミリメートル単位の整数に変換する（0 方向へ切り捨て）。
#[inline]
pub const fn fx_to_mm(v: Fx) -> i32 {
    ((v as i64 * 1000) / FX_ONE as i64) as i32
}

/// 整数部を取り出す（負の無限大方向へ丸め）。
#[inline]
pub const fn fx_floor_int(v: Fx) -> i32 {
    v >> FX_SHIFT
}

/// 乗算。中間結果を i64 に広げて桁溢れを避ける。
#[inline]
pub const fn fx_mul(a: Fx, b: Fx) -> Fx {
    (((a as i64) * (b as i64)) >> FX_SHIFT) as Fx
}

/// 除算。`b == 0` のときは 0 を返す（シミュレーション中にパニックさせない）。
#[inline]
pub const fn fx_div(a: Fx, b: Fx) -> Fx {
    if b == 0 {
        0
    } else {
        (((a as i64) << FX_SHIFT) / (b as i64)) as Fx
    }
}

/// 1000 分率の倍率を適用する。データファイルの倍率はすべてこの形式。
#[inline]
pub const fn fx_scale_permille(v: Fx, permille: i32) -> Fx {
    (((v as i64) * (permille as i64)) / 1000) as Fx
}

/// 値を範囲に収める。
#[inline]
pub const fn fx_clamp(v: Fx, lo: Fx, hi: Fx) -> Fx {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// 線形補間。`t` は 0..=[`FX_ONE`]。
#[inline]
pub const fn fx_lerp(a: Fx, b: Fx, t: Fx) -> Fx {
    a + fx_mul(b - a, t)
}

/// Hermite 補間の重み `3t² - 2t³`。ノイズ生成で使う。
#[inline]
pub const fn fx_smoothstep(t: Fx) -> Fx {
    let t2 = fx_mul(t, t);
    let t3 = fx_mul(t2, t);
    3 * t2 - 2 * t3
}

/// 整数平方根。ビットごとの復元法で、入力に対して一意な結果を返す。
///
/// `isqrt64(n)` は `r*r <= n < (r+1)*(r+1)` を満たす最大の `r`。
pub const fn isqrt64(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut rem: u64 = n;
    let mut root: u64 = 0;
    // 最上位から 2 ビットずつ処理する
    let mut bit: u64 = 1u64 << 62;
    while bit > rem {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// 二乗和の平方根を固定小数点で返す。
///
/// `dx`, `dy` は Fx。中間で i64 に広げるので 5 km 規模でも桁溢れしない。
#[inline]
pub fn fx_hypot(dx: Fx, dy: Fx) -> Fx {
    let d2 = (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64);
    isqrt64(d2 as u64) as Fx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip() {
        assert_eq!(fx(1), FX_ONE);
        assert_eq!(fx(-3), -3 * FX_ONE);
        assert_eq!(fx_floor_int(fx(7)), 7);
        // 1.7 m = 1700 mm。1/1024 m の量子化による誤差は 1 mm 以内。
        let h = fx_from_mm(1700);
        assert!((fx_to_mm(h) - 1700).abs() <= 1);
    }

    #[test]
    fn floor_rounds_toward_negative_infinity() {
        assert_eq!(fx_floor_int(FX_ONE - 1), 0);
        assert_eq!(fx_floor_int(-1), -1);
        assert_eq!(fx_floor_int(-FX_ONE), -1);
    }

    #[test]
    fn mul_div_are_inverse_within_quantization() {
        let a = fx_from_mm(2500);
        let b = fx_from_mm(1750);
        let p = fx_mul(a, b);
        let q = fx_div(p, b);
        assert!((q - a).abs() <= 2, "a={a} q={q}");
    }

    #[test]
    fn div_by_zero_is_zero_not_panic() {
        assert_eq!(fx_div(fx(5), 0), 0);
    }

    #[test]
    fn permille_scaling() {
        // 移動速度倍率 650（森）を 4 m/s に適用すると 2.6 m/s。
        // 1/1024 m の量子化と切り捨てで 1 mm 程度ずれる。
        let v = fx(4);
        assert!((fx_to_mm(fx_scale_permille(v, 650)) - 2600).abs() <= 2);
    }

    #[test]
    fn isqrt_is_exact_on_squares() {
        for n in 0u64..2000 {
            assert_eq!(isqrt64(n * n), n, "n={n}");
        }
    }

    #[test]
    fn isqrt_bounds_hold() {
        for n in [
            1u64,
            2,
            3,
            10,
            999,
            1 << 20,
            (1u64 << 40) + 7,
            u64::MAX >> 2,
        ] {
            let r = isqrt64(n);
            assert!(r * r <= n, "n={n} r={r}");
            if let Some(next_sq) = (r + 1).checked_mul(r + 1) {
                assert!(next_sq > n, "n={n} r={r}");
            }
        }
    }

    #[test]
    fn hypot_matches_pythagorean_triples() {
        // 3-4-5 (m)
        assert_eq!(fx_hypot(fx(3), fx(4)), fx(5));
        // 5 km 規模で桁溢れしない
        let d = fx_hypot(fx(3000), fx(4000));
        assert_eq!(d, fx(5000));
    }

    #[test]
    fn lerp_endpoints() {
        assert_eq!(fx_lerp(fx(2), fx(8), 0), fx(2));
        assert_eq!(fx_lerp(fx(2), fx(8), FX_ONE), fx(8));
        assert_eq!(fx_lerp(fx(2), fx(8), FX_HALF), fx(5));
    }

    #[test]
    fn smoothstep_endpoints() {
        assert_eq!(fx_smoothstep(0), 0);
        assert_eq!(fx_smoothstep(FX_ONE), FX_ONE);
        assert_eq!(fx_smoothstep(FX_HALF), FX_HALF);
    }
}
