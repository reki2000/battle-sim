//! 死体の障害物フィールド。
//!
//! 倒れた兵士は消えずに戦場へ残り、通行の障害になる（仕様 02 章 4.1 節、
//! 06 章 3.5 節）。ただし死体は**高さ 25 cm 程度の低い障害物**でしかない。
//! 跨げば越えられるので通行を塞ぐことはなく、代わりに越えるのに時間がかかる。
//! 二段三段に重なった場所ほど遅くなるが、時間をかければ必ず通り抜けられる。
//!
//! そのため死体は衝突解決（押し合い）には入れない。押し合いに入れると
//! 「死体の山が壁になる」「死体が押されて動く」という嘘が出るうえ、
//! 近傍クエリの枠（[`crate::spatial::MAX_NEIGHBORS`]）を死体が食い潰して
//! 生者同士の判定が壊れる。代わりに 1 m 格子の密度フィールドとして持ち、
//! 移動速度の倍率とつまずき判定に使う。
//!
//! 格子は死体の bounding box にだけ張り、[`REBUILD_INTERVAL`] ティックごとに
//! 作り直す。死体は動かないので毎 tick 数える必要はない。

use crate::soldiers::{Soldiers, State};
use sim_math::{fx_floor_int, Fx, Vec2Fx};

/// 格子の一辺（m）。跨ぐ単位＝人ひとり分。
pub const CELL_M: i32 = 1;

/// 作り直す間隔（tick）。20 Hz なので 10 tick = 0.5 秒。
pub const REBUILD_INTERVAL: u32 = 10;

/// 死体の高さ（cm）。跨げる高さであることがこのモジュールの前提。
pub const CORPSE_HEIGHT_CM: i32 = 25;

/// 重なりの段数ごとの移動速度倍率（‰）。index が段数、最後の値が上限。
///
/// 1 体なら跨ぐだけ、2〜3 段に重なると足場が崩れて明らかに遅くなる。
/// 0 にはしない——どれだけ積み上がっても、時間をかければ抜けられる。
const MOVE_MULT_PERMILLE: [u16; 5] = [1000, 700, 500, 380, 300];

/// 走っているときに 1 tick あたりつまずく確率（‰）。段数に比例する。
pub const STUMBLE_PERMILLE_PER_STACK: u32 = 8;

/// 死体の密度フィールド。
#[derive(Default, Debug)]
pub struct CorpseField {
    origin_x: i32,
    origin_y: i32,
    cols: i32,
    rows: i32,
    /// セルごとの死体数（飽和加算で 255 まで）
    counts: Vec<u8>,
    /// 最後に作り直したティック
    built_tick: u32,
    /// 数えた死体の総数（デバッグ・テスト用）
    total: u32,
}

impl CorpseField {
    /// 一定間隔で作り直す。間隔外のティックでは何もしない。
    pub fn maybe_rebuild(&mut self, soldiers: &Soldiers, tick: u32) {
        if tick % REBUILD_INTERVAL == 0 {
            self.rebuild(soldiers, tick);
        }
    }

    /// 倒れている兵士（`Downed` / `Dead`）を数え直す。
    pub fn rebuild(&mut self, soldiers: &Soldiers, tick: u32) {
        self.built_tick = tick;
        self.total = 0;
        let n = soldiers.len();

        let (mut min_x, mut min_y) = (Fx::MAX, Fx::MAX);
        let (mut max_x, mut max_y) = (Fx::MIN, Fx::MIN);
        let mut any = false;
        for i in 0..n {
            if !is_corpse(soldiers.hot.state[i]) {
                continue;
            }
            any = true;
            min_x = min_x.min(soldiers.hot.pos_x[i]);
            min_y = min_y.min(soldiers.hot.pos_y[i]);
            max_x = max_x.max(soldiers.hot.pos_x[i]);
            max_y = max_y.max(soldiers.hot.pos_y[i]);
        }
        if !any {
            self.cols = 0;
            self.rows = 0;
            self.counts.clear();
            return;
        }

        self.origin_x = fx_floor_int(min_x).div_euclid(CELL_M);
        self.origin_y = fx_floor_int(min_y).div_euclid(CELL_M);
        self.cols = fx_floor_int(max_x).div_euclid(CELL_M) - self.origin_x + 1;
        self.rows = fx_floor_int(max_y).div_euclid(CELL_M) - self.origin_y + 1;

        let cells = (self.cols as usize) * (self.rows as usize);
        self.counts.clear();
        self.counts.resize(cells, 0);
        for i in 0..n {
            if !is_corpse(soldiers.hot.state[i]) {
                continue;
            }
            if let Some(c) = self.cell_index(soldiers.hot.pos_x[i], soldiers.hot.pos_y[i]) {
                self.counts[c] = self.counts[c].saturating_add(1);
                self.total += 1;
            }
        }
    }

    #[inline]
    fn cell_index(&self, x: Fx, y: Fx) -> Option<usize> {
        if self.cols == 0 || self.rows == 0 {
            return None;
        }
        let cx = fx_floor_int(x).div_euclid(CELL_M) - self.origin_x;
        let cy = fx_floor_int(y).div_euclid(CELL_M) - self.origin_y;
        if cx < 0 || cy < 0 || cx >= self.cols || cy >= self.rows {
            return None;
        }
        Some((cy as usize) * (self.cols as usize) + (cx as usize))
    }

    /// その地点に積み重なっている死体の数。
    #[inline]
    pub fn stack_at(&self, pos: Vec2Fx) -> u8 {
        self.cell_index(pos.x, pos.y).map_or(0, |c| self.counts[c])
    }

    /// その地点を通るときの移動速度倍率（‰）。
    #[inline]
    pub fn move_mult_permille(&self, pos: Vec2Fx) -> u16 {
        let stack = self.stack_at(pos) as usize;
        MOVE_MULT_PERMILLE[stack.min(MOVE_MULT_PERMILLE.len() - 1)]
    }

    /// 数えた死体の総数。
    #[inline]
    pub fn corpse_count(&self) -> u32 {
        self.total
    }

    /// 最後に作り直したティック。
    #[inline]
    pub fn built_tick(&self) -> u32 {
        self.built_tick
    }
}

/// この状態の兵士は地面に倒れていて、通行の障害になる。
#[inline]
pub fn is_corpse(state: State) -> bool {
    matches!(state, State::Downed | State::Dead)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::{fx, fx_from_mm};

    fn corpses_at(points: &[(i32, i32)]) -> Soldiers {
        let mut s = Soldiers::default();
        for &(x, y) in points {
            let id = s.push(fx(x), fx(y), 0, 0, 0, Attrs::default(), 0);
            s.hot.state[id as usize] = State::Dead;
        }
        s
    }

    #[test]
    fn empty_field_has_no_effect() {
        let field = CorpseField::default();
        assert_eq!(field.stack_at(Vec2Fx::new(fx(10), fx(10))), 0);
        assert_eq!(field.move_mult_permille(Vec2Fx::new(fx(10), fx(10))), 1000);
    }

    #[test]
    fn living_soldiers_are_not_obstacles() {
        let mut s = corpses_at(&[(10, 10)]);
        s.hot.state[0] = State::Idle;
        let mut field = CorpseField::default();
        field.rebuild(&s, 0);
        assert_eq!(field.corpse_count(), 0);
        assert_eq!(field.stack_at(Vec2Fx::new(fx(10), fx(10))), 0);
    }

    #[test]
    fn stacked_corpses_slow_more_than_a_single_one() {
        let s = corpses_at(&[(10, 10), (10, 10), (10, 10), (20, 20)]);
        let mut field = CorpseField::default();
        field.rebuild(&s, 0);
        assert_eq!(field.corpse_count(), 4);

        let single = field.move_mult_permille(Vec2Fx::new(fx(20), fx(20)));
        let triple = field.move_mult_permille(Vec2Fx::new(fx(10), fx(10)));
        let clear = field.move_mult_permille(Vec2Fx::new(fx(40), fx(40)));
        assert!(
            triple < single,
            "重なった方が遅くない: {triple} vs {single}"
        );
        assert!(single < clear);
        // どれだけ積み上がっても通れなくはならない
        assert!(triple > 0);
    }

    #[test]
    fn a_corpse_only_blocks_its_own_cell() {
        let s = corpses_at(&[(30, 30)]);
        let mut field = CorpseField::default();
        field.rebuild(&s, 0);
        assert_eq!(field.stack_at(Vec2Fx::new(fx(30), fx(30))), 1);
        // 2 m 離れれば影響はない
        assert_eq!(field.stack_at(Vec2Fx::new(fx(32), fx(30))), 0);
        // 同じ 1 m セルの中なら効く
        assert_eq!(
            field.stack_at(Vec2Fx::new(fx(30) + fx_from_mm(600), fx(30))),
            1
        );
    }

    #[test]
    fn rebuild_only_runs_on_the_interval() {
        let s = corpses_at(&[(10, 10)]);
        let mut field = CorpseField::default();
        field.maybe_rebuild(&s, 3);
        assert_eq!(field.corpse_count(), 0, "間隔外で作り直している");
        field.maybe_rebuild(&s, REBUILD_INTERVAL);
        assert_eq!(field.corpse_count(), 1);
    }
}
