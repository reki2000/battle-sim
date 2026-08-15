//! 固定小数点の value noise と fBm。
//!
//! 浮動小数点を使わないため、格子点のハッシュから整数値を引き、
//! Hermite 補間する方式を採る。

use sim_math::{fx_lerp, fx_mul, fx_smoothstep, splitmix64, Fx, FX_ONE};

/// 格子点の擬似乱数値を -FX_ONE..=FX_ONE で返す。
#[inline]
fn lattice(x: i32, y: i32, seed: u64) -> Fx {
    let h = splitmix64(
        seed ^ (x as i64 as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)),
    );
    // 上位 11 ビットを -1024..=1023 に写す
    ((h >> 53) as i32) - FX_ONE
}

/// value noise。座標は Fx（1.0 = 格子 1 マス）。戻り値は -FX_ONE..=FX_ONE。
pub fn value_noise(x: Fx, y: Fx, seed: u64) -> Fx {
    let xi = x >> sim_math::FX_SHIFT;
    let yi = y >> sim_math::FX_SHIFT;
    let xf = x & (FX_ONE - 1);
    let yf = y & (FX_ONE - 1);

    let u = fx_smoothstep(xf);
    let v = fx_smoothstep(yf);

    let n00 = lattice(xi, yi, seed);
    let n10 = lattice(xi + 1, yi, seed);
    let n01 = lattice(xi, yi + 1, seed);
    let n11 = lattice(xi + 1, yi + 1, seed);

    let a = fx_lerp(n00, n10, u);
    let b = fx_lerp(n01, n11, u);
    fx_lerp(a, b, v)
}

/// オクターブを重ねた fBm。戻り値はおおよそ -FX_ONE..=FX_ONE に正規化される。
pub fn fbm(x: Fx, y: Fx, octaves: u8, seed: u64) -> Fx {
    let mut sum: i64 = 0;
    let mut norm: i64 = 0;
    let mut amp: Fx = FX_ONE;
    let mut fx_x = x;
    let mut fx_y = y;

    for o in 0..octaves {
        sum += fx_mul(value_noise(fx_x, fx_y, seed ^ (o as u64) << 32), amp) as i64;
        norm += amp as i64;
        // lacunarity = 2、gain = 0.5
        fx_x = fx_x.saturating_mul(2);
        fx_y = fx_y.saturating_mul(2);
        amp >>= 1;
        if amp == 0 {
            break;
        }
    }
    if norm == 0 {
        0
    } else {
        // sum も norm も Fx スケール。比を Fx で返すので分子を FX_ONE 倍する。
        ((sum * FX_ONE as i64) / norm) as Fx
    }
}

/// 稜線ノイズ。`1 - |noise|` を重ねて山脈の尾根を作る。
pub fn ridged(x: Fx, y: Fx, octaves: u8, seed: u64) -> Fx {
    let mut sum: i64 = 0;
    let mut norm: i64 = 0;
    let mut amp: Fx = FX_ONE;
    let mut fx_x = x;
    let mut fx_y = y;

    for o in 0..octaves {
        let n = value_noise(fx_x, fx_y, seed ^ (o as u64) << 40);
        let r = FX_ONE - n.abs();
        sum += fx_mul(fx_mul(r, r), amp) as i64;
        norm += amp as i64;
        fx_x = fx_x.saturating_mul(2);
        fx_y = fx_y.saturating_mul(2);
        amp >>= 1;
        if amp == 0 {
            break;
        }
    }
    if norm == 0 {
        0
    } else {
        ((sum * FX_ONE as i64) / norm) as Fx
    }
}

/// ドメインワープした fBm。座標自体をノイズでずらし、等方的でない自然な形を作る。
pub fn warped_fbm(x: Fx, y: Fx, warp: Fx, octaves: u8, seed: u64) -> Fx {
    let wx = x + fx_mul(warp, fbm(x, y, 3, seed ^ 0xA1A1_A1A1));
    let wy = y + fx_mul(warp, fbm(x, y, 3, seed ^ 0xB2B2_B2B2));
    fbm(wx, wy, octaves, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_math::fx;

    #[test]
    fn noise_is_deterministic() {
        let a = value_noise(fx(3) + 512, fx(7) + 128, 42);
        let b = value_noise(fx(3) + 512, fx(7) + 128, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn noise_differs_by_seed() {
        let a = value_noise(fx(3) + 512, fx(7) + 128, 42);
        let b = value_noise(fx(3) + 512, fx(7) + 128, 43);
        assert_ne!(a, b);
    }

    #[test]
    fn noise_stays_in_range() {
        for i in 0..2000 {
            let v = value_noise(i * 37, i * 91, 7);
            assert!((-FX_ONE..=FX_ONE).contains(&v), "v={v}");
        }
    }

    #[test]
    fn noise_is_continuous_across_lattice_lines() {
        // 格子点をまたいで値が跳ばないこと（隣接サンプルの差が小さい）
        let mut max_step = 0;
        let mut prev = value_noise(0, fx(5), 11);
        for i in 1..4096 {
            let v = value_noise(i * 4, fx(5), 11);
            max_step = max_step.max((v - prev).abs());
            prev = v;
        }
        assert!(max_step < FX_ONE / 8, "max_step={max_step}");
    }

    #[test]
    fn fbm_stays_in_range() {
        for i in 0..1000 {
            let v = fbm(i * 53, i * 17, 6, 99);
            assert!((-FX_ONE - 8..=FX_ONE + 8).contains(&v), "v={v}");
        }
    }

    #[test]
    fn ridged_is_non_negative() {
        for i in 0..1000 {
            let v = ridged(i * 53, i * 17, 5, 99);
            assert!(v >= 0, "v={v}");
        }
    }

    #[test]
    fn warped_fbm_is_deterministic() {
        let a = warped_fbm(fx(2), fx(3), FX_ONE, 5, 1234);
        let b = warped_fbm(fx(2), fx(3), FX_ONE, 5, 1234);
        assert_eq!(a, b);
    }
}
