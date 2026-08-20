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

/// セルサイズを可変にできる粗い空間ハッシュ。`SpatialHash` と同じ
/// カウントソート方式だが、セル一辺を呼び出し側が選べる。射撃の標的探索
/// （弓の射程に合わせた大きなセル）や、工兵タスクの危険察知・補給拠点探し
/// （数十 m 圏内を見る）のように、`SpatialHash::CELL_M`（2 m）の 3×3 クエリ
/// では届かない距離を見る用途に使う。生存兵全員を毎 tick 索引し直すが、
/// O(n) のカウントソートなので `SpatialHash::rebuild` と同じ計算量に収まる。
#[derive(Debug)]
pub struct CoarseIndex {
    cell_m: i32,
    origin_x: i32,
    origin_y: i32,
    cols: i32,
    rows: i32,
    cell_start: Vec<u32>,
    entries: Vec<u32>,
}

impl CoarseIndex {
    pub fn build(cell_m: i32, soldiers: &Soldiers) -> Self {
        let n = soldiers.len();
        let (mut min_x, mut min_y) = (Fx::MAX, Fx::MAX);
        let (mut max_x, mut max_y) = (Fx::MIN, Fx::MIN);
        let mut any = false;
        for i in 0..n {
            if !soldiers.is_alive(i) {
                continue;
            }
            any = true;
            min_x = min_x.min(soldiers.hot.pos_x[i]);
            min_y = min_y.min(soldiers.hot.pos_y[i]);
            max_x = max_x.max(soldiers.hot.pos_x[i]);
            max_y = max_y.max(soldiers.hot.pos_y[i]);
        }
        if !any {
            return CoarseIndex {
                cell_m,
                origin_x: 0,
                origin_y: 0,
                cols: 0,
                rows: 0,
                cell_start: Vec::new(),
                entries: Vec::new(),
            };
        }
        let origin_x = fx_floor_int(min_x).div_euclid(cell_m) - 1;
        let origin_y = fx_floor_int(min_y).div_euclid(cell_m) - 1;
        let cols = fx_floor_int(max_x).div_euclid(cell_m) + 1 - origin_x + 1;
        let rows = fx_floor_int(max_y).div_euclid(cell_m) + 1 - origin_y + 1;
        let cells = (cols as usize) * (rows as usize);

        let mut counts = vec![0u32; cells];
        let cell_of = |x: Fx, y: Fx| -> usize {
            let cx = (fx_floor_int(x).div_euclid(cell_m) - origin_x).clamp(0, cols - 1);
            let cy = (fx_floor_int(y).div_euclid(cell_m) - origin_y).clamp(0, rows - 1);
            (cy as usize) * (cols as usize) + (cx as usize)
        };
        for i in 0..n {
            if soldiers.is_alive(i) {
                counts[cell_of(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i])] += 1;
            }
        }
        let mut cell_start = vec![0u32; cells + 1];
        let mut acc = 0u32;
        for c in 0..cells {
            cell_start[c] = acc;
            acc += counts[c];
        }
        cell_start[cells] = acc;
        let mut entries = vec![0u32; acc as usize];
        let mut cursor = cell_start.clone();
        for i in 0..n {
            if soldiers.is_alive(i) {
                let c = cell_of(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i]);
                entries[cursor[c] as usize] = i as u32;
                cursor[c] += 1;
            }
        }
        CoarseIndex {
            cell_m,
            origin_x,
            origin_y,
            cols,
            rows,
            cell_start,
            entries,
        }
    }

    /// 周囲 3×3 セルから、`faction` とは異なる陣営の兵士だけを最大
    /// [`MAX_NEIGHBORS`] 件集める。同陣営の候補は上限にカウントせず読み飛ばす
    /// （[`SpatialHash::query_enemies`] と同じ理由。issue #5）。
    pub fn query_excluding_faction(
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
        let cx = (fx_floor_int(x).div_euclid(self.cell_m) - self.origin_x).clamp(0, self.cols - 1);
        let cy = (fx_floor_int(y).div_euclid(self.cell_m) - self.origin_y).clamp(0, self.rows - 1);
        let mut count = 0;
        for dy in -1..=1i32 {
            let ny = cy + dy;
            if ny < 0 || ny >= self.rows {
                continue;
            }
            for dx in -1..=1i32 {
                let nx = cx + dx;
                if nx < 0 || nx >= self.cols {
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

    /// 指定半径を覆うセルから、近いセルにいる敵を最大 [`MAX_NEIGHBORS`] 件集める。
    ///
    /// 3×3 固定の問い合わせでは、セル境界の位置によって 8 m セル越しの
    /// 10〜14 m 先が候補から漏れる。局所兵士 AI の迎撃半径は任務ごとに異なる
    /// ため、必要なセル数まで同心四角形状に広げる。中心セルから外側へ走査し、
    /// 密集時は近いセルだけで上限に達した時点で止めることで計算量も抑える。
    pub fn query_enemies_in_radius(
        &self,
        soldiers: &Soldiers,
        x: Fx,
        y: Fx,
        radius: Fx,
        faction: u8,
        out: &mut [u32; MAX_NEIGHBORS],
    ) -> usize {
        if self.cols == 0 || radius <= 0 {
            return 0;
        }
        let cx = (fx_floor_int(x).div_euclid(self.cell_m) - self.origin_x).clamp(0, self.cols - 1);
        let cy = (fx_floor_int(y).div_euclid(self.cell_m) - self.origin_y).clamp(0, self.rows - 1);
        let cell_size = fx(self.cell_m);
        let cell_radius = (radius + cell_size - 1) / cell_size;
        let radius_sq = (radius as i64) * (radius as i64);
        let here = sim_math::Vec2Fx::new(x, y);
        let mut count = 0usize;

        for ring in 0..=cell_radius {
            for dy in -ring..=ring {
                let ny = cy + dy;
                if ny < 0 || ny >= self.rows {
                    continue;
                }
                for dx in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    let nx = cx + dx;
                    if nx < 0 || nx >= self.cols {
                        continue;
                    }
                    let c = (ny as usize) * (self.cols as usize) + nx as usize;
                    let (start, end) =
                        (self.cell_start[c] as usize, self.cell_start[c + 1] as usize);
                    for &id in &self.entries[start..end] {
                        let i = id as usize;
                        if soldiers.faction[i] == faction
                            || sim_math::dist_sq(here, soldiers.pos(i)) > radius_sq
                        {
                            continue;
                        }
                        out[count] = id;
                        count += 1;
                        if count >= MAX_NEIGHBORS {
                            return count;
                        }
                    }
                }
            }
        }
        count
    }

    /// 周囲 3×3 セルにいる兵士（陣営を問わない）を最大 [`MAX_NEIGHBORS`] 件集める。
    pub fn query_all(&self, x: Fx, y: Fx, out: &mut [u32; MAX_NEIGHBORS]) -> usize {
        if self.cols == 0 {
            return 0;
        }
        let cx = (fx_floor_int(x).div_euclid(self.cell_m) - self.origin_x).clamp(0, self.cols - 1);
        let cy = (fx_floor_int(y).div_euclid(self.cell_m) - self.origin_y).clamp(0, self.rows - 1);
        let mut count = 0;
        for dy in -1..=1i32 {
            let ny = cy + dy;
            if ny < 0 || ny >= self.rows {
                continue;
            }
            for dx in -1..=1i32 {
                let nx = cx + dx;
                if nx < 0 || nx >= self.cols {
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

    #[test]
    fn coarse_radius_query_reaches_beyond_adjacent_cells() {
        let mut s = Soldiers::default();
        s.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), 0);
        let enemy = s.push(fx(112), fx(100), 0, 0, 1, Attrs::default(), 0);
        let index = CoarseIndex::build(8, &s);
        let mut out = [0u32; MAX_NEIGHBORS];
        let n = index.query_enemies_in_radius(&s, fx(100), fx(100), fx(14), 0, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], enemy);
    }
}
