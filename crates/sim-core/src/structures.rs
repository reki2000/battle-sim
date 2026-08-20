//! M6 の野戦築城：杭列・堀・鹿砦・土塁・馬防柵などの構造物。
//!
//! 仕様は `docs/spec/07-engineers.md` 2 節、`docs/spec/12-roadmap.md` の M6。
//!
//! 仕様の `Structure.segments: Vec<(Vec2Fx, Vec2Fx)>` は単純化し、1 構造物 =
//! 1 線分にしている。長い防塁が欲しければ工兵タスクを複数出して `Structure` を
//! 並べればよく、実効的な表現力は変わらない。

use sim_math::{dist_sq, fx_from_mm, Brad, Fx, Vec2Fx, FX_ONE};

pub type StructureId = u32;

/// 仕様 07 章 2 節の建造物の種類。パヴィスは弓兵の防御バフであり地形障害では
/// ないため、工兵タスクとしてはまだ配線していない（このモジュールにはデータ
/// だけ用意してある。効果テーブル・完成度の仕組みは他の種類と共通に使える）。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StructureKind {
    /// 杭列。騎兵の忌避判定を極端に厳しくし、それでも突っ込んだ馬に大ダメージ。
    Stakes,
    /// 堀。歩兵は減速して渡れるが、騎兵は進入できない。
    Ditch,
    /// 鹿砦。伐採した木材で組む。騎兵不可。
    Abatis,
    /// 土塁。堀とセットで作ると効率的（この単純化ではまだ材料連携はしない）。
    Rampart,
    /// 馬防柵。杭列の強化版。騎兵を完全に阻止する。
    Palisade,
    /// パヴィス（弩兵用の大盾）。まだ工兵タスクからは作れない（本文参照）。
    Pavise,
}

impl StructureKind {
    /// 完成（`completion_permille` = 1000）時点の歩兵移動倍率（‰、1000 = 無補正）。
    pub fn infantry_move_mult_permille(self) -> u16 {
        use StructureKind::*;
        match self {
            Stakes => 600,   // 減速 -40%
            Ditch => 300,    // 移動 -70%
            Abatis => 400,   // 歩兵 -60%
            Rampart => 800,  // 登坂の遅れ（仕様は数値を明示しないので控えめに設定）
            Palisade => 400, // 乗り越えに時間がかかる
            Pavise => 1000,
        }
    }

    /// 完成時点の隊列維持倍率（‰）。
    pub fn infantry_cohesion_mult_permille(self) -> u16 {
        use StructureKind::*;
        match self {
            Stakes | Rampart => 900,
            Ditch => 400, // 隊列維持 -60%
            Abatis | Palisade => 600,
            Pavise => 1000,
        }
    }

    /// 完成した状態で、騎兵の進入そのものを物理的に塞ぐか。
    ///
    /// 杭列だけは「忌避判定を厳しくしつつ、それでも突っ込めば大ダメージ」という
    /// 半端な壁として扱う（`crate::cavalry` 側の忌避ロジックに任せる）ので、
    /// ここには含めない。
    pub fn hard_blocks_cavalry(self) -> bool {
        matches!(
            self,
            StructureKind::Ditch
                | StructureKind::Abatis
                | StructureKind::Rampart
                | StructureKind::Palisade
        )
    }

    /// 建造に必要な工数（milli-man-seconds / m）。仕様 07 章 2 節の表を
    /// 1000 倍して固定小数点を避けている。パヴィスは「1 枚」を 1 m 換算で扱う。
    pub fn work_per_meter_mms(self) -> u32 {
        use StructureKind::*;
        match self {
            Stakes => 30_000,
            Ditch => 250_000,
            Abatis => 120_000,
            Rampart => 400_000,
            Palisade => 200_000,
            Pavise => 20_000,
        }
    }

    /// 最大耐久。突撃や敵工兵の破壊工作で減る。
    pub fn hp_max(self) -> u16 {
        use StructureKind::*;
        match self {
            Stakes => 40,
            Ditch => 120,
            Abatis => 90,
            Rampart => 200,
            Palisade => 110,
            Pavise => 25,
        }
    }

    /// 構造物の footprint の半幅（mm）。この距離以内にいれば効果を受ける。
    pub fn footprint_half_width_mm(self) -> i32 {
        use StructureKind::*;
        match self {
            Stakes | Palisade => 700,
            Ditch => 1_200,
            Abatis => 1_000,
            Rampart => 1_500,
            Pavise => 600,
        }
    }
}

/// 建造物 1 つ（= 1 線分）。
#[derive(Clone, Copy, Debug)]
pub struct Structure {
    pub id: StructureId,
    pub kind: StructureKind,
    pub a: Vec2Fx,
    pub b: Vec2Fx,
    pub owner: u8,
    pub hp: u16,
    pub hp_max: u16,
    /// 0..=1000。工事の進み具合。効果はこれに比例してスケールする
    /// （仕様「半分掘れた堀は半分の効果」）。
    pub completion_permille: u16,
}

impl Structure {
    #[inline]
    fn is_active(&self) -> bool {
        self.hp > 0 && self.completion_permille > 0
    }

    fn distance_sq_to(&self, p: Vec2Fx) -> i64 {
        dist_sq(p, closest_point_on_segment(p, self.a, self.b))
    }

    fn within_footprint(&self, p: Vec2Fx) -> bool {
        let half_w = fx_from_mm(self.kind.footprint_half_width_mm()) as i64;
        self.distance_sq_to(p) <= half_w * half_w
    }

    /// completion に応じて 1000（無補正）から `full` へ線形補間した倍率。
    fn scaled(&self, full: u16) -> u16 {
        let full = full as i32;
        let c = self.completion_permille as i32;
        (1000 - (1000 - full) * c / 1000).clamp(0, 1000) as u16
    }
}

/// 線分 `a`-`b` 上で `p` に最も近い点。
fn closest_point_on_segment(p: Vec2Fx, a: Vec2Fx, b: Vec2Fx) -> Vec2Fx {
    let ab = b.sub(a);
    let len_sq = ab.len_sq();
    if len_sq == 0 {
        return a;
    }
    let ap = p.sub(a);
    let dot = ap.dot(ab);
    let t = (dot.saturating_mul(FX_ONE as i64) / len_sq).clamp(0, FX_ONE as i64) as Fx;
    a.add(ab.scale(t))
}

/// 完成した障害物が騎兵の進入を塞ぎ始める完成度のしきい値。
const CAVALRY_BLOCK_COMPLETION_THRESHOLD: u16 = 500;
/// 杭列が忌避判定に影響し始める完成度のしきい値（まだ刺さっていない杭は効かない）。
const STAKES_MIN_COMPLETION: u16 = 200;
/// 杭列が完成しているときに忌避チャンスへ足す最大ボーナス（‰）。
const STAKES_REFUSAL_BONUS_FULL_PERMILLE: u32 = 650;

/// 構造物の集合。
#[derive(Default, Debug)]
pub struct StructureSystem {
    pub structures: Vec<Structure>,
}

impl StructureSystem {
    /// 新しい構造物を completion=0 で登録する。工兵タスクが工事を進める。
    pub fn build(&mut self, kind: StructureKind, a: Vec2Fx, b: Vec2Fx, owner: u8) -> StructureId {
        let id = self.structures.len() as StructureId;
        let hp_max = kind.hp_max();
        self.structures.push(Structure {
            id,
            kind,
            a,
            b,
            owner,
            hp: hp_max,
            hp_max,
            completion_permille: 0,
        });
        id
    }

    pub fn get(&self, id: StructureId) -> Option<&Structure> {
        self.structures.get(id as usize)
    }

    fn get_mut(&mut self, id: StructureId) -> Option<&mut Structure> {
        self.structures.get_mut(id as usize)
    }

    /// 工事の進捗を completion に反映する。
    pub fn set_completion(&mut self, id: StructureId, completion_permille: u16) {
        if let Some(s) = self.get_mut(id) {
            s.completion_permille = completion_permille.min(1000);
        }
    }

    /// 構造物に打撃を与える。耐久が尽きれば completion も 0 になり効果を失う
    /// （敵工兵の破壊工作、突撃で杭が折れる、など）。
    pub fn damage(&mut self, id: StructureId, amount: u16) {
        if let Some(s) = self.get_mut(id) {
            s.hp = s.hp.saturating_sub(amount);
            if s.hp == 0 {
                s.completion_permille = 0;
            } else {
                let ratio = (s.hp as u32 * 1000) / s.hp_max.max(1) as u32;
                s.completion_permille = s.completion_permille.min(ratio as u16);
            }
        }
    }

    /// 指定位置での歩兵への効果（移動倍率, 隊列維持倍率）。複数の構造物が
    /// 重なる場合は最も厳しい方を取る。
    pub fn infantry_effect_at(&self, p: Vec2Fx) -> (u16, u16) {
        let mut move_mult = 1000u16;
        let mut cohesion_mult = 1000u16;
        for s in &self.structures {
            if !s.is_active() || !s.within_footprint(p) {
                continue;
            }
            move_mult = move_mult.min(s.scaled(s.kind.infantry_move_mult_permille()));
            cohesion_mult = cohesion_mult.min(s.scaled(s.kind.infantry_cohesion_mult_permille()));
        }
        (move_mult, cohesion_mult)
    }

    /// 完成した堀・鹿砦・土塁・馬防柵が、この位置への騎兵の進入そのものを
    /// 塞いでいるか。
    pub fn blocks_cavalry_at(&self, p: Vec2Fx) -> bool {
        self.structures.iter().any(|s| {
            s.is_active()
                && s.kind.hard_blocks_cavalry()
                && s.completion_permille >= CAVALRY_BLOCK_COMPLETION_THRESHOLD
                && s.within_footprint(p)
        })
    }

    /// `pos` の前方 `half_arc` 以内、距離 `detect_r` 以内にある杭列のうち
    /// 最も近いものを返す（`crate::cavalry` の忌避判定から呼ぶ）。
    pub fn stakes_ahead(
        &self,
        pos: Vec2Fx,
        facing: Brad,
        half_arc: u32,
        detect_r: Fx,
    ) -> Option<StructureId> {
        let mut best: Option<(i64, StructureId)> = None;
        for s in &self.structures {
            if s.kind != StructureKind::Stakes
                || !s.is_active()
                || s.completion_permille < STAKES_MIN_COMPLETION
            {
                continue;
            }
            let cp = closest_point_on_segment(pos, s.a, s.b);
            if !sim_math::within_arc(facing, pos, cp, half_arc) {
                continue;
            }
            let d2 = dist_sq(pos, cp);
            if d2 > (detect_r as i64) * (detect_r as i64) {
                continue;
            }
            if best.map_or(true, |(bd, _)| d2 < bd) {
                best = Some((d2, s.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// 杭列が忌避チャンス（‰）に足す追加分。completion に比例する。
    pub fn stakes_refusal_bonus_permille(&self, id: StructureId) -> u32 {
        self.get(id).map_or(0, |s| {
            (STAKES_REFUSAL_BONUS_FULL_PERMILLE * s.completion_permille as u32) / 1000
        })
    }

    /// 決定論検証用の状態ハッシュ。
    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for s in &self.structures {
            mix(s.kind as u64);
            mix(s.hp as u64);
            mix(s.completion_permille as u64);
            mix(s.owner as u64);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_math::{brad_from_deg, fx};

    #[test]
    fn incomplete_structure_has_no_effect() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Ditch,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(20)),
            0,
        );
        assert_eq!(
            sys.infantry_effect_at(Vec2Fx::new(fx(10), fx(10))),
            (1000, 1000)
        );
        sys.set_completion(id, 1000);
        let (mv, coh) = sys.infantry_effect_at(Vec2Fx::new(fx(10), fx(10)));
        assert_eq!(mv, StructureKind::Ditch.infantry_move_mult_permille());
        assert_eq!(coh, StructureKind::Ditch.infantry_cohesion_mult_permille());
    }

    #[test]
    fn half_built_structure_has_half_the_effect() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Ditch,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(20)),
            0,
        );
        sys.set_completion(id, 500);
        let (mv, _) = sys.infantry_effect_at(Vec2Fx::new(fx(10), fx(10)));
        // Ditch のフル効果は 300‰。半分の完成度なら 1000 と 300 の中間付近。
        assert!((600..700).contains(&mv), "mv={mv}");
    }

    #[test]
    fn completed_ditch_blocks_cavalry_but_not_far_away_points() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Ditch,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(20)),
            0,
        );
        sys.set_completion(id, 1000);
        assert!(sys.blocks_cavalry_at(Vec2Fx::new(fx(10), fx(10))));
        assert!(!sys.blocks_cavalry_at(Vec2Fx::new(fx(50), fx(10))));
    }

    #[test]
    fn stakes_do_not_hard_block_cavalry() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Stakes,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(20)),
            0,
        );
        sys.set_completion(id, 1000);
        assert!(!sys.blocks_cavalry_at(Vec2Fx::new(fx(10), fx(10))));
    }

    #[test]
    fn stakes_ahead_is_found_within_arc_and_range() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Stakes,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(20)),
            0,
        );
        sys.set_completion(id, 1000);
        let pos = Vec2Fx::new(fx(5), fx(10));
        let facing = brad_from_deg(0); // +x
        let found = sys.stakes_ahead(pos, facing, brad_from_deg(45) as u32, fx(10));
        assert_eq!(found, Some(id));
        // 完成度が閾値未満なら見えない
        sys.set_completion(id, 100);
        assert_eq!(
            sys.stakes_ahead(pos, facing, brad_from_deg(45) as u32, fx(10)),
            None
        );
    }

    #[test]
    fn damaging_a_structure_below_zero_hp_kills_its_effect() {
        let mut sys = StructureSystem::default();
        let id = sys.build(
            StructureKind::Stakes,
            Vec2Fx::new(fx(10), fx(0)),
            Vec2Fx::new(fx(10), fx(2)),
            0,
        );
        sys.set_completion(id, 1000);
        let hp_max = sys.get(id).unwrap().hp_max;
        sys.damage(id, hp_max);
        assert_eq!(sys.get(id).unwrap().completion_permille, 0);
        assert_eq!(sys.stakes_refusal_bonus_permille(id), 0);
    }
}
