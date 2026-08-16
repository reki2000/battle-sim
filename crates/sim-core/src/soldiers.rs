//! 兵士の SoA レイアウト。
//!
//! 仕様は `docs/spec/02-simulation-core.md` 4 節。
//! ホット（毎 tick 触る）とコールド（稀にしか触らない）を別配列に分け、
//! ホットが 1 体 16 バイトに収まるようにする。

use sim_math::{Brad, Fx};

/// 兵士 ID。配列の index そのもの。
pub type SoldierId = u32;
/// 存在しない参照。
pub const NO_ID: u32 = u32::MAX;

/// 兵士の状態。仕様 02 章 4.2 節。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    #[default]
    Idle = 0,
    Marching = 1,
    Repositioning = 2,
    Advancing = 3,
    Charging = 4,
    Engaged = 5,
    Shooting = 6,
    Reloading = 7,
    Wavering = 8,
    Broken = 9,
    Rallying = 10,
    Working = 11,
    Downed = 12,
    Dead = 13,
}

impl State {
    /// この状態の兵士がまだ戦闘に参加しているか。
    #[inline]
    pub fn is_active(self) -> bool {
        !matches!(self, State::Downed | State::Dead)
    }

    /// 交戦中またはそれに準じる状態か（思考頻度の決定に使う）。
    #[inline]
    pub fn is_fighting(self) -> bool {
        matches!(self, State::Engaged | State::Charging)
    }
}

/// フラグビット。
pub mod flags {
    pub const MOUNTED: u8 = 1 << 0;
    pub const COMMANDER: u8 = 1 << 1;
    pub const BANNER_BEARER: u8 = 1 << 2;
    pub const ENGINEER: u8 = 1 << 3;
    pub const MISSILE_TROOP: u8 = 1 << 4;
    /// 静止していて衝突判定から外してよい
    pub const SLEEPING: u8 = 1 << 5;
}

/// 運動能力と性格。生成時に一度だけ決まり、以降ほぼ変わらない。
///
/// 仕様 02 章 4.3 節。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Attrs {
    // 運動能力
    pub speed: u8,
    pub accel: u8,
    pub endurance: u8,
    pub strength: u8,
    pub reflex: u8,
    pub skill: u8,
    // 性格
    pub bravery: u8,
    pub discipline: u8,
    pub aggression: u8,
    pub self_preservation: u8,
    pub loyalty: u8,
    pub composure: u8,
    _pad: [u8; 4],
}

impl Attrs {
    // 12 個の能力値はどれも独立した意味を持ち、まとめる自然な単位がない。
    // 実運用では `Rng::attr` から生成するか `Attrs::default()` を使う。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        speed: u8,
        accel: u8,
        endurance: u8,
        strength: u8,
        reflex: u8,
        skill: u8,
        bravery: u8,
        discipline: u8,
        aggression: u8,
        self_preservation: u8,
        loyalty: u8,
        composure: u8,
    ) -> Self {
        Self {
            speed,
            accel,
            endurance,
            strength,
            reflex,
            skill,
            bravery,
            discipline,
            aggression,
            self_preservation,
            loyalty,
            composure,
            _pad: [0; 4],
        }
    }
}

/// ホットな配列群。移動と衝突で毎 tick 触る。1 体 16 バイト。
#[derive(Default, Debug)]
pub struct Hot {
    pub pos_x: Vec<Fx>,
    pub pos_y: Vec<Fx>,
    pub vel_x: Vec<i16>,
    pub vel_y: Vec<i16>,
    pub facing: Vec<Brad>,
    pub state: Vec<State>,
    pub flags: Vec<u8>,
}

/// 兵士の SoA。
#[derive(Default, Debug)]
pub struct Soldiers {
    pub hot: Hot,

    // ウォーム: 思考で読む
    pub hp: Vec<u16>,
    pub fatigue: Vec<u16>,
    pub morale: Vec<u16>,
    pub unit_id: Vec<u16>,
    pub target: Vec<u32>,
    pub slot: Vec<u16>,
    pub think_at: Vec<u32>,
    pub z_cm: Vec<i16>,

    // コールド: 生成時に決まる
    pub attrs: Vec<Attrs>,
    pub faction: Vec<u8>,
}

/// 最大体力。
pub const MAX_HP: u16 = 100;
/// 最大疲労。
pub const MAX_FATIGUE: u16 = 10_000;
/// 最大士気。
pub const MAX_MORALE: u16 = 1000;

impl Soldiers {
    #[inline]
    pub fn len(&self) -> usize {
        self.hot.pos_x.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn index_if_present(&self, id: SoldierId) -> Option<usize> {
        let i = id as usize;
        (i < self.len()).then_some(i)
    }

    #[inline]
    pub fn is_active_id(&self, id: SoldierId) -> bool {
        self.index_if_present(id)
            .is_some_and(|i| self.hot.state[i].is_active())
    }

    #[inline]
    pub fn pos_checked(&self, id: SoldierId) -> Option<sim_math::Vec2Fx> {
        self.index_if_present(id).map(|i| self.pos(i))
    }

    /// 兵士を 1 体追加し、その ID を返す。
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        pos_x: Fx,
        pos_y: Fx,
        facing: Brad,
        unit_id: u16,
        faction: u8,
        attrs: Attrs,
        flags: u8,
    ) -> SoldierId {
        let id = self.len() as SoldierId;
        self.hot.pos_x.push(pos_x);
        self.hot.pos_y.push(pos_y);
        self.hot.vel_x.push(0);
        self.hot.vel_y.push(0);
        self.hot.facing.push(facing);
        self.hot.state.push(State::Idle);
        self.hot.flags.push(flags);

        self.hp.push(MAX_HP);
        self.fatigue.push(0);
        // 初期士気は勇敢さから（仕様 06 章 5.1、簡略版）
        self.morale
            .push((400 + (attrs.bravery as u16) * 12 / 10).min(MAX_MORALE));
        self.unit_id.push(unit_id);
        self.target.push(NO_ID);
        self.slot.push(0);
        // 思考の位相を ID で散らし、同一ティックへの集中を避ける
        self.think_at.push(id % 20);
        self.z_cm.push(0);

        self.attrs.push(attrs);
        self.faction.push(faction);
        id
    }

    #[inline]
    pub fn pos(&self, i: usize) -> sim_math::Vec2Fx {
        sim_math::Vec2Fx::new(self.hot.pos_x[i], self.hot.pos_y[i])
    }

    #[inline]
    pub fn set_pos(&mut self, i: usize, p: sim_math::Vec2Fx) {
        self.hot.pos_x[i] = p.x;
        self.hot.pos_y[i] = p.y;
    }

    #[inline]
    pub fn is_alive(&self, i: usize) -> bool {
        self.hot.state[i].is_active()
    }

    /// 衝突半径。騎乗しているかで変わる（仕様 06 章 2.2）。
    #[inline]
    pub fn radius(&self, i: usize) -> Fx {
        if self.hot.flags[i] & flags::MOUNTED != 0 {
            sim_math::fx_from_mm(900)
        } else {
            sim_math::fx_from_mm(350)
        }
    }

    /// 押し合いでの質量寄与（kg 相当）。
    #[inline]
    pub fn mass(&self, i: usize) -> i32 {
        let base = if self.hot.flags[i] & flags::MOUNTED != 0 {
            550
        } else {
            75
        };
        base + (self.attrs[i].strength as i32) / 8
    }

    /// 生存者数。
    pub fn alive_count(&self) -> usize {
        self.hot.state.iter().filter(|s| s.is_active()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_math::fx;

    #[test]
    fn hot_row_is_sixteen_bytes() {
        // ホット配列 1 体分の合計サイズ。キャッシュライン 64 B に 4 体乗ること。
        let per_row = core::mem::size_of::<Fx>() * 2      // pos
            + core::mem::size_of::<i16>() * 2             // vel
            + core::mem::size_of::<Brad>()                // facing
            + core::mem::size_of::<State>()               // state
            + core::mem::size_of::<u8>(); // flags
        assert_eq!(per_row, 16, "ホット行が 16 バイトを超えた: {per_row}");
    }

    #[test]
    fn attrs_is_sixteen_bytes() {
        assert_eq!(core::mem::size_of::<Attrs>(), 16);
    }

    #[test]
    fn push_assigns_sequential_ids() {
        let mut s = Soldiers::default();
        let a = s.push(fx(1), fx(2), 0, 0, 0, Attrs::default(), 0);
        let b = s.push(fx(3), fx(4), 0, 0, 0, Attrs::default(), 0);
        assert_eq!((a, b), (0, 1));
        assert_eq!(s.len(), 2);
        assert_eq!(s.pos(1), sim_math::Vec2Fx::new(fx(3), fx(4)));
    }

    #[test]
    fn think_phases_are_spread() {
        let mut s = Soldiers::default();
        for _ in 0..40 {
            s.push(0, 0, 0, 0, 0, Attrs::default(), 0);
        }
        // 位相が 20 種に散っている
        let distinct: std::collections::BTreeSet<_> = s.think_at.iter().collect();
        assert_eq!(distinct.len(), 20);
    }

    #[test]
    fn mounted_soldiers_are_bigger_and_heavier() {
        let mut s = Soldiers::default();
        s.push(0, 0, 0, 0, 0, Attrs::default(), 0);
        s.push(0, 0, 0, 0, 0, Attrs::default(), flags::MOUNTED);
        assert!(s.radius(1) > s.radius(0));
        assert!(s.mass(1) > s.mass(0) * 5);
    }

    #[test]
    fn state_classification() {
        assert!(State::Idle.is_active());
        assert!(!State::Dead.is_active());
        assert!(!State::Downed.is_active());
        assert!(State::Engaged.is_fighting());
        assert!(!State::Marching.is_fighting());
    }

    #[test]
    fn braver_soldiers_start_with_higher_morale() {
        let mut s = Soldiers::default();
        let timid = Attrs::new(0, 0, 0, 0, 0, 0, 40, 0, 0, 0, 0, 0);
        let brave = Attrs::new(0, 0, 0, 0, 0, 0, 200, 0, 0, 0, 0, 0);
        s.push(0, 0, 0, 0, 0, timid, 0);
        s.push(0, 0, 0, 0, 0, brave, 0);
        assert!(s.morale[1] > s.morale[0]);
        assert!(s.morale[1] <= MAX_MORALE);
    }
}
