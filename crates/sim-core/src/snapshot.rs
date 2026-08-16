//! 描画用スナップショット。
//!
//! wasm のリニアメモリ上に SoA で置き、JS 側は `Uint8Array` のビューとして
//! そのまま読む（コピーなし）。仕様 01 章 3.2 節。
//!
//! 1 体 20 バイト。50,000 体で 1 MB。

use crate::soldiers::Soldiers;

/// 兵士 1 体あたりのバイト数。JS 側と一致させること。
pub const SOLDIER_STRIDE: usize = 20;

/// スナップショットのレイアウトバージョン。JS 側と食い違ったら弾く。
pub const SNAPSHOT_VERSION: u32 = 2;

/// 描画用の SoA バッファ。
///
/// レイアウト（リトルエンディアン、1 体 20 バイト）:
///
/// | オフセット | 型  | 内容 |
/// |---|---|---|
/// | 0  | f32 | x [m] |
/// | 4  | f32 | y [m] |
/// | 8  | i16 | z [cm] |
/// | 10 | u16 | facing [brad] |
/// | 12 | u16 | unit_id |
/// | 14 | u16 | troop_type（視覚プロファイルのindex） |
/// | 16 | u8  | state |
/// | 17 | u8  | flags |
/// | 18 | u8  | faction |
/// | 19 | u8  | padding |
///
/// 位置だけ f32 に変換するのは、描画側が扱いやすいため。
/// シミュレーション状態は固定小数点のまま保たれる。
#[derive(Default, Debug)]
pub struct RenderSnapshot {
    pub bytes: Vec<u8>,
    pub count: u32,
}

impl RenderSnapshot {
    /// 現在の兵士配列から書き出す。バッファは再利用される。
    pub fn write(&mut self, s: &Soldiers) {
        let n = s.len();
        self.bytes.clear();
        self.bytes.resize(n * SOLDIER_STRIDE, 0);
        self.count = n as u32;

        for i in 0..n {
            let o = i * SOLDIER_STRIDE;
            // Fx (1/1024 m) を m の f32 に変換する。
            // これはシミュレーションの外側なので浮動小数点を使ってよい。
            let x = s.hot.pos_x[i] as f32 / sim_math::FX_ONE as f32;
            let y = s.hot.pos_y[i] as f32 / sim_math::FX_ONE as f32;
            self.bytes[o..o + 4].copy_from_slice(&x.to_le_bytes());
            self.bytes[o + 4..o + 8].copy_from_slice(&y.to_le_bytes());
            self.bytes[o + 8..o + 10].copy_from_slice(&s.z_cm[i].to_le_bytes());
            self.bytes[o + 10..o + 12].copy_from_slice(&s.hot.facing[i].to_le_bytes());
            self.bytes[o + 12..o + 14].copy_from_slice(&s.unit_id[i].to_le_bytes());
            self.bytes[o + 14..o + 16].copy_from_slice(&s.troop_type[i].to_le_bytes());
            self.bytes[o + 16] = s.hot.state[i] as u8;
            self.bytes[o + 17] = s.hot.flags[i];
            self.bytes[o + 18] = s.faction[i];
        }
    }

    #[inline]
    pub fn ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    #[inline]
    pub fn byte_len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::{Attrs, State};
    use sim_math::fx;

    #[test]
    fn stride_matches_the_documented_layout() {
        // 4 + 4 + 2 + 2 + 2 + 2 + 1 + 1 + 1 + 1(padding)
        assert_eq!(SOLDIER_STRIDE, 20);
    }

    #[test]
    fn snapshot_round_trips_positions() {
        let mut s = Soldiers::default();
        s.push_typed(fx(123), fx(456), 8192, 3, 2, 7, Attrs::default(), 0);
        s.z_cm[0] = 1234;
        s.hot.state[0] = State::Charging;

        let mut snap = RenderSnapshot::default();
        snap.write(&s);
        assert_eq!(snap.count, 1);
        assert_eq!(snap.byte_len(), 20);

        let b = &snap.bytes;
        let x = f32::from_le_bytes(b[0..4].try_into().unwrap());
        let y = f32::from_le_bytes(b[4..8].try_into().unwrap());
        assert!((x - 123.0).abs() < 0.01);
        assert!((y - 456.0).abs() < 0.01);
        assert_eq!(i16::from_le_bytes(b[8..10].try_into().unwrap()), 1234);
        assert_eq!(u16::from_le_bytes(b[10..12].try_into().unwrap()), 8192);
        assert_eq!(u16::from_le_bytes(b[12..14].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(b[14..16].try_into().unwrap()), 7);
        assert_eq!(b[16], State::Charging as u8);
        assert_eq!(b[18], 2);
    }

    #[test]
    fn buffer_is_reused_across_writes() {
        let mut s = Soldiers::default();
        for _ in 0..100 {
            s.push(fx(1), fx(1), 0, 0, 0, Attrs::default(), 0);
        }
        let mut snap = RenderSnapshot::default();
        snap.write(&s);
        let cap = snap.bytes.capacity();
        snap.write(&s);
        assert_eq!(snap.bytes.capacity(), cap, "毎回確保し直している");
        assert_eq!(snap.count, 100);
    }

    #[test]
    fn empty_snapshot_is_empty() {
        let mut snap = RenderSnapshot::default();
        snap.write(&Soldiers::default());
        assert_eq!(snap.count, 0);
        assert_eq!(snap.byte_len(), 0);
    }
}
