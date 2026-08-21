//! 部隊の中の小集団（B3）。
//!
//! 会戦では、兵士は「部隊 500 人」ではなく「隣にいる数人」を見て動く。ここでは
//! Unit の中に 3〜8 人程度の小集団を作り、近い仲間・局所的な先導役を持たせる。
//!
//! 所属は**簡単には変わらない**。損耗のたびに部隊全体を組み直すと、一人の死で
//! 全員の所属と移動先が変わってしまう。小さくなりすぎた集団だけが、近くの集団へ
//! 段階的に合流する。

use sim_math::{dist_sq, Fx, Vec2Fx};

use crate::organization::CommandTree;
use crate::soldiers::{SoldierId, Soldiers, State, NO_ID};

/// 小集団の上限人数。3〜8 人のうち、隊列 1 列ぶんに近い 6 人を基準にする。
pub const MAX_SQUAD_SIZE: usize = 6;
/// これを下回ったら、近くの集団へ合流する。
pub const MIN_SQUAD_SIZE: usize = 3;
/// 合流を受け入れる側の上限。取り残された数人を吸収するぶんだけ膨らんでよい。
pub const MERGE_MAX_SIZE: usize = MAX_SQUAD_SIZE + 2;
/// 所属と先導役を見直す間隔（tick）。20 Hz なので 40 tick = 2 秒。
const REVIEW_INTERVAL_TICKS: u32 = 40;
/// 合流先を探す距離（m）。これより遠い集団とは合流しない。
const MERGE_RADIUS_M: i32 = 12;
/// 所属していないことを表す番兵。
pub const NO_SQUAD: u16 = u16::MAX;

/// 小集団 1 つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Squad {
    /// 属する指揮ノード（葉部隊）。
    pub node: u32,
    /// 生きている構成員。
    pub members: Vec<SoldierId>,
    /// 局所的な先導役。規律・勇敢さ・忠誠から決まり、倒れたら次の者へ移る。
    pub leader: SoldierId,
    /// 構成員の重心。孤立の判定に使う。
    pub centroid: Vec2Fx,
    /// 交戦中・迎撃中の構成員の人数。
    pub engaged: u16,
}

/// 兵士 → 小集団の対応と、小集団ごとの状態。
#[derive(Debug, Default)]
pub struct SquadSystem {
    squad_of: Vec<u16>,
    squads: Vec<Squad>,
    next_review: u32,
}

impl SquadSystem {
    pub fn register(&mut self) {
        self.squad_of.push(NO_SQUAD);
    }

    fn ensure_len(&mut self, len: usize) {
        self.squad_of.resize(len, NO_SQUAD);
    }

    /// 空になった集団は一覧から消さない。`squad_of` が添字で所属を持っている
    /// ので、詰めると他の兵士の所属まで書き換わってしまう。
    /// この兵士の小集団。
    #[inline]
    pub fn squad_of(&self, id: SoldierId) -> Option<&Squad> {
        let index = *self.squad_of.get(id as usize)? as usize;
        self.squads.get(index)
    }

    #[inline]
    pub fn squad_index(&self, id: SoldierId) -> u16 {
        self.squad_of.get(id as usize).copied().unwrap_or(NO_SQUAD)
    }

    pub fn squads(&self) -> &[Squad] {
        &self.squads
    }

    /// 1 tick 進める。所属の見直しは周期的に、構成員の集計は毎 tick 行う。
    ///
    /// 集計だけを毎 tick 行うのは、先導役が倒れた・仲間が交戦を始めたといった
    /// 変化に個人の判断が即応できるようにするため。所属そのものは周期でしか
    /// 動かさないので、損耗のたびに全員が組み替わることはない。
    pub fn tick(&mut self, tick: u32, soldiers: &Soldiers, command: &CommandTree) {
        self.ensure_len(soldiers.len());
        if self.squads.is_empty() || tick >= self.next_review {
            self.next_review = tick.saturating_add(REVIEW_INTERVAL_TICKS);
            // ノードごとの集団一覧を 1 回だけ作る。集団の数は兵数に比例するので、
            // 「小さい集団ごとに全集団を走査」だと兵数の二乗になってしまう。
            let mut by_node: Vec<Vec<usize>> = vec![Vec::new(); command.nodes.len()];
            for (index, squad) in self.squads.iter().enumerate() {
                if let Some(list) = by_node.get_mut(squad.node as usize) {
                    list.push(index);
                }
            }
            self.assign_new_members(soldiers, command, &mut by_node);
            self.merge_small_squads(soldiers, &by_node);
        }
        self.refresh(soldiers);
    }

    /// まだ所属の無い兵士を、隊形スロット順に小集団へ入れる。
    ///
    /// スロット順にまとめると、隣り合って並んでいる者が同じ集団になる。
    fn assign_new_members(
        &mut self,
        soldiers: &Soldiers,
        command: &CommandTree,
        by_node: &mut [Vec<usize>],
    ) {
        for node in &command.nodes {
            let Some(unit) = &node.unit else {
                continue;
            };
            // このノードの既存集団のうち、空きがあるもの。
            let mut open: Vec<usize> = by_node
                .get(node.id as usize)
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|&index| self.squads[index].members.len() < MAX_SQUAD_SIZE)
                        .collect()
                })
                .unwrap_or_default();
            let mut unassigned: Vec<SoldierId> = unit
                .soldiers
                .iter()
                .copied()
                .filter(|&id| {
                    soldiers.is_active_id(id)
                        && self.squad_of.get(id as usize).copied().unwrap_or(NO_SQUAD) == NO_SQUAD
                })
                .collect();
            if unassigned.is_empty() {
                continue;
            }
            // 隊形スロット順（同じ列の隣どうしが同じ集団になる）。同スロットは
            // ID 順で安定させる。
            unassigned.sort_by_key(|&id| (soldiers.slot[id as usize], id));

            for id in unassigned {
                let index = match open.last().copied() {
                    Some(index) if self.squads[index].members.len() < MAX_SQUAD_SIZE => index,
                    _ => {
                        open.pop();
                        self.squads.push(Squad {
                            node: node.id,
                            members: Vec::with_capacity(MAX_SQUAD_SIZE),
                            leader: NO_ID,
                            centroid: Vec2Fx::ZERO,
                            engaged: 0,
                        });
                        let index = self.squads.len() - 1;
                        open.push(index);
                        if let Some(list) = by_node.get_mut(node.id as usize) {
                            list.push(index);
                        }
                        index
                    }
                };
                self.squads[index].members.push(id);
                if let Some(slot) = self.squad_of.get_mut(id as usize) {
                    *slot = index as u16;
                }
                if self.squads[index].members.len() >= MAX_SQUAD_SIZE {
                    open.pop();
                }
            }
        }
    }

    /// 小さくなりすぎた集団を、同じ部隊の近い集団へ合流させる。
    ///
    /// 動くのは合流する側の数人だけで、受け入れ側の所属は変わらない。
    fn merge_small_squads(&mut self, soldiers: &Soldiers, by_node: &[Vec<usize>]) {
        let alive_count = |squad: &Squad| {
            squad
                .members
                .iter()
                .filter(|&&id| soldiers.is_active_id(id))
                .count()
        };
        let merge_radius = sim_math::fx(MERGE_RADIUS_M) as i64;
        let small: Vec<usize> = (0..self.squads.len())
            .filter(|&index| {
                let count = alive_count(&self.squads[index]);
                count > 0 && count < MIN_SQUAD_SIZE
            })
            .collect();
        for index in small {
            let (node, centroid) = (self.squads[index].node, self.squads[index].centroid);
            let candidates = by_node.get(node as usize).map(|list| list.as_slice());
            let host = candidates
                .unwrap_or(&[])
                .iter()
                .copied()
                .filter(|&other| {
                    other != index
                        && alive_count(&self.squads[other]) + alive_count(&self.squads[index])
                            <= MERGE_MAX_SIZE
                        && dist_sq(self.squads[other].centroid, centroid)
                            <= merge_radius * merge_radius
                })
                // 近い集団から埋める。同距離なら index の小さい方（安定）。
                .min_by_key(|&other| (dist_sq(self.squads[other].centroid, centroid), other));
            let Some(host) = host else {
                continue;
            };
            let moving: Vec<SoldierId> = self.squads[index]
                .members
                .iter()
                .copied()
                .filter(|&id| soldiers.is_active_id(id))
                .collect();
            for id in moving {
                self.squads[index].members.retain(|&member| member != id);
                self.squads[host].members.push(id);
                if let Some(slot) = self.squad_of.get_mut(id as usize) {
                    *slot = host as u16;
                }
            }
        }
    }

    /// 生存者・重心・先導役・交戦人数を数え直す。
    fn refresh(&mut self, soldiers: &Soldiers) {
        for (index, squad) in self.squads.iter_mut().enumerate() {
            squad.members.retain(|&id| {
                soldiers.is_active_id(id) && {
                    // 所属表と食い違う者（合流で移った者）は落とす。
                    soldiers
                        .index_if_present(id)
                        .is_some_and(|i| self.squad_of[i] == index as u16)
                }
            });
            if squad.members.is_empty() {
                squad.leader = NO_ID;
                squad.engaged = 0;
                continue;
            }
            let mut sum = Vec2Fx::ZERO;
            let mut engaged = 0u16;
            let mut leader = NO_ID;
            let mut best_leadership = i32::MIN;
            for &id in &squad.members {
                let i = id as usize;
                sum = sum.add(soldiers.pos(i));
                if matches!(
                    soldiers.hot.state[i],
                    State::Engaged | State::Charging | State::Advancing
                ) {
                    engaged = engaged.saturating_add(1);
                }
                let attrs = soldiers.attrs[i];
                let leadership =
                    attrs.discipline as i32 * 2 + attrs.bravery as i32 + attrs.loyalty as i32
                        - if soldiers.hot.state[i] == State::Broken {
                            1_000
                        } else {
                            0
                        };
                if leadership > best_leadership || (leadership == best_leadership && id < leader) {
                    best_leadership = leadership;
                    leader = id;
                }
            }
            let count = squad.members.len() as Fx;
            squad.centroid = Vec2Fx::new(sum.x / count, sum.y / count);
            squad.leader = leader;
            squad.engaged = engaged;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organization::{Unit, FORMATION_LINE};
    use crate::soldiers::Attrs;
    use sim_math::{fx, fx_from_mm};

    fn line_unit(members: Vec<SoldierId>, origin: Vec2Fx) -> Unit {
        Unit {
            soldiers: members,
            troop_type: 0,
            formation: FORMATION_LINE,
            formation_origin: origin,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(900),
            rank_spacing: fx_from_mm(900),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: origin,
            pursuit_leash: None,
        }
    }

    fn world_with_unit(count: u32) -> (Soldiers, CommandTree) {
        let mut soldiers = Soldiers::default();
        let mut ids = Vec::new();
        for k in 0..count {
            let id = soldiers.push(fx(100 + k as i32), fx(100), 0, 0, 0, Attrs::default(), 0);
            soldiers.slot[id as usize] = k as u16;
            ids.push(id);
        }
        let mut command = CommandTree::new();
        command.add_node(
            None,
            0,
            0,
            ids[0],
            vec![],
            Some(line_unit(ids, Vec2Fx::new(fx(100), fx(100)))),
        );
        (soldiers, command)
    }

    #[test]
    fn soldiers_are_grouped_into_small_squads() {
        let (soldiers, command) = world_with_unit(20);
        let mut squads = SquadSystem::default();
        squads.tick(0, &soldiers, &command);
        // 合流で空になった集団は残るので、中身のあるものだけを見る。
        let filled: Vec<&Squad> = squads
            .squads()
            .iter()
            .filter(|squad| !squad.members.is_empty())
            .collect();
        assert!(filled.len() >= 20 / MERGE_MAX_SIZE);
        assert_eq!(
            filled
                .iter()
                .map(|squad| squad.members.len())
                .sum::<usize>(),
            20
        );
        for squad in filled {
            // 端数の集団は近くへ合流するので、受け入れ側は少し膨らむ。
            assert!(squad.members.len() <= MERGE_MAX_SIZE);
            assert_ne!(squad.leader, NO_ID);
        }
        // 隣り合ったスロットは同じ集団になる。
        assert_eq!(squads.squad_index(0), squads.squad_index(1));
    }

    /// 1 人倒れただけで全員の所属が変わらない（B3 の受け入れ条件）。
    #[test]
    fn losing_a_soldier_does_not_reshuffle_the_unit() {
        let (mut soldiers, command) = world_with_unit(20);
        let mut squads = SquadSystem::default();
        squads.tick(0, &soldiers, &command);
        let before: Vec<u16> = (0..20).map(|id| squads.squad_index(id)).collect();

        soldiers.hot.state[7] = State::Dead;
        squads.tick(REVIEW_INTERVAL_TICKS, &soldiers, &command);
        let after: Vec<u16> = (0..20).map(|id| squads.squad_index(id)).collect();
        let moved = before
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(id, (b, a))| b != a && *id != 7)
            .count();
        assert_eq!(moved, 0, "1 人の死で他の兵の所属が変わった");
    }

    /// 小さくなりすぎた集団は、近くの集団へ段階的に合流する。
    #[test]
    fn tiny_squads_merge_into_a_neighbour() {
        let (mut soldiers, command) = world_with_unit(12);
        let mut squads = SquadSystem::default();
        squads.tick(0, &soldiers, &command);
        // 2 つ目の集団（スロット 6〜11）を 1 人だけ残して倒す。
        let survivor = 11;
        for id in 6..11u32 {
            soldiers.hot.state[id as usize] = State::Dead;
        }
        let host_before = squads.squad_index(0);
        squads.tick(REVIEW_INTERVAL_TICKS, &soldiers, &command);
        assert_eq!(
            squads.squad_index(survivor),
            host_before,
            "取り残された兵が近くの集団へ合流していない"
        );
    }

    /// 先導役が倒れたら次の者へ移る。
    #[test]
    fn leadership_passes_on_when_the_leader_falls() {
        let (mut soldiers, command) = world_with_unit(6);
        // 1 人だけ規律を高くして先導役にする。
        soldiers.attrs[2].discipline = 250;
        let mut squads = SquadSystem::default();
        squads.tick(0, &soldiers, &command);
        assert_eq!(squads.squad_of(0).unwrap().leader, 2);

        soldiers.hot.state[2] = State::Downed;
        squads.tick(REVIEW_INTERVAL_TICKS, &soldiers, &command);
        let leader = squads.squad_of(0).unwrap().leader;
        assert_ne!(leader, 2);
        assert_ne!(leader, NO_ID);
    }
}
