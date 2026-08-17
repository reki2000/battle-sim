//! 徒歩兵の助走・衝突・転倒。
//!
//! 騎兵の突撃（[`crate::cavalry`]）と同じ考え方を徒歩に持ち込む。人ひとりの
//! 運動量は馬に比べればずっと小さいが、**止まって斬り合うより、助走をつけて
//! ぶつかった方が強い**という関係は同じで、それが兵士の行動に「一旦下がって
//! 走り込む」「離れている敵へ走っていってから攻撃する」という動機を与える。
//!
//! このモジュールが持つのは 3 つ。
//!
//! 1. **助走モメンタム**: `State::Charging` の徒歩兵が走った距離から育つ。
//!    移動速度（[`crate::World::desired_speed`]）と接触時の衝撃に効く。
//! 2. **接触の解決**: 助走をつけた兵士が敵にぶつかると、通常の白兵戦とは別に
//!    一度きりの衝撃（ダメージ・ノックバック・士気）が入る。踏ん張っている
//!    相手にぶつかれば逆に**弾き返されて転倒**することもある。
//! 3. **微視的な行動決定**: 間合いの外に敵がいれば走り込む。噛み合った戦列で
//!    手詰まりになり、後ろに余裕があれば、いったん下がって助走をつけ直す。
//!
//! 転倒（[`flags::STUMBLING`]）は倒れている数十 tick のあいだ、移動も攻撃も
//! できず、防御判定に大きな不利がつく（[`crate::combat`] 側で参照する）。
//! 密集した戦列で転ぶことが致命的なのは中世会戦の現実そのもので、
//! 「走って突っ込めば必ず得」にならないための代償でもある。

use sim_math::{
    dist_sq, fx_from_mm, fx_scale_permille, within_arc, Fx, Purpose, Rng, Vec2Fx, TICK_HZ,
};

use crate::combat::CombatSystem;
use crate::corpses::{CorpseField, STUMBLE_PERMILLE_PER_STACK};
use crate::soldiers::{flags, Gait, SoldierId, Soldiers, State, MAX_FATIGUE};
use crate::spatial::{SpatialHash, MAX_NEIGHBORS};

/// 助走がこの距離に達すると、疲れていない兵士のモメンタムが 1000‰ になる。
/// 人が全力に乗るのに要る距離は馬（100 m）よりずっと短く、鎧を着た兵士が
/// 数歩で出せる勢いはこの程度で頭打ちになる。
const CHARGE_FULL_DISTANCE_MM: u32 = 8_000;

/// これ未満の速度では助走距離を積まない（mm / tick）。60 mm/tick = 1.2 m/s。
const CHARGE_MIN_SPEED_MM_PER_TICK: i32 = 60;

/// 駆け出した瞬間の徒歩の速度（mm/s）。助走が乗る前でもここまでは出る。
pub const CHARGE_START_MM_PER_S: i32 = 2_400;
/// 徒歩の全力疾走の速度（mm/s）。モメンタム 1000‰ でここまで出る。
pub const SPRINT_TOP_MM_PER_S: i32 = 4_500;

/// これ未満のモメンタムでは衝撃を起こさない。勢いのない「歩いてきただけ」の
/// 接触は通常の白兵戦に任せる。
const IMPACT_MIN_PERMILLE: u16 = 250;

/// 衝撃ダメージの基準値（モメンタム 1000‰ で満額）。騎兵突撃
/// （[`crate::cavalry`] の 70）よりずっと小さい。
const BASE_IMPACT_DAMAGE: i32 = 22;
/// ノックバックの基準距離（mm、モメンタム 1000‰ で満額）。
const KNOCKBACK_BASE_MM: i32 = 600;
/// 衝撃を受けた側の士気ダメージ。
const IMPACT_MORALE_SHOCK: u16 = 45;
/// 転倒したときの士気ダメージ。
const STUMBLE_MORALE_SHOCK: u16 = 30;
/// 転倒したときの疲労。
const STUMBLE_FATIGUE: u16 = 150;

/// 接触とみなす間合い（両者の半径に加える分、mm）。
const CONTACT_RANGE_MM: i32 = 400;

/// 味方とこれ以上めり込んだら「隙間が足りずにぶつかった」とみなす（mm）。
/// 肩が触れる程度では転ばない。
const DEEP_CONTACT_MM: i32 = 80;

/// 起き上がるまでの tick 数の下限・上限。0.5 〜 1.3 秒。
const STUMBLE_TICKS_MIN: u8 = 10;
const STUMBLE_TICKS_MAX: u8 = 26;

/// 助走の判断を見直す間隔（tick）とそのばらつき。
const DECISION_INTERVAL: u32 = 40;
const DECISION_JITTER: i32 = 40;

/// 助走をかけに行く相手を探す最大距離（mm）。空間ハッシュの 3×3 セルで
/// 見える範囲に収める。
const RUNUP_MAX_DISTANCE_MM: i32 = 6_000;
/// 間合いよりこれだけ離れていれば「助走をつける余地がある」とみなす（mm）。
const RUNUP_MIN_GAP_MM: i32 = 1_200;

/// 下がる距離（mm）。ここから走り直す。
const BACKOFF_DISTANCE_MM: i32 = 3_000;
/// 下がっている時間（tick）。
const BACKOFF_TICKS: u32 = 22;
/// 後ろにこれより近く味方がいると下がれない（mm）。
const BACKOFF_REAR_CLEARANCE_MM: i32 = 1_400;
/// 助走を打ち切るまでの時間（tick）。
const RUNUP_TICKS: u32 = 60;

/// 助走を止めるかどうかを判定し始める距離（mm）。接触の一歩手前。
const FALTER_DISTANCE_MM: i32 = 4_000;
/// 失速判定の基準値（‰）。
const FALTER_BASE_PERMILLE: i32 = 140;
/// 正面で踏ん張っている敵 1 人あたりの失速の増分（‰）。
const FALTER_PER_BRACED_ENEMY: i32 = 90;
/// 一緒に走っている味方 1 人あたりの失速の減分（‰）。一人で突っ込むのは怖い。
const FALTER_PER_RUNNING_FRIEND: i32 = 60;
/// 失速したあと足が止まっている時間（tick）。
const FALTER_HOLD_TICKS: u32 = 40;
/// 失速したときの士気ダメージ。
const FALTER_MORALE_LOSS: u16 = 20;

/// 突撃してくる敵を「受け止める姿勢」に入る距離（mm）。
const BRACE_DISTANCE_MM: i32 = 4_000;
/// 受け止める姿勢で受けたときに残る衝撃（‰）。盾で受け流す分だけ減る。
const BRACED_IMPACT_PERMILLE: i32 = 550;

/// 下がる判断に必要な最低士気と、それを許す疲労の上限。
const BACKOFF_MIN_MORALE: u16 = 400;
const BACKOFF_MAX_FATIGUE: u16 = 5_000;
/// 走り込む判断に必要な最低士気と、それを許す疲労の上限。
const RUNUP_MIN_MORALE: u16 = 300;
const RUNUP_MAX_FATIGUE: u16 = 7_000;

/// 兵士が自分で選んでいる微視的な行動。
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Micro {
    /// 部隊の命令どおりに動く
    #[default]
    None = 0,
    /// 助走の距離を取るために下がっている
    BackOff = 1,
    /// 敵へ走り込んでいる
    RunUp = 2,
}

/// 助走・転倒の統計（UI とテスト用）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ChargeStats {
    /// 助走をつけた接触の回数
    pub impacts: u32,
    /// 転倒の回数
    pub stumbles: u32,
    /// 助走のために下がった回数
    pub backoffs: u32,
    /// 走り込みを始めた回数
    pub runups: u32,
    /// 接触の手前で足が止まった（失速した）回数
    pub falters: u32,
    /// 突撃を受け止める姿勢に入った回数
    pub braces: u32,
}

/// 徒歩兵の助走・転倒に関する per-soldier 状態。index は `SoldierId`。
#[derive(Debug, Default)]
pub struct ChargeSystem {
    /// 助走のモメンタム（0..1000‰）
    momentum_permille: Vec<u16>,
    /// 今回の助走で走った距離（mm）
    run_mm: Vec<u32>,
    /// 転倒から起き上がるまでの残り tick
    stumble_ticks: Vec<u8>,
    micro: Vec<Micro>,
    /// 現在の微視的行動が終わるティック
    micro_until: Vec<u32>,
    /// 下がる先／走り込む先
    focus_x: Vec<Fx>,
    focus_y: Vec<Fx>,
    /// 次に助走の判断をするティック
    next_decision: Vec<u32>,
    /// 前 tick に味方と深く接触していたか（ぶつかった瞬間だけ転倒判定するため）
    in_contact: Vec<bool>,
    /// 失速して足が止まっているあいだの、再開できるティック
    hesitate_until: Vec<u32>,
    /// この助走で失速判定を済ませたか（1 回の突撃につき 1 回だけ引く）
    falter_checked: Vec<bool>,
    pub stats: ChargeStats,
}

impl ChargeSystem {
    pub fn register(&mut self) {
        self.momentum_permille.push(0);
        self.run_mm.push(0);
        self.stumble_ticks.push(0);
        self.micro.push(Micro::None);
        self.micro_until.push(0);
        self.focus_x.push(0);
        self.focus_y.push(0);
        self.next_decision.push(0);
        self.in_contact.push(false);
        self.hesitate_until.push(0);
        self.falter_checked.push(false);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.momentum_permille.len() < len {
            self.register();
        }
    }

    /// 現在の助走モメンタム（0..1000‰）。
    #[inline]
    pub fn momentum_permille(&self, id: SoldierId) -> u16 {
        self.momentum_permille
            .get(id as usize)
            .copied()
            .unwrap_or(0)
    }

    /// 転倒中か。
    #[inline]
    pub fn is_stumbling(&self, id: SoldierId) -> bool {
        self.stumble_ticks.get(id as usize).copied().unwrap_or(0) > 0
    }

    /// 助走で敵へ詰めている最中か。
    ///
    /// この間は白兵戦の間合い（[`crate::World::apply_melee_standoff`]）を
    /// 無視して体ごとぶつかりに行く。助走の途中で「間合いを保つ」に切り替わって
    /// しまうと、勢いのある兵士が接触の手前で止まり、衝撃が永遠に入らない。
    #[inline]
    pub fn is_closing(&self, id: SoldierId) -> bool {
        self.micro(id) == Micro::RunUp || self.momentum_permille(id) >= IMPACT_MIN_PERMILLE
    }

    /// 接触の手前で足が止まっているか（突撃が寸前で失速した状態）。
    #[inline]
    pub fn is_hesitating(&self, id: SoldierId, tick: u32) -> bool {
        self.hesitate_until
            .get(id as usize)
            .is_some_and(|&until| tick < until)
    }

    /// 選んでいる微視的行動。
    #[inline]
    pub fn micro(&self, id: SoldierId) -> Micro {
        self.micro.get(id as usize).copied().unwrap_or(Micro::None)
    }

    /// 助走の距離を取るために下がっている最中か。
    ///
    /// この間は敵を見たまま退がる（向きは [`Self::decide`] が毎 tick 敵の方へ
    /// 向け直しており、移動方向では上書きしない）。
    #[inline]
    pub fn is_backing_off(&self, id: SoldierId) -> bool {
        self.micro(id) == Micro::BackOff
    }

    /// 転倒させる。既に転倒していれば何もしない。
    fn knock_down(&mut self, i: usize, soldiers: &mut Soldiers, ticks: u8) {
        if self.stumble_ticks[i] > 0 {
            return;
        }
        self.stumble_ticks[i] = ticks.clamp(STUMBLE_TICKS_MIN, STUMBLE_TICKS_MAX);
        soldiers.hot.flags[i] |= flags::STUMBLING;
        soldiers.hot.vel_x[i] = 0;
        soldiers.hot.vel_y[i] = 0;
        soldiers.fatigue[i] = soldiers.fatigue[i]
            .saturating_add(STUMBLE_FATIGUE)
            .min(MAX_FATIGUE);
        soldiers.morale[i] = soldiers.morale[i].saturating_sub(STUMBLE_MORALE_SHOCK);
        self.momentum_permille[i] = 0;
        self.run_mm[i] = 0;
        self.micro[i] = Micro::None;
        self.stats.stumbles = self.stats.stumbles.saturating_add(1);
    }

    /// フェーズ前半: 転倒からの復帰、助走モメンタムの更新、接触の衝撃。
    ///
    /// `World::tick` では `combat.tick` より**前**に呼ぶ。衝撃で `Engaged` に
    /// 落ちた兵士を、同じ tick 内で通常の白兵戦ロジックが拾えるようにするため
    /// （[`crate::cavalry`] と同じ理由）。
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
        combat: &mut CombatSystem,
        map_limit: Fx,
    ) {
        self.ensure_len(soldiers.len());
        let n = soldiers.len();
        let mut neighbors = [0u32; MAX_NEIGHBORS];

        for i in 0..n {
            // 転倒からの復帰。倒れているあいだは何もできない
            if self.stumble_ticks[i] > 0 {
                self.stumble_ticks[i] -= 1;
                if self.stumble_ticks[i] == 0 {
                    soldiers.hot.flags[i] &= !flags::STUMBLING;
                } else {
                    soldiers.hot.vel_x[i] = 0;
                    soldiers.hot.vel_y[i] = 0;
                    continue;
                }
            }

            // 受け止める姿勢は「突撃が来ているあいだだけ」。毎 tick 落とし、
            // 突撃してくる兵士の前方走査（下）が張り直す。
            soldiers.hot.flags[i] &= !flags::BRACED;

            let mounted = soldiers.hot.flags[i] & flags::MOUNTED != 0;
            if !soldiers.is_alive(i) || mounted {
                // 騎乗兵のモメンタムは `cavalry` が持つ
                self.reset(i);
                continue;
            }

            if soldiers.hot.state[i] != State::Charging {
                self.momentum_permille[i] = 0;
                self.run_mm[i] = 0;
                self.falter_checked[i] = false;
                continue;
            }

            let speed_mm = soldiers.speed_mm_per_tick(i);
            if speed_mm >= CHARGE_MIN_SPEED_MM_PER_TICK {
                self.run_mm[i] = self.run_mm[i].saturating_add(speed_mm as u32);
            } else {
                // 足が止まれば助走は失われる
                self.run_mm[i] = self.run_mm[i].saturating_sub(self.run_mm[i] / 4);
            }

            let raw_permille =
                ((self.run_mm[i] as u64 * 1000) / CHARGE_FULL_DISTANCE_MM as u64).min(1000) as u32;
            // 疲れているほど乗せられる勢いが減る
            let fatigue_cap = 1000u32
                .saturating_sub(soldiers.fatigue[i] as u32 * 500 / MAX_FATIGUE as u32)
                .max(300);
            self.momentum_permille[i] = ((raw_permille * fatigue_cap) / 1000).min(1000) as u16;

            // 前方を見る必要があるのは「勢いが乗って衝撃を起こしうる」か
            // 「まだ失速判定を引いていない」ときだけ。勢いのないまま走り続けて
            // いる兵士のために毎 tick 近傍を引くと、白兵戦の相手探しをもう一度
            // やるのと同じ重さになる。
            if self.momentum_permille[i] < IMPACT_MIN_PERMILLE && self.falter_checked[i] {
                continue;
            }

            // 前方の敵を見る。接触したか、その手前で足が止まるか、そして
            // 突っ込まれる側が身構えるかをここで決める。
            let pos = soldiers.pos(i);
            let count =
                hash.query_enemies(soldiers, pos.x, pos.y, soldiers.faction[i], &mut neighbors);
            let contact_r = soldiers.radius(i) + fx_from_mm(CONTACT_RANGE_MM);
            let half_arc = sim_math::brad_from_deg(50) as u32;
            let falter_r = fx_from_mm(FALTER_DISTANCE_MM) as i64;
            let brace_r = fx_from_mm(BRACE_DISTANCE_MM) as i64;
            let mut nearest_contact: Option<(i64, usize)> = None;
            let mut nearest_ahead: Option<i64> = None;
            let mut braced_ahead = 0i32;
            for &jid in &neighbors[..count] {
                let j = jid as usize;
                if !soldiers.is_alive(j) {
                    continue;
                }
                let jp = soldiers.pos(j);
                if !within_arc(soldiers.hot.facing[i], pos, jp, half_arc) {
                    continue;
                }
                let d2 = dist_sq(pos, jp);
                if d2 <= brace_r * brace_r {
                    // 突撃が来ていると気づいた敵は、受け止める姿勢に入る
                    if self.decide_brace(world_seed, tick, j, i, soldiers) {
                        braced_ahead += 1;
                    }
                }
                if d2 <= falter_r * falter_r
                    && nearest_ahead.map_or(true, |best| d2 < best)
                    && !matches!(soldiers.hot.state[j], State::Broken | State::Downed)
                {
                    nearest_ahead = Some(d2);
                }
                if d2 <= ((contact_r + soldiers.radius(j)) as i64).pow(2)
                    && nearest_contact.map_or(true, |(best, _)| d2 < best)
                {
                    nearest_contact = Some((d2, j));
                }
            }

            // 突撃が寸前で失速する。中世会戦では、接触まで走りきる突撃より
            // 手前で足が止まる突撃の方が多かった（仕様 06 章 3.7 節）。
            if nearest_ahead.is_some() && !self.falter_checked[i] {
                self.falter_checked[i] = true;
                let running_friends = count_running_friends(i, pos, soldiers, hash);
                let mut falter = FALTER_BASE_PERMILLE;
                falter += braced_ahead.min(4) * FALTER_PER_BRACED_ENEMY;
                falter -= running_friends.min(6) * FALTER_PER_RUNNING_FRIEND;
                falter -= soldiers.attrs[i].bravery as i32 / 3;
                falter -= (soldiers.morale[i] as i32 - 500) / 8;
                let mut rng = Rng::stream(world_seed, i as u32, Purpose::ChargeDecision, tick);
                if rng.chance_permille(falter.clamp(0, 900) as u32) {
                    self.falter(i, tick, soldiers);
                    continue;
                }
            }

            if self.momentum_permille[i] < IMPACT_MIN_PERMILLE {
                continue;
            }
            let Some((_, target)) = nearest_contact else {
                continue;
            };

            self.resolve_impact(world_seed, tick, i, target, soldiers, combat, map_limit);
        }
    }

    /// 突撃を受ける側が、盾を構えて足を止めるかどうかを決める。
    ///
    /// 判定は 1 秒ごとに引き直す（毎 tick 引くと構えたり解いたりを繰り返す）。
    /// 規律の高い兵士は踏みとどまり、そうでない兵士は身をかわそうとする。
    fn decide_brace(
        &mut self,
        world_seed: u64,
        tick: u32,
        defender: usize,
        charger: usize,
        soldiers: &mut Soldiers,
    ) -> bool {
        if !soldiers.is_alive(defender)
            || soldiers.is_stumbling(defender)
            || soldiers.hot.flags[defender] & flags::MOUNTED != 0
            || matches!(
                soldiers.hot.state[defender],
                State::Broken | State::Rallying | State::Charging
            )
        {
            return false;
        }
        // 突撃してくる相手を見ていなければ身構えられない
        if !within_arc(
            soldiers.hot.facing[defender],
            soldiers.pos(defender),
            soldiers.pos(charger),
            sim_math::brad_from_deg(60) as u32,
        ) {
            return false;
        }
        if soldiers.is_braced(defender) {
            return true;
        }
        let attrs = soldiers.attrs[defender];
        let chance =
            (400 + attrs.discipline as i32 * 2 + attrs.bravery as i32 - 200).clamp(50, 950);
        let mut rng = Rng::stream(
            world_seed,
            defender as u32,
            Purpose::ChargeDecision,
            tick / sim_math::TICK_HZ,
        );
        if rng.chance_permille(chance as u32) {
            soldiers.hot.flags[defender] |= flags::BRACED;
            soldiers.hot.vel_x[defender] = 0;
            soldiers.hot.vel_y[defender] = 0;
            self.stats.braces = self.stats.braces.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// 接触の手前で足が止まる。助走は失われ、しばらく踏み出せない。
    fn falter(&mut self, i: usize, tick: u32, soldiers: &mut Soldiers) {
        self.momentum_permille[i] = 0;
        self.run_mm[i] = 0;
        self.micro[i] = Micro::None;
        self.hesitate_until[i] = tick + FALTER_HOLD_TICKS;
        self.next_decision[i] = tick + FALTER_HOLD_TICKS;
        soldiers.hot.vel_x[i] = 0;
        soldiers.hot.vel_y[i] = 0;
        soldiers.morale[i] = soldiers.morale[i].saturating_sub(FALTER_MORALE_LOSS);
        if soldiers.hot.state[i] == State::Charging {
            soldiers.hot.state[i] = State::Advancing;
        }
        self.stats.falters = self.stats.falters.saturating_add(1);
    }

    /// 助走をつけた徒歩兵が敵に届いた瞬間の解決。
    #[allow(clippy::too_many_arguments)]
    fn resolve_impact(
        &mut self,
        world_seed: u64,
        tick: u32,
        i: usize,
        target: usize,
        soldiers: &mut Soldiers,
        combat: &mut CombatSystem,
        map_limit: Fx,
    ) {
        let m = self.momentum_permille[i] as i32;
        let pos = soldiers.pos(i);

        // 相手が盾を構えて足を止めていれば「踏ん張っている」。突っ込んだ側が
        // 弾き返されるのはこの場合（[`Self::decide_brace`]）。姿勢に入って
        // いなくても、正面からこちらを見ていれば多少は耐える。
        let facing_me = within_arc(
            soldiers.hot.facing[target],
            soldiers.pos(target),
            pos,
            sim_math::brad_from_deg(60) as u32,
        ) && !matches!(
            soldiers.hot.state[target],
            State::Broken | State::Wavering | State::Downed
        );
        let set_shield = soldiers.is_braced(target);
        let braced = facing_me || set_shield;

        // 受け止める姿勢は衝撃そのものを弱める（盾で受け流す）
        let m = if set_shield {
            m * BRACED_IMPACT_PERMILLE / 1000
        } else {
            m
        };
        let damage = ((BASE_IMPACT_DAMAGE * m) / 1000).max(2) as u16;
        combat.apply_impact_damage(i as SoldierId, target as SoldierId, damage, soldiers, tick);
        self.stats.impacts = self.stats.impacts.saturating_add(1);

        let attacker_push = soldiers.mass(i) + soldiers.attrs[i].strength as i32;
        let defender_push = soldiers.mass(target) + soldiers.attrs[target].strength as i32;

        if soldiers.is_alive(target) {
            let dir = soldiers.pos(target).sub(pos).normalized();
            let push_len = fx_scale_permille(
                fx_from_mm(KNOCKBACK_BASE_MM * attacker_push / defender_push.max(1)),
                m,
            );
            let pushed = soldiers.pos(target).add(dir.scale(push_len));
            soldiers.set_pos(
                target,
                Vec2Fx::new(pushed.x.clamp(0, map_limit), pushed.y.clamp(0, map_limit)),
            );
            soldiers.morale[target] = soldiers.morale[target].saturating_sub(IMPACT_MORALE_SHOCK);

            // 受けた側の転倒。踏ん張っていれば耐えやすい
            let mut rng = Rng::stream(world_seed, target as u32, Purpose::Stumble, tick);
            let mut chance = m / 3;
            chance += (attacker_push - defender_push).clamp(-200, 200) / 2;
            chance -= soldiers.attrs[target].skill as i32 / 4;
            if braced {
                chance -= 150;
            }
            if soldiers.hot.state[target] == State::Broken {
                chance += 120;
            }
            if rng.chance_permille(chance.clamp(0, 800) as u32) {
                let ticks = recovery_ticks(&mut rng, soldiers.attrs[target].accel);
                self.knock_down(target, soldiers, ticks);
            }
        }

        // 突っ込んだ側が弾き返されて転ぶこともある
        let mut rng = Rng::stream(world_seed, i as u32, Purpose::Stumble, tick);
        let mut recoil = if braced { 180 } else { 40 };
        recoil += (defender_push - attacker_push).clamp(-200, 200) / 2;
        recoil -= soldiers.attrs[i].skill as i32 / 4;
        recoil += m / 8;
        if rng.chance_permille(recoil.clamp(0, 700) as u32) {
            let ticks = recovery_ticks(&mut rng, soldiers.attrs[i].accel);
            self.knock_down(i, soldiers, ticks);
            return;
        }

        // 突撃は一度きり。勢いを使い切って白兵戦へ移る。次に助走をつけ直すか
        // どうかは、間を置いてから改めて判断する
        self.momentum_permille[i] = 0;
        self.run_mm[i] = 0;
        self.micro[i] = Micro::None;
        self.next_decision[i] = tick + DECISION_INTERVAL;
        if soldiers.is_alive(i) {
            soldiers.hot.state[i] = State::Engaged;
        }
    }

    fn reset(&mut self, i: usize) {
        self.momentum_permille[i] = 0;
        self.run_mm[i] = 0;
        self.micro[i] = Micro::None;
    }

    /// フェーズ後半: 「下がって助走をつける」「離れた敵へ走り込む」の決定。
    ///
    /// `command.formation_goals` より後に呼び、決めた兵士の `goal` を上書きする。
    /// 上書きは 1 tick 限りで、次の tick も同じ判断が続くかぎり毎 tick 塗り直す
    /// ——部隊の命令が変われば自然に元へ戻る。
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
        combat: &CombatSystem,
        goals: &mut [Vec2Fx],
        map_limit: Fx,
    ) {
        self.ensure_len(soldiers.len());
        let n = soldiers.len().min(goals.len());
        let mut neighbors = [0u32; MAX_NEIGHBORS];

        // SoA なので index で回す（`goals` 以外に 6 本の配列を同じ index で
        // 引くため、イテレータ化しても読みやすくならない）
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            // 突撃が寸前で失速した兵士は、しばらく足が止まる（実際に止めるのは
            // `World::steer`。ここで `goal` を書き換えてしまうと、部隊に属さない
            // 兵士の目標を壊す）
            if self.is_hesitating(i as SoldierId, tick) {
                continue;
            }
            // 何も進行中でなく、見直しの番でもない兵士は近傍を見るまでもない。
            // 近傍クエリは 1 体あたりでは安いが、毎 tick 全員ぶん回すと
            // 白兵戦の相手探しをもう一度やるのと同じ重さになる。
            if self.micro[i] == Micro::None && tick < self.next_decision[i] {
                continue;
            }
            if !self.can_act(i, soldiers, combat) {
                if self.micro[i] != Micro::None {
                    self.reset(i);
                }
                continue;
            }

            let pos = soldiers.pos(i);
            let nearest = nearest_enemy(i, pos, soldiers, hash, &mut neighbors);

            // 今の行動を続けるか、終わったか
            match self.micro[i] {
                Micro::BackOff => {
                    if tick >= self.micro_until[i] {
                        self.begin_runup(i, tick, nearest.map(|(_, j)| soldiers.pos(j)), soldiers);
                    } else {
                        goals[i] = Vec2Fx::new(self.focus_x[i], self.focus_y[i]);
                        if let Some((_, j)) = nearest {
                            // 下がりながらも敵から目を離さない
                            soldiers.hot.facing[i] = soldiers.pos(j).sub(pos).angle();
                        }
                    }
                    continue;
                }
                Micro::RunUp => {
                    let done = tick >= self.micro_until[i] || nearest.is_none();
                    if done {
                        self.reset(i);
                        // 走り込みが空振りに終わった直後にまた走り出さない
                        self.next_decision[i] = tick + DECISION_INTERVAL;
                        if soldiers.hot.state[i] == State::Charging {
                            soldiers.hot.state[i] = State::Advancing;
                        }
                    } else if let Some((_, j)) = nearest {
                        self.focus_x[i] = soldiers.pos(j).x;
                        self.focus_y[i] = soldiers.pos(j).y;
                        goals[i] = soldiers.pos(j);
                        soldiers.hot.state[i] = State::Charging;
                    }
                    continue;
                }
                Micro::None => {}
            }

            if tick < self.next_decision[i] {
                continue;
            }
            let mut rng = Rng::stream(world_seed, i as u32, Purpose::ChargeDecision, tick);
            self.next_decision[i] =
                tick + (DECISION_INTERVAL as i32 + rng.range(0, DECISION_JITTER)) as u32;

            let Some((d2, j)) = nearest else {
                continue;
            };
            let attrs = soldiers.attrs[i];
            let reach = combat.weapons[i].reach + soldiers.radius(i) + soldiers.radius(j);
            let gap_start = (reach + fx_from_mm(RUNUP_MIN_GAP_MM)) as i64;

            if d2 > gap_start * gap_start {
                // 間合いの外に敵がいる: 走っていって、勢いのまま当たる
                if soldiers.fatigue[i] > RUNUP_MAX_FATIGUE || soldiers.morale[i] < RUNUP_MIN_MORALE
                {
                    continue;
                }
                let chance = (450 + attrs.aggression as i32 * 2).clamp(0, 950) as u32;
                if rng.chance_permille(chance) {
                    self.begin_runup(i, tick, Some(soldiers.pos(j)), soldiers);
                }
                continue;
            }

            // 間合いの中で噛み合っている: 下がって助走をつけ直す価値があるか
            if soldiers.fatigue[i] > BACKOFF_MAX_FATIGUE || soldiers.morale[i] < BACKOFF_MIN_MORALE
            {
                continue;
            }
            let away = pos.sub(soldiers.pos(j)).normalized();
            if !rear_is_clear(i, pos, away, soldiers, hash, &mut neighbors) {
                continue;
            }
            // 引き際は「攻めっ気」が決め、規律の高い兵士は列を保つ方を選ぶ
            let chance =
                (60 + attrs.aggression as i32 * 2 - attrs.discipline as i32).clamp(20, 350) as u32;
            if !rng.chance_permille(chance) {
                continue;
            }
            let anchor = pos.add(away.scale(fx_from_mm(BACKOFF_DISTANCE_MM)));
            self.focus_x[i] = anchor.x.clamp(0, map_limit);
            self.focus_y[i] = anchor.y.clamp(0, map_limit);
            self.micro[i] = Micro::BackOff;
            self.micro_until[i] = tick + BACKOFF_TICKS;
            goals[i] = Vec2Fx::new(self.focus_x[i], self.focus_y[i]);
            soldiers.hot.facing[i] = soldiers.pos(j).sub(pos).angle();
            if soldiers.hot.state[i] == State::Engaged {
                soldiers.hot.state[i] = State::Repositioning;
            }
            self.stats.backoffs = self.stats.backoffs.saturating_add(1);
        }
    }

    fn begin_runup(
        &mut self,
        i: usize,
        tick: u32,
        target_pos: Option<Vec2Fx>,
        soldiers: &mut Soldiers,
    ) {
        let Some(target_pos) = target_pos else {
            self.reset(i);
            return;
        };
        self.micro[i] = Micro::RunUp;
        self.micro_until[i] = tick + RUNUP_TICKS;
        self.focus_x[i] = target_pos.x;
        self.focus_y[i] = target_pos.y;
        self.run_mm[i] = 0;
        // 新しい助走なので、失速判定も引き直す
        self.falter_checked[i] = false;
        soldiers.hot.state[i] = State::Charging;
        self.stats.runups = self.stats.runups.saturating_add(1);
    }

    /// 自分で助走の判断をしてよい兵士か。
    fn can_act(&self, i: usize, soldiers: &Soldiers, combat: &CombatSystem) -> bool {
        if !soldiers.is_alive(i) || soldiers.is_stumbling(i) {
            return false;
        }
        if soldiers.hot.flags[i] & (flags::MOUNTED | flags::ENGINEER) != 0 {
            return false;
        }
        // 射手は距離を取るのが仕事なので、自分から走り込まない
        if combat.weapons[i].ranged {
            return false;
        }
        !matches!(
            soldiers.hot.state[i],
            State::Broken | State::Rallying | State::Working | State::Downed | State::Dead
        )
    }

    /// フェーズ後半: 走っている兵士が人や死体にぶつかってつまずく判定。
    ///
    /// 衝突解決（押し合い）の後に呼ぶ。押し合いで解消しきれずに体が深く
    /// 重なったままの組が「ぶつかった」組で、走り込んだ側ほど転びやすい。
    pub fn resolve_run_collisions(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        hash: &SpatialHash,
        corpses: &CorpseField,
    ) {
        self.ensure_len(soldiers.len());
        let n = soldiers.len();
        let mut neighbors = [0u32; MAX_NEIGHBORS];

        for i in 0..n {
            if !soldiers.is_alive(i)
                || soldiers.is_stumbling(i)
                || soldiers.hot.flags[i] & flags::MOUNTED != 0
                || soldiers.gait(i) != Gait::Running
            {
                self.in_contact[i] = false;
                continue;
            }

            // 死体の山につまずく（跨ぎ損ねる）
            let stack = corpses.stack_at(soldiers.pos(i)) as u32;
            if stack > 0 {
                let mut rng = Rng::stream(world_seed, i as u32, Purpose::Stumble, tick);
                let chance = (stack * STUMBLE_PERMILLE_PER_STACK)
                    .saturating_sub(soldiers.attrs[i].accel as u32 / 32);
                if rng.chance_permille(chance.min(200)) {
                    let ticks = recovery_ticks(&mut rng, soldiers.attrs[i].accel);
                    self.knock_down(i, soldiers, ticks);
                    continue;
                }
            }

            // 走り抜けようとして味方にぶつかる。押し合いの後でもなお、必要な
            // 隙間（[`Soldiers::pass_radius`]）に対して深く重なったままの組が
            // 「隙間が足りなかった」組。敵との接触は助走の衝撃
            // （[`Self::resolve_impact`]）が別に扱うのでここでは見ない。
            let pos = soldiers.pos(i);
            let count = hash.query_neighbors(pos.x, pos.y, &mut neighbors);
            let mut worst: Option<(Fx, usize)> = None;
            for &jid in &neighbors[..count] {
                let j = jid as usize;
                if j == i
                    || !soldiers.is_alive(j)
                    || soldiers.is_stumbling(j)
                    || soldiers.faction[j] != soldiers.faction[i]
                {
                    continue;
                }
                let rsum = soldiers.pass_radius(i) + soldiers.pass_radius(j);
                let d2 = dist_sq(pos, soldiers.pos(j));
                if d2 >= (rsum as i64) * (rsum as i64) {
                    continue;
                }
                let overlap = rsum - sim_math::isqrt64(d2 as u64) as Fx;
                if worst.map_or(true, |(best, _)| overlap > best) {
                    worst = Some((overlap, j));
                }
            }
            let deep = worst.is_some_and(|(overlap, _)| overlap >= fx_from_mm(DEEP_CONTACT_MM));
            // ぶつかった「瞬間」だけ判定する。押し合いが続くあいだ毎 tick
            // 転倒判定をすると、詰まった隊列の中の走者が必ず転ぶことになる。
            let was_in_contact = self.in_contact[i];
            self.in_contact[i] = deep;
            if !deep || was_in_contact {
                continue;
            }
            let (_, j) = worst.expect("deep なら worst がある");

            let mut rng = Rng::stream(world_seed, i as u32, Purpose::Stumble, tick);
            let mine = soldiers.mass(i) + soldiers.attrs[i].strength as i32;
            let theirs = soldiers.mass(j) + soldiers.attrs[j].strength as i32;
            let mut chance = 120 + (theirs - mine).clamp(-200, 400) / 2;
            chance -= soldiers.attrs[i].skill as i32 / 4;
            if soldiers.gait(j) == Gait::Standing {
                // 踏ん張っている相手にぶつかると弾き返される
                chance += 80;
            }
            if rng.chance_permille(chance.clamp(0, 600) as u32) {
                let ticks = recovery_ticks(&mut rng, soldiers.attrs[i].accel);
                self.knock_down(i, soldiers, ticks);
                // ぶつかられた側も巻き込まれることがある
                if rng.chance_permille(250) {
                    let ticks = recovery_ticks(&mut rng, soldiers.attrs[j].accel);
                    self.knock_down(j, soldiers, ticks);
                }
            }
        }
    }

    /// 決定論検証用の状態ハッシュ。
    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for i in 0..self.momentum_permille.len() {
            mix(self.momentum_permille[i] as u64);
            mix(self.run_mm[i] as u64);
            mix(self.stumble_ticks[i] as u64);
            mix(self.micro[i] as u64);
            mix(self.micro_until[i] as u64);
            mix(self.next_decision[i] as u64);
            mix(self.in_contact[i] as u64);
            mix(self.hesitate_until[i] as u64);
            mix(self.falter_checked[i] as u64);
        }
        h
    }
}

/// 転倒から起き上がるまでの tick 数。身のこなし（`accel`）が良いほど早い。
fn recovery_ticks(rng: &mut Rng, accel: u8) -> u8 {
    let span = (STUMBLE_TICKS_MAX - STUMBLE_TICKS_MIN) as i32;
    let base = STUMBLE_TICKS_MIN as i32 + rng.range(0, span + 1);
    (base - accel as i32 / 48).clamp(STUMBLE_TICKS_MIN as i32, STUMBLE_TICKS_MAX as i32) as u8
}

/// 近くで一緒に走っている味方の数。集団で突っ込むほど足は止まりにくい。
fn count_running_friends(i: usize, pos: Vec2Fx, soldiers: &Soldiers, hash: &SpatialHash) -> i32 {
    let mut buf = [0u32; MAX_NEIGHBORS];
    let count = hash.query_neighbors(pos.x, pos.y, &mut buf);
    let mut friends = 0;
    for &jid in &buf[..count] {
        let j = jid as usize;
        if j == i || !soldiers.is_alive(j) || soldiers.faction[j] != soldiers.faction[i] {
            continue;
        }
        if soldiers.hot.state[j] == State::Charging && soldiers.gait(j) == Gait::Running {
            friends += 1;
        }
    }
    friends
}

/// 近傍から最も近い敵を探す。返すのは（距離の二乗, index）。
fn nearest_enemy(
    i: usize,
    pos: Vec2Fx,
    soldiers: &Soldiers,
    hash: &SpatialHash,
    neighbors: &mut [u32; MAX_NEIGHBORS],
) -> Option<(i64, usize)> {
    let count = hash.query_enemies(soldiers, pos.x, pos.y, soldiers.faction[i], neighbors);
    let limit = fx_from_mm(RUNUP_MAX_DISTANCE_MM) as i64;
    let mut best: Option<(i64, usize)> = None;
    for &jid in &neighbors[..count] {
        let j = jid as usize;
        if !soldiers.is_alive(j) {
            continue;
        }
        let d2 = dist_sq(pos, soldiers.pos(j));
        if d2 > limit * limit {
            continue;
        }
        if best.map_or(true, |(bd, bj)| (d2, j) < (bd, bj)) {
            best = Some((d2, j));
        }
    }
    best
}

/// 後ろに下がる余地があるか。真後ろに味方が詰まっていれば下がれない。
fn rear_is_clear(
    i: usize,
    pos: Vec2Fx,
    away: Vec2Fx,
    soldiers: &Soldiers,
    hash: &SpatialHash,
    neighbors: &mut [u32; MAX_NEIGHBORS],
) -> bool {
    let count = hash.query_neighbors(pos.x, pos.y, neighbors);
    let clearance = fx_from_mm(BACKOFF_REAR_CLEARANCE_MM) as i64;
    let rear_facing = away.angle();
    for &jid in &neighbors[..count] {
        let j = jid as usize;
        if j == i || !soldiers.is_alive(j) {
            continue;
        }
        let jp = soldiers.pos(j);
        if dist_sq(pos, jp) > clearance * clearance {
            continue;
        }
        if within_arc(rear_facing, pos, jp, sim_math::brad_from_deg(50) as u32) {
            return false;
        }
    }
    true
}

/// 走り続けたときの 1 秒あたりの疲労（仕様 06 章 6 節）。
pub const FATIGUE_PER_SEC_SPRINT: u16 = 25;
/// 早足の 1 秒あたりの疲労。
pub const FATIGUE_PER_SEC_TROT: u16 = 8;
/// 歩行の 1 秒あたりの疲労。
pub const FATIGUE_PER_SEC_WALK: u16 = 3;
/// 立ち止まっているときの 1 秒あたりの回復。
pub const FATIGUE_RECOVERY_PER_SEC: u16 = 2;
/// 「早足」とみなす速度（mm/s）。これ以上は走っている扱い。
pub const TROT_MIN_MM_PER_S: i32 = 2_000;
/// 「全力疾走」とみなす速度（mm/s）。
pub const SPRINT_MIN_MM_PER_S: i32 = 3_500;

/// 移動による疲労の増減（1 秒分）。`endurance` が高いほど消費が小さい。
pub fn movement_fatigue_delta(speed_mm_per_tick: i32, endurance: u8) -> i32 {
    let mm_per_s = speed_mm_per_tick * TICK_HZ as i32;
    let base = if mm_per_s >= SPRINT_MIN_MM_PER_S {
        FATIGUE_PER_SEC_SPRINT as i32
    } else if mm_per_s >= TROT_MIN_MM_PER_S {
        FATIGUE_PER_SEC_TROT as i32
    } else if mm_per_s >= 300 {
        FATIGUE_PER_SEC_WALK as i32
    } else {
        return -(FATIGUE_RECOVERY_PER_SEC as i32);
    };
    // 仕様 06 章 6 節の「× (2 - endurance/255)」
    base * (510 - endurance as i32) / 255
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::{brad_from_deg, fx};

    fn attrs() -> Attrs {
        Attrs::new(140, 140, 140, 140, 140, 140, 140, 140, 200, 100, 140, 140)
    }

    #[test]
    fn momentum_needs_a_run_up() {
        let mut soldiers = Soldiers::default();
        let id = soldiers.push(fx(50), fx(50), 0, 0, 0, attrs(), 0);
        soldiers.hot.state[id as usize] = State::Charging;
        let mut charge = ChargeSystem::default();
        charge.register();
        let mut combat = CombatSystem::default();
        combat.register();
        let mut hash = SpatialHash::default();
        hash.rebuild(&soldiers);

        // 止まったままではモメンタムは育たない
        charge.tick(1, 0, &mut soldiers, &hash, &mut combat, fx(500));
        assert_eq!(charge.momentum_permille(id), 0);

        // 走れば育つ
        soldiers.hot.vel_x[id as usize] = sim_math::fx_from_mm(200) as i16;
        for tick in 1..40 {
            charge.tick(1, tick, &mut soldiers, &hash, &mut combat, fx(500));
        }
        assert!(
            charge.momentum_permille(id) > 500,
            "助走してもモメンタムが乗らない: {}",
            charge.momentum_permille(id)
        );
    }

    #[test]
    fn a_charging_soldier_hits_harder_than_a_standing_one() {
        fn run(charging: bool) -> u16 {
            let mut soldiers = Soldiers::default();
            let a = soldiers.push(fx(50), fx(50), 0, 0, 0, attrs(), 0);
            let b = soldiers.push(
                fx(50) + sim_math::fx_from_mm(800),
                fx(50),
                brad_from_deg(180),
                0,
                1,
                attrs(),
                0,
            );
            let mut charge = ChargeSystem::default();
            let mut combat = CombatSystem::default();
            for _ in 0..2 {
                charge.register();
                combat.register();
            }
            if charging {
                soldiers.hot.state[a as usize] = State::Charging;
                soldiers.hot.vel_x[a as usize] = sim_math::fx_from_mm(200) as i16;
            }
            let mut hash = SpatialHash::default();
            for tick in 0..30 {
                hash.rebuild(&soldiers);
                charge.tick(7, tick, &mut soldiers, &hash, &mut combat, fx(500));
            }
            crate::soldiers::MAX_HP - soldiers.hp[b as usize]
        }

        let charged = run(true);
        let standing = run(false);
        assert_eq!(standing, 0, "助走なしで衝撃が入っている");
        assert!(charged > 0, "助走をつけても衝撃が入らない");
    }

    #[test]
    fn a_stumbled_soldier_cannot_move_for_a_while() {
        let mut soldiers = Soldiers::default();
        let id = soldiers.push(fx(50), fx(50), 0, 0, 0, attrs(), 0);
        let mut charge = ChargeSystem::default();
        charge.register();
        let mut combat = CombatSystem::default();
        combat.register();
        let hash = SpatialHash::default();

        soldiers.hot.vel_x[id as usize] = sim_math::fx_from_mm(200) as i16;
        charge.knock_down(id as usize, &mut soldiers, 20);
        assert!(soldiers.is_stumbling(id as usize));
        assert_eq!(soldiers.hot.vel_x[id as usize], 0);

        for tick in 0..19 {
            charge.tick(1, tick, &mut soldiers, &hash, &mut combat, fx(500));
            assert!(soldiers.is_stumbling(id as usize), "早く起き上がりすぎ");
        }
        charge.tick(1, 19, &mut soldiers, &hash, &mut combat, fx(500));
        assert!(!soldiers.is_stumbling(id as usize), "起き上がらない");
    }

    #[test]
    fn sprinting_costs_more_fatigue_than_walking() {
        let walk = movement_fatigue_delta(60, 140); // 1.2 m/s
        let trot = movement_fatigue_delta(120, 140); // 2.4 m/s
        let sprint = movement_fatigue_delta(220, 140); // 4.4 m/s
        let idle = movement_fatigue_delta(0, 140);
        assert!(idle < 0, "止まっていて回復しない");
        assert!(walk > 0 && trot > walk && sprint > trot);
        // 頑健な兵士ほど消費が小さい
        assert!(movement_fatigue_delta(220, 250) < sprint);
    }
}
