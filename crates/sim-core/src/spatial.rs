//! 空間ハッシュ。
//!
//! 毎 tick カウントソートで再構築する。O(n) で、配列順が入力に対して一意なので
//! 決定論を壊さない（仕様 06 章 1 節、11 章 2.3 節）。
//!
//! グリッドは兵士全体の bounding box にだけ張る。5 km 四方の全域に張ると
//! 25 MB になるが、実際に兵士がいるのは戦場のごく一部なので 2 MB 程度で済む。

use crate::soldiers::Soldiers;
use sim_math::{fx, fx_floor_int, Fx};

/// セルの一辺（m）。近傍クエリは 3×3 セル = 36 m² を見る。
pub const CELL_M: i32 = 2;

/// 1 回のクエリで返す近傍の上限。これが最悪計算量を保証する。
pub const MAX_NEIGHBORS: usize = 12;

#[derive(Default, Debug)]
pub struct SpatialHash {
    /// グリッドの原点（セル座標に対応するワールド座標）
    origin_x: i32,
    origin_y: i32,
    cols: u32,
    rows: u32,
    /// 各セルの `entries` 内での開始位置。長さは cols*rows+1
    cell_start: Vec<u32>,
    /// セル順に並べ替えた兵士 ID
    entries: Vec<u32>,
    /// 再利用するカウント用バッファ
    counts: Vec<u32>,
}

impl SpatialHash {
    /// 生存している兵士から索引を作り直す。
    pub fn rebuild(&mut self, s: &Soldiers) {
        let n = s.len();
        self.entries.clear();
        if n == 0 {
            self.cols = 0;
            self.rows = 0;
            self.cell_start.clear();
            return;
        }

        // 生存者の bounding box を求める
        let (mut min_x, mut min_y) = (Fx::MAX, Fx::MAX);
        let (mut max_x, mut max_y) = (Fx::MIN, Fx::MIN);
        let mut any = false;
        for i in 0..n {
            if !s.is_alive(i) {
                continue;
            }
            any = true;
            min_x = min_x.min(s.hot.pos_x[i]);
            min_y = min_y.min(s.hot.pos_y[i]);
            max_x = max_x.max(s.hot.pos_x[i]);
            max_y = max_y.max(s.hot.pos_y[i]);
        }
        if !any {
            self.cols = 0;
            self.rows = 0;
            self.cell_start.clear();
            return;
        }

        // 1 セル分のマージンを取り、3×3 クエリが範囲外に出ないようにする
        self.origin_x = fx_floor_int(min_x).div_euclid(CELL_M) - 1;
        self.origin_y = fx_floor_int(min_y).div_euclid(CELL_M) - 1;
        let max_cx = fx_floor_int(max_x).div_euclid(CELL_M) + 1;
        let max_cy = fx_floor_int(max_y).div_euclid(CELL_M) + 1;
        self.cols = (max_cx - self.origin_x + 1) as u32;
        self.rows = (max_cy - self.origin_y + 1) as u32;

        let cells = (self.cols as usize) * (self.rows as usize);
        self.counts.clear();
        self.counts.resize(cells, 0);

        // 1 パス目: 各セルの人数を数える
        for i in 0..n {
            if !s.is_alive(i) {
                continue;
            }
            let c = self.cell_index_of(s.hot.pos_x[i], s.hot.pos_y[i]);
            self.counts[c] += 1;
        }

        // 前置和で開始位置を作る
        self.cell_start.clear();
        self.cell_start.resize(cells + 1, 0);
        let mut acc = 0u32;
        for c in 0..cells {
            self.cell_start[c] = acc;
            acc += self.counts[c];
        }
        self.cell_start[cells] = acc;

        // 2 パス目: ID を配置する。ID の昇順で走査するのでセル内の順序も決定的
        self.entries.resize(acc as usize, 0);
        let mut cursor = self.cell_start.clone();
        for i in 0..n {
            if !s.is_alive(i) {
                continue;
            }
            let c = self.cell_index_of(s.hot.pos_x[i], s.hot.pos_y[i]);
            self.entries[cursor[c] as usize] = i as u32;
            cursor[c] += 1;
        }
    }

    #[inline]
    fn cell_coords(&self, x: Fx, y: Fx) -> (i32, i32) {
        let cx = fx_floor_int(x).div_euclid(CELL_M) - self.origin_x;
        let cy = fx_floor_int(y).div_euclid(CELL_M) - self.origin_y;
        (
            cx.clamp(0, self.cols as i32 - 1),
            cy.clamp(0, self.rows as i32 - 1),
        )
    }

    #[inline]
    fn cell_index_of(&self, x: Fx, y: Fx) -> usize {
        let (cx, cy) = self.cell_coords(x, y);
        (cy as usize) * (self.cols as usize) + (cx as usize)
    }

    /// 指定位置の周囲 3×3 セルにいる兵士を `out` に集める。
    ///
    /// 最大 [`MAX_NEIGHBORS`] 件で打ち切る。打ち切りはセル走査の順（決定論的）。
    pub fn query_neighbors(&self, x: Fx, y: Fx, out: &mut [u32; MAX_NEIGHBORS]) -> usize {
        if self.cols == 0 {
            return 0;
        }
        let (cx, cy) = self.cell_coords(x, y);
        let mut count = 0;
        for dy in -1..=1i32 {
            let ny = cy + dy;
            if ny < 0 || ny >= self.rows as i32 {
                continue;
            }
            for dx in -1..=1i32 {
                let nx = cx + dx;
                if nx < 0 || nx >= self.cols as i32 {
                    continue;
                }
                let c = (ny as usize) * (self.cols as usize) + (nx as usize);
                let (start, end) = (self.cell_start[c] as usize, self.cell_start[c + 1] as usize);
                for &id in &self.entries[start..end] {
                    if count >= MAX_NEIGHBORS {
                        return count;
                    }
                    out[count] = id;
                    count += 1;
                }
            }
        }
        count
    }

    /// 指定位置の周囲 3×3 セルから、`faction` とは異なる陣営の兵士だけを
    /// 最大 [`MAX_NEIGHBORS`] 件集める。同陣営の候補は上限にカウントせず
    /// 読み飛ばす。
    ///
    /// [`query_neighbors`](Self::query_neighbors) は陣営を問わず先着順で
    /// 打ち切るため、密集陣形（ファイル間隔 1 m 未満）では自陣の兵士だけで
    /// 12 件の枠が埋まってしまい、1 m 先にいる敵に気づけなくなる
    /// （issue #5）。交戦相手探しのように「敵だけ」を探す用途ではこちらを使う。
    pub fn query_enemies(
        &self,
        soldiers: &Soldiers,
        x: Fx,
        y: Fx,
        faction: u8,
        out: &mut [u32; MAX_NEIGHBORS],
    ) -> usize {
        if self.cols == 0 {
            return 0;
        }
        let (cx, cy) = self.cell_coords(x, y);
        let mut count = 0;
        for dy in -1..=1i32 {
            let ny = cy + dy;
            if ny < 0 || ny >= self.rows as i32 {
                continue;
            }
            for dx in -1..=1i32 {
                let nx = cx + dx;
                if nx < 0 || nx >= self.cols as i32 {
                    continue;
                }
                let c = (ny as usize) * (self.cols as usize) + (nx as usize);
                let (start, end) = (self.cell_start[c] as usize, self.cell_start[c + 1] as usize);
                for &id in &self.entries[start..end] {
                    if soldiers.faction[id as usize] == faction {
                        continue;
                    }
                    if count >= MAX_NEIGHBORS {
                        return count;
                    }
                    out[count] = id;
                    count += 1;
                }
            }
        }
        count
    }

    /// 半径 `r` 以内の兵士を集める（近傍セルからさらに距離で絞る）。
    pub fn query_radius(
        &self,
        s: &Soldiers,
        x: Fx,
        y: Fx,
        r: Fx,
        out: &mut [u32; MAX_NEIGHBORS],
    ) -> usize {
        let mut buf = [0u32; MAX_NEIGHBORS];
        let n = self.query_neighbors(x, y, &mut buf);
        let r2 = (r as i64) * (r as i64);
        let mut count = 0;
        let here = sim_math::Vec2Fx::new(x, y);
        for &id in &buf[..n] {
            if sim_math::dist_sq(here, s.pos(id as usize)) <= r2 {
                out[count] = id;
                count += 1;
            }
        }
        count
    }

    /// 索引に入っている兵士の総数（デバッグ用）。
    pub fn indexed_count(&self) -> usize {
        self.entries.len()
    }

    /// グリッドのセル数（デバッグ用）。
    pub fn cell_count(&self) -> usize {
        (self.cols as usize) * (self.rows as usize)
    }
}

/// セル一辺の Fx 表現。
#[inline]
pub fn cell_size_fx() -> Fx {
    fx(CELL_M)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::{Attrs, Soldiers, State};
    use sim_math::fx;

    fn grid_of(n: i32, spacing: i32) -> Soldiers {
        let mut s = Soldiers::default();
        for y in 0..n {
            for x in 0..n {
                s.push(
                    fx(100 + x * spacing),
                    fx(100 + y * spacing),
                    0,
                    0,
                    0,
                    Attrs::default(),
                    0,
                );
            }
        }
        s
    }

    #[test]
    fn empty_world_is_handled() {
        let mut h = SpatialHash::default();
        h.rebuild(&Soldiers::default());
        let mut out = [0u32; MAX_NEIGHBORS];
        assert_eq!(h.query_neighbors(0, 0, &mut out), 0);
    }

    #[test]
    fn all_living_soldiers_are_indexed() {
        let s = grid_of(20, 1);
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        assert_eq!(h.indexed_count(), 400);
    }

    #[test]
    fn dead_soldiers_are_excluded() {
        let mut s = grid_of(10, 1);
        for i in 0..50 {
            s.hot.state[i] = State::Dead;
        }
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        assert_eq!(h.indexed_count(), 50);
    }

    #[test]
    fn neighbors_are_actually_close() {
        // 1 m 間隔の格子。3×3 セル = 6 m 四方の中にいる者だけが返る
        let s = grid_of(20, 1);
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        let mut out = [0u32; MAX_NEIGHBORS];
        let me = s.pos(210);
        let n = h.query_neighbors(me.x, me.y, &mut out);
        assert!(n > 0);
        for &id in &out[..n] {
            let d = sim_math::dist(me, s.pos(id as usize));
            assert!(d <= fx(9), "近傍が遠すぎる: {d}");
        }
    }

    #[test]
    fn query_is_capped() {
        // 全員を 1 セルに詰め込んでも上限で打ち切られる
        let mut s = Soldiers::default();
        for _ in 0..500 {
            s.push(fx(50), fx(50), 0, 0, 0, Attrs::default(), 0);
        }
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        let mut out = [0u32; MAX_NEIGHBORS];
        assert_eq!(h.query_neighbors(fx(50), fx(50), &mut out), MAX_NEIGHBORS);
    }

    #[test]
    fn rebuild_is_deterministic() {
        let s = grid_of(15, 1);
        let mut a = SpatialHash::default();
        let mut b = SpatialHash::default();
        a.rebuild(&s);
        b.rebuild(&s);
        let mut oa = [0u32; MAX_NEIGHBORS];
        let mut ob = [0u32; MAX_NEIGHBORS];
        for i in 0..s.len() {
            let p = s.pos(i);
            let na = a.query_neighbors(p.x, p.y, &mut oa);
            let nb = b.query_neighbors(p.x, p.y, &mut ob);
            assert_eq!(na, nb);
            assert_eq!(oa, ob, "セル内の順序が一致しない (i={i})");
        }
    }

    #[test]
    fn grid_covers_only_the_active_area() {
        // 5 km 四方のワールドでも、兵士が 100 m 四方に固まっていれば
        // グリッドはその範囲だけを覆う
        let s = grid_of(10, 10); // 100 m 四方
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        // (100/2 + マージン)² 程度。5km/2m = 2500² = 625 万には遠く及ばない
        assert!(
            h.cell_count() < 4000,
            "セル数が多すぎる: {}",
            h.cell_count()
        );
    }

    #[test]
    fn radius_query_filters_by_distance() {
        let s = grid_of(20, 1);
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        let mut out = [0u32; MAX_NEIGHBORS];
        let me = s.pos(210);
        let n = h.query_radius(&s, me.x, me.y, fx(1), &mut out);
        for &id in &out[..n] {
            assert!(sim_math::dist(me, s.pos(id as usize)) <= fx(1));
        }
    }

    #[test]
    fn negative_coordinates_work() {
        let mut s = Soldiers::default();
        for i in 0..20 {
            s.push(fx(-100 + i), fx(-50 - i), 0, 0, 0, Attrs::default(), 0);
        }
        let mut h = SpatialHash::default();
        h.rebuild(&s);
        assert_eq!(h.indexed_count(), 20);
        let mut out = [0u32; MAX_NEIGHBORS];
        let p = s.pos(5);
        assert!(h.query_neighbors(p.x, p.y, &mut out) > 0);
    }
}
