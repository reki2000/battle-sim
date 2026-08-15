//! 粗いグリッドでサンプルしてバイリニア補間で細かいグリッドへ拡大する。
//!
//! 地形の大域ノイズ（数百 m 周期）は 2 m セルの解像度で見ればどこも滑らかで、
//! セルごとに新しく評価する意味がほとんどない。5 km マップ（2500² セル）で
//! オクターブを複数持つノイズ関数を毎セル呼ぶと、それだけで生成時間の大半を
//! 食ってしまう（実測: 5 km・デフォルト設定で標高計算だけで 1.3 秒）。
//!
//! 周期が 100 m を超えるような滑らかな成分は、数 m おきの粗いグリッドで
//! サンプルしてバイリニア補間で埋めても見た目はほぼ変わらない。これにより
//! 評価回数を `stride²` 分の 1 に減らせる。

use sim_math::{fx, fx_div, fx_lerp, Fx};

/// 粗いグリッド上で `f` を評価し、`dim × dim` の細かいグリッドへ
/// バイリニア補間で拡大する。
///
/// `stride` は粗いグリッドのセル間隔（fine セル単位）。`f` は fine 座標
/// （粗いグリッド点に対応する整数座標のみ）を受け取る。
pub fn upsample_coarse<F: Fn(i32, i32) -> Fx>(dim: u32, stride: i32, f: F) -> Vec<Fx> {
    let dim_i = dim as i32;
    if dim_i <= 0 {
        return Vec::new();
    }
    let stride = stride.max(1);
    // 端の補間で cx+1, cy+1 が必要になるぶんの余裕を持たせる
    let coarse_dim = (dim_i - 1) / stride + 2;

    let mut coarse = vec![0 as Fx; (coarse_dim * coarse_dim) as usize];
    for cy in 0..coarse_dim {
        for cx in 0..coarse_dim {
            coarse[(cy * coarse_dim + cx) as usize] = f(cx * stride, cy * stride);
        }
    }

    let mut out = vec![0 as Fx; (dim_i * dim_i) as usize];
    let stride_fx = fx(stride);
    for y in 0..dim_i {
        let cy = y / stride;
        let fy_frac = fx_div(fx(y - cy * stride), stride_fx);
        for x in 0..dim_i {
            let cx = x / stride;
            let fx_frac = fx_div(fx(x - cx * stride), stride_fx);

            let i00 = (cy * coarse_dim + cx) as usize;
            let i10 = (cy * coarse_dim + cx + 1) as usize;
            let i01 = ((cy + 1) * coarse_dim + cx) as usize;
            let i11 = ((cy + 1) * coarse_dim + cx + 1) as usize;

            let a = fx_lerp(coarse[i00], coarse[i10], fx_frac);
            let b = fx_lerp(coarse[i01], coarse[i11], fx_frac);
            out[(y * dim_i + x) as usize] = fx_lerp(a, b, fy_frac);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_math::FX_ONE;

    #[test]
    fn exact_at_coarse_grid_points() {
        // stride の倍数の座標では、補間誤差なく f の値そのものが出るはず
        let stride = 4;
        let dim = 20u32;
        let out = upsample_coarse(dim, stride, |x, y| fx(x) + fx(y) * 2);
        for gy in 0..(dim as i32 / stride) {
            for gx in 0..(dim as i32 / stride) {
                let x = gx * stride;
                let y = gy * stride;
                let expected = fx(x) + fx(y) * 2;
                let got = out[(y * dim as i32 + x) as usize];
                assert_eq!(got, expected, "x={x} y={y}");
            }
        }
    }

    #[test]
    fn linear_field_upsamples_exactly() {
        // 線形関数はバイリニア補間で理論上ちょうど再現できる。
        //
        // 実際の用途（ノイズは常に概ね ±FX_ONE に正規化されている）に合わせて、
        // 傾きを「粗いセル 1 個ぶん進んで FX_ONE の何分の1か変化する」程度に
        // 抑える。傾きを大きくすると、分数座標の固定小数点丸め誤差が
        // その傾きに比例して増幅されるため、テストとしての意味が薄れる
        // （実際に最初はこれで誤検出した: 傾き 3072/セルという非現実的に
        // 急な関数を使ったところ、丸め誤差が単独セルで 12 まで増幅された）。
        let stride = 5;
        let dim = 23u32;
        let f = |x: i32, y: i32| fx(x) / 4 - fx(y) / 6 + FX_ONE;
        let out = upsample_coarse(dim, stride, f);
        for y in 0..dim as i32 {
            for x in 0..dim as i32 {
                let expected = f(x, y);
                let got = out[(y * dim as i32 + x) as usize];
                assert!(
                    (got - expected).abs() <= 4,
                    "x={x} y={y} got={got} exp={expected}"
                );
            }
        }
    }

    #[test]
    fn output_size_matches_dim_squared() {
        let dim = 17u32;
        let out = upsample_coarse(dim, 3, |x, y| fx(x + y));
        assert_eq!(out.len(), (dim * dim) as usize);
    }

    #[test]
    fn stride_larger_than_dim_does_not_panic() {
        let dim = 5u32;
        let out = upsample_coarse(dim, 100, |x, y| fx(x + y));
        assert_eq!(out.len(), 25);
    }

    #[test]
    fn is_deterministic() {
        let a = upsample_coarse(50, 4, |x, y| fx(x * y % 97));
        let b = upsample_coarse(50, 4, |x, y| fx(x * y % 97));
        assert_eq!(a, b);
    }
}
