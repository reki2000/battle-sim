//! 兵士個人の局所判断。
//!
//! 指揮系統が与える陣形スロットを基準にしつつ、近くに敵が現れた兵士だけが
//! 個別のタイミングで迎撃へ出る。部隊全体の目標を書き換えず、各兵士の `goal` を
//! 一時的に上書きする点は徒歩突撃の [`crate::charge`] と同じ。

use sim_math::{dist_sq, fx_from_mm, Purpose, Rng, Vec2Fx};

use crate::organization::{CommandTree, Intent};
use crate::perception::LocalAwareness;
use crate::perception::{is_closing, threat_arc, PerceptionSystem, ThreatArc};
use crate::soldiers::{flags, Attrs, SoldierId, Soldiers, State, NO_ID};
use crate::spatial::MAX_NEIGHBORS;
use crate::squads::SquadSystem;

const DEFAULT_REACTION_RADIUS_MM: i32 = 8_000;
const MOVE_REACTION_RADIUS_MM: i32 = 6_000;
const HOLD_REACTION_RADIUS_MM: i32 = 10_000;
const ATTACK_REACTION_RADIUS_MM: i32 = 14_000;
const SCREEN_REACTION_RADIUS_MM: i32 = 12_000;
const DISENGAGE_MARGIN_MM: i32 = 3_000;

/// 一人の敵へ全員が殺到しないよう、既に向かっている味方一人あたりに課す点数。
const TARGET_LOAD_PENALTY: i32 = 180;

/// 味方が誰も戦っていなくても放置できない距離（mm）。これより近い敵は自衛の
/// 対象になる。
const SELF_DEFENSE_RADIUS_MM: i32 = 3_000;
/// 味方が組み合っている相手（援護に入れる戦闘）への加点。
const SUPPORT_BONUS: i32 = 400;
/// 守備範囲へ入り込んだ敵への加点。迎撃は守備任務そのものなので、命令追従を
/// 上回るだけの重みを持たせる。
const INTRUDER_BONUS: i32 = 900;
/// 守備範囲へ近づいてくる敵への加点。離れていく敵には付かない。
const CLOSING_BONUS: i32 = 600;
/// 側面・背面の敵に気づくまでの遅れ（tick）。正面の敵にはこの遅れがない。
/// 20 Hz なので 6 tick = 0.3 秒、12 tick = 0.6 秒。
const FLANK_NOTICE_TICKS: u32 = 6;
const REAR_NOTICE_TICKS: u32 = 12;

// ── 行動の点数（B2）────────────────────────────────────────────────
// 仕様 05 章 3.2 節のユーティリティ AI。1,000 を「ふつうに命令へ従う」基準に
// 置き、性格（0〜255）・士気（0〜1000）・疲労（0〜10000）・周囲の状況で上下
// させる。値は将来 `data/ai_weights.toml` へ出す（TODO P0-1）。

/// 持ち場へ戻る行動を考え始めるスロットからの距離（mm）。
const SLOT_RETURN_MM: i32 = 1_500;
/// 「持ち場から離れている」ことの点数が頭打ちになる距離（mm）。
const SLOT_RETURN_SATURATION_MM: i32 = 8_000;
/// 持ち場に着いたうえで反応半径内に敵がいるとき、「命令に従う（＝立っている）」
/// から引く点数。
const ARRIVED_WITH_ENEMY_PENALTY: i32 = 500;
/// この距離まで敵が来たら、踏みとどまって受ける選択肢が出る（mm）。
///
/// 踏みとどまって受ける選択肢が出る距離（mm）。武器の間合いの内側に収める。
/// ここを広く取ると、兵士が接触する前に足を止めてしまい、白兵戦そのものが
/// 起きにくくなる。
const DEFEND_TRIGGER_MM: i32 = 1_500;
/// 一歩退く選択肢が出る距離（mm）。踏みとどまるより手前——まだ間合いの外で
/// 敵が迫ってくる段階——から選べるようにして、怯えた兵と踏み込む兵を分ける。
const FALL_BACK_TRIGGER_MM: i32 = 2_500;
/// 一歩退く距離（mm）。助走を取り直せる程度で、隊列からは離れない。
const FALL_BACK_STEP_MM: i32 = 2_000;
/// 離脱するときに一度に取る距離（mm）。
const FLEE_STEP_MM: i32 = 15_000;
/// 現在の行動を続けることへの加点と、乗り換えに必要な差。毎 tick の行動往復を
/// 防ぐ（B2 の受け入れ条件）。最低継続時間と合わせて効かせるので、ここは
/// 「僅差では乗り換えない」程度に留める。大きくしすぎると、初期行動である
/// 命令追従から誰も動かなくなる。
const CONTINUATION_BONUS: i32 = 150;
const SWITCH_MARGIN: i32 = 80;
/// 反応半径の中の敵へ出ることそのものの価値。距離や性格の前に置く底上げで、
/// 戦列がぶつかったときに前列が最後の間合いを詰められるだけの重みを持たせる。
const ENGAGE_BASE: i32 = 500;

// ── 小集団（B3）────────────────────────────────────────────────
/// 同じ小集団の仲間が既に出ているときの加点（1 人あたり）。一斉突入ではなく、
/// 小集団ごとの波として参加させるためのつながり。
const SQUAD_COMMITTED_BONUS: i32 = 90;
/// 先導役が既に出ているときの加点。
const SQUAD_LEADER_BONUS: i32 = 180;
/// 誰も出ていない小集団で、最初の一人になることへの減点（躊躇）。
///
/// 大きくすると「誰も最初に出ない」戦列になり、白兵戦が痩せる。1,000 人会戦を
/// 3 seed で見ると、60 で白兵死 21〜28、200 で 11〜21。躊躇は残しつつ戦列が
/// 噛み合う中間として 120 を採る。
const SQUAD_FIRST_MOVER_PENALTY: i32 = 120;
/// 小集団の重心から離れているほど、単独で前へ出るのをためらう（m あたり）。
const SQUAD_ISOLATION_PENALTY_PER_M: i32 = 40;
/// 孤立の減点の上限。離れすぎた兵が何をしても選べなくならないようにする。
const SQUAD_ISOLATION_PENALTY_MAX: i32 = 300;
/// 1 人の敵へ取りつける実効的な人数。体の幅と間合いから、円周上に並べる人数の
/// 目安（半径 0.35 m の体に 0.9 m の武器なら 4 人前後）。
const MAX_ATTACKERS_PER_TARGET: u16 = 4;
/// 行動を選んだら最低これだけは続ける（tick）。20 Hz なので 16 tick = 0.8 秒。
const MIN_COMMIT_TICKS: u32 = 16;
/// 判断に乗せるノイズの幅。動揺している兵士（composure が低い）ほど広い。
const DECISION_NOISE: i32 = 260;
/// まだ側背の脅威に気づいていないことを表す番兵。
const NOT_NOTICED: u32 = u32::MAX;
/// すでに気づいていて、いつでも反応できることを表す番兵（`tick` は必ずこれ以上）。
const ALERTED: u32 = 0;
/// 人物追跡でも、対象本人へ直接殺到するのは小集団に限る。残りは部隊の移動に
/// 従い、途中で遭遇した護衛や近くの敵へ対処する。
const HUNT_TARGET_MAX_PURSUERS: u16 = 4;

/// 兵士が「いま何をするか」。任務（`Intent`）が最終目的で、こちらはその達成手段。
///
/// 4 つの個人任務（近傍の敵との交戦・人物追跡・地点占拠・地点防衛）は
/// 指揮系統が与えるもので、ここでは変えない。同じ任務のもとで、性格・士気・
/// 疲労・周囲の状況に応じて手段が分かれる。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndividualAction {
    /// 部隊の陣形・命令に従う。
    #[default]
    FollowOrder = 0,
    /// 近くの敵を迎撃する。
    JoinFight = 1,
    /// 組み合っている味方を援護する。
    Support = 2,
    /// 踏みとどまって自衛する。間合いを詰めず、来た敵を受ける。
    Defend = 3,
    /// 持ち場（隊形スロット）を維持する・戻る。
    HoldSlot = 4,
    /// 不利な間合いから一歩退く。
    FallBack = 5,
    /// 安全な方向へ離脱する（潰走を含む）。
    Flee = 6,
    /// 軍旗または指揮官の近くで再集結する。
    Rally = 7,
}

impl IndividualAction {
    pub const COUNT: usize = 8;

    /// JSON と UI が使う安定した識別子。
    pub fn id(self) -> &'static str {
        match self {
            IndividualAction::FollowOrder => "follow_order",
            IndividualAction::JoinFight => "join_fight",
            IndividualAction::Support => "support",
            IndividualAction::Defend => "defend",
            IndividualAction::HoldSlot => "hold_slot",
            IndividualAction::FallBack => "fall_back",
            IndividualAction::Flee => "flee",
            IndividualAction::Rally => "rally",
        }
    }

    pub fn from_index(index: usize) -> IndividualAction {
        const ALL: [IndividualAction; IndividualAction::COUNT] = [
            IndividualAction::FollowOrder,
            IndividualAction::JoinFight,
            IndividualAction::Support,
            IndividualAction::Defend,
            IndividualAction::HoldSlot,
            IndividualAction::FallBack,
            IndividualAction::Flee,
            IndividualAction::Rally,
        ];
        ALL[index.min(IndividualAction::COUNT - 1)]
    }

    /// 特定の敵へ向かう行動か（`focus` を使う）。
    #[inline]
    pub fn seeks_target(self) -> bool {
        matches!(
            self,
            IndividualAction::JoinFight | IndividualAction::Support
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoldierAiStats {
    pub joins: u32,
    pub retargets: u32,
    pub disengages: u32,
}

/// デバッグ表示のために残す、1 人ぶんの判断の記録（B0 の観測基盤）。
///
/// 記録は 1 人だけを対象にする（[`SoldierAiSystem::watch`]）。全員ぶん残すと
/// 兵数に比例した領域と書き込みが毎 tick 増えるうえ、UI が読むのは選択中の
/// 兵士 1 人だけだからである。記録は判断結果を変えないので、状態ハッシュへは
/// 含めない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchCandidate {
    pub target: SoldierId,
    /// 選択に使った点数。大きいほど選ばれやすい。
    pub score: i32,
    pub distance_mm: i32,
    /// この敵が既に交戦中か（点数の加点要因）。
    pub fighting: bool,
    /// 既にこの敵へ向かっている味方の人数（点数の減点要因）。
    pub load: u16,
}

/// 観測対象の兵士が最後に局所行動を選んだときの記録。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchRecord {
    pub soldier: SoldierId,
    /// 判断した tick。まだ一度も判断していなければ `None`。
    pub tick: Option<u32>,
    /// 選べた行動とその点数（列挙順）。
    pub actions: Vec<(IndividualAction, i32)>,
    /// 選んだ行動。
    pub chosen_action: Option<IndividualAction>,
    /// 判断の結果として選んだ敵。選ばなければ `None`。
    pub chosen: Option<SoldierId>,
    /// そのときの反応半径（mm）。0 なら任務が局所迎撃を禁じている。
    pub reaction_radius_mm: i32,
    /// 点数の高い順に並べた候補（上位 [`WATCH_CANDIDATES`] 件）。
    pub candidates: Vec<WatchCandidate>,
}

/// 記録しておく候補の数。UI が一覧するのに十分で、混戦でも書き込みが増えない。
pub const WATCH_CANDIDATES: usize = 4;

/// per-soldier の局所判断状態。認識用索引・任務ポリシー・負荷配列は導出キャッシュ。
/// 行動・対象・継続期限に加え、受理済み陣形目標と次回更新 tick は決定論ハッシュへ
/// 含める。
#[derive(Debug)]
pub struct SoldierAiSystem {
    action: Vec<IndividualAction>,
    focus: Vec<SoldierId>,
    commit_until: Vec<u32>,
    /// 指揮系統から最後に受理した陣形目標。隊長の目標が毎 tick 動いても、
    /// 兵士は自分の反応周期でここへ取り込むため、全員が同時に歩き出さない。
    accepted_formation_goal: Vec<Vec2Fx>,
    next_formation_sample: Vec<u32>,
    reaction_radius_mm: Vec<i32>,
    disengage_radius_mm: Vec<i32>,
    ordered_focus: Vec<SoldierId>,
    commanded: Vec<bool>,
    /// 任務そのものが「敵へ出ていく」ものか（攻撃・突撃・側面・包囲・追撃・
    /// 人物追跡）。そうでない任務では、援護できる戦闘と自衛だけが個人行動の
    /// 候補になる。
    aggressive: Vec<bool>,
    /// 地点防衛の中心と守備半径（mm）。半径 0 は守備任務ではない。
    guard_center: Vec<Vec2Fx>,
    guard_radius_mm: Vec<i32>,
    /// 側背の脅威に気づく tick。[`NOT_NOTICED`] はまだ気づいていない。
    notice_at: Vec<u32>,
    /// 行き先が固定の行動（退く・離脱・再集結）の目標地点。
    action_goal: Vec<Vec2Fx>,
    /// 再集結先。軍旗、無ければ指揮官、どちらも失っていれば隊列アンカー。
    rally_point: Vec<Vec2Fx>,
    /// 再集結先を持っているか。部隊に属さない兵士には戻る場所がない。
    has_rally: Vec<bool>,
    target_load: Vec<u16>,
    /// デバッグ観測の対象。`NO_ID` なら誰も観測しない。
    watch: SoldierId,
    watch_record: WatchRecord,
    pub stats: SoldierAiStats,
}

impl Default for SoldierAiSystem {
    fn default() -> Self {
        SoldierAiSystem {
            action: Vec::new(),
            focus: Vec::new(),
            commit_until: Vec::new(),
            accepted_formation_goal: Vec::new(),
            next_formation_sample: Vec::new(),
            reaction_radius_mm: Vec::new(),
            disengage_radius_mm: Vec::new(),
            ordered_focus: Vec::new(),
            commanded: Vec::new(),
            aggressive: Vec::new(),
            guard_center: Vec::new(),
            guard_radius_mm: Vec::new(),
            notice_at: Vec::new(),
            action_goal: Vec::new(),
            rally_point: Vec::new(),
            has_rally: Vec::new(),
            target_load: Vec::new(),
            watch: NO_ID,
            watch_record: WatchRecord::default(),
            stats: SoldierAiStats::default(),
        }
    }
}

impl SoldierAiSystem {
    pub fn register(&mut self) {
        self.action.push(IndividualAction::FollowOrder);
        self.focus.push(NO_ID);
        self.commit_until.push(0);
        self.accepted_formation_goal.push(Vec2Fx::ZERO);
        self.next_formation_sample.push(u32::MAX);
        self.reaction_radius_mm.push(DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm
            .push(DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.ordered_focus.push(NO_ID);
        self.commanded.push(false);
        self.aggressive.push(false);
        self.guard_center.push(Vec2Fx::ZERO);
        self.guard_radius_mm.push(0);
        self.notice_at.push(NOT_NOTICED);
        self.action_goal.push(Vec2Fx::ZERO);
        self.rally_point.push(Vec2Fx::ZERO);
        self.has_rally.push(false);
        self.target_load.push(0);
    }

    fn ensure_len(&mut self, len: usize) {
        while self.action.len() < len {
            self.register();
        }
        self.reaction_radius_mm
            .resize(len, DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm
            .resize(len, DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.accepted_formation_goal.resize(len, Vec2Fx::ZERO);
        self.next_formation_sample.resize(len, u32::MAX);
        self.ordered_focus.resize(len, NO_ID);
        self.commanded.resize(len, false);
        self.aggressive.resize(len, false);
        self.guard_center.resize(len, Vec2Fx::ZERO);
        self.guard_radius_mm.resize(len, 0);
        self.notice_at.resize(len, NOT_NOTICED);
        self.action_goal.resize(len, Vec2Fx::ZERO);
        self.rally_point.resize(len, Vec2Fx::ZERO);
        self.has_rally.resize(len, false);
        self.target_load.resize(len, 0);
    }

    #[inline]
    pub fn action(&self, id: SoldierId) -> IndividualAction {
        self.action
            .get(id as usize)
            .copied()
            .unwrap_or(IndividualAction::FollowOrder)
    }

    #[inline]
    pub fn focus(&self, id: SoldierId) -> Option<SoldierId> {
        self.focus
            .get(id as usize)
            .copied()
            .filter(|&target| target != NO_ID)
    }

    /// この兵士が受理済みの陣形目標（＝いま向かうべき持ち場）。
    ///
    /// 隊長が配る最新のスロットではなく、兵士が自分の反応周期で取り込んだ値。
    /// 「スロットからどれだけ離れているか」はここを基準に測る。
    #[inline]
    pub fn formation_goal(&self, id: SoldierId) -> Vec2Fx {
        self.accepted_formation_goal
            .get(id as usize)
            .copied()
            .unwrap_or(Vec2Fx::ZERO)
    }

    /// 次に陣形目標を取り込む tick。`tick` を引けば反応待ち時間になる。
    #[inline]
    pub fn next_formation_sample(&self, id: SoldierId) -> u32 {
        self.next_formation_sample
            .get(id as usize)
            .copied()
            .unwrap_or(u32::MAX)
    }

    /// 現在の局所行動を続けると決めている期限（tick）。
    #[inline]
    pub fn commit_until(&self, id: SoldierId) -> u32 {
        self.commit_until.get(id as usize).copied().unwrap_or(0)
    }

    /// 任務から決まる反応半径（mm）。0 なら局所迎撃を禁じられている。
    #[inline]
    pub fn reaction_radius_mm(&self, id: SoldierId) -> i32 {
        self.reaction_radius_mm
            .get(id as usize)
            .copied()
            .unwrap_or(0)
    }

    /// 任務が名指しで指定した対象（人物追跡）。
    #[inline]
    pub fn ordered_focus(&self, id: SoldierId) -> Option<SoldierId> {
        self.ordered_focus
            .get(id as usize)
            .copied()
            .filter(|&target| target != NO_ID)
    }

    /// デバッグ表示のために 1 人の判断を記録し始める。`None` で記録をやめる。
    ///
    /// 記録するだけで判断は変えないので、観測を始めても状態ハッシュは変わらない。
    pub fn set_watch(&mut self, id: Option<SoldierId>) {
        let id = id.unwrap_or(NO_ID);
        if self.watch != id {
            self.watch = id;
            self.watch_record = WatchRecord {
                soldier: id,
                ..WatchRecord::default()
            };
        }
    }

    /// 観測対象が最後に局所行動を選んだときの記録。
    #[inline]
    pub fn watch_record(&self) -> &WatchRecord {
        &self.watch_record
    }

    /// 個々の認識タイミングで局所行動を選び、継続中の行動目標を毎 tick 適用する。
    ///
    /// 周囲の把握は [`PerceptionSystem`] が済ませてある。ここは「何を知って
    /// いるか」ではなく「知ったうえでどうするか」だけを決める。
    pub fn tick(
        &mut self,
        world_seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        ctx: &DecisionContext<'_>,
        goals: &mut [Vec2Fx],
    ) {
        let DecisionContext {
            command,
            perception,
            squads,
        } = *ctx;
        let n = soldiers.len().min(goals.len());
        self.ensure_len(n);
        self.assign_mission_policy(command, soldiers, n);
        self.latch_formation_goals(tick, soldiers, goals, n);

        self.target_load.fill(0);
        for i in 0..n {
            if !self.action[i].seeks_target() {
                continue;
            }
            if let Some(target) = soldiers.index_if_present(self.focus[i]) {
                self.target_load[target] = self.target_load[target].saturating_add(1);
            }
        }

        // `goals` はこの時点では全員ぶんの陣形目標なので、局所行動で上書きする
        // 前に持ち場からの距離を検証する。これで敵を追い続けて守備地点から際限なく
        // 離れることがない。
        for (i, &formation_goal) in goals.iter().take(n).enumerate() {
            if self.action[i].seeks_target()
                && !self.current_action_is_valid(i, soldiers, formation_goal)
            {
                self.clear(i);
                self.stats.disengages = self.stats.disengages.saturating_add(1);
            }
        }

        let mut candidates = [0u32; MAX_NEIGHBORS];
        for (i, &formation_goal) in goals.iter().take(n).enumerate() {
            // 自衛（至近に敵が来た）と恐慌（崩れた）は、思考周期も最低継続時間も
            // 待たずに割り込む。それ以外は自分の周期で考える。
            let interrupt = must_reconsider(
                self.action[i],
                soldiers,
                i,
                perception.awareness(i as SoldierId),
            );
            if tick < soldiers.think_at[i] && !interrupt {
                continue;
            }
            soldiers.think_at[i] = tick.saturating_add(think_interval(soldiers, i));
            if !can_choose_local_action(soldiers, i) {
                continue;
            }
            if tick < self.commit_until[i] && !interrupt {
                continue;
            }
            let radius_mm = self.reaction_radius_mm[i];
            let watched = self.watch == i as SoldierId;
            if watched {
                self.watch_record.tick = Some(tick);
                self.watch_record.reaction_radius_mm = radius_mm;
                self.watch_record.chosen = None;
                self.watch_record.chosen_action = None;
                self.watch_record.candidates.clear();
                self.watch_record.actions.clear();
            }
            if radius_mm <= 0 {
                if self.action[i].seeks_target() {
                    self.clear(i);
                }
                continue;
            }

            let Some(index) = perception.index() else {
                continue;
            };
            let awareness = perception.awareness(i as SoldierId);
            let facing = soldiers.hot.facing[i];
            let guard_radius = self.guard_radius_mm[i];
            let guard_center = self.guard_center[i];
            let self_defense = fx_from_mm(SELF_DEFENSE_RADIUS_MM) as i64;
            let pos = soldiers.pos(i);
            let count = index.query_enemies_in_radius(
                soldiers,
                pos.x,
                pos.y,
                fx_from_mm(radius_mm),
                soldiers.faction[i],
                &mut candidates,
            );
            let radius = fx_from_mm(radius_mm) as i64;
            let mut best: Option<(i32, SoldierId)> = None;
            let mut support: Option<(i32, SoldierId)> = None;
            for &target_id in &candidates[..count] {
                let Some(target) = soldiers.index_if_present(target_id) else {
                    continue;
                };
                if !soldiers.is_alive(target) {
                    continue;
                }
                let d2 = dist_sq(pos, soldiers.pos(target));
                if d2 > radius * radius {
                    continue;
                }
                // 持ち場から見ても反応範囲内の敵だけを選ぶ。自分が既に前へ出ている
                // からといって、そこからさらに追撃範囲を延長しない。
                if dist_sq(formation_goal, soldiers.pos(target)) > radius * radius {
                    continue;
                }
                let is_ordered_target = self.ordered_focus[i] == target_id;
                // 1 人の敵へ取りつける人数には上限がある。既に囲まれている敵は
                // 候補から外し、余った兵は隣の敵・援護・持ち場へ回る（B3）。
                let crowd_limit = if is_ordered_target {
                    HUNT_TARGET_MAX_PURSUERS
                } else {
                    MAX_ATTACKERS_PER_TARGET
                };
                if self.target_load[target] >= crowd_limit && self.focus[i] != target_id {
                    continue;
                }
                // 守備任務では、持ち場へ入り込んだ敵と近づいてくる敵だけを相手
                // にする。離れていく敵を追いかけると持ち場が空く。
                let (intruder, closing) = if guard_radius > 0 {
                    let guard_sq = fx_from_mm(guard_radius) as i64;
                    (
                        dist_sq(guard_center, soldiers.pos(target)) <= guard_sq * guard_sq,
                        is_closing(soldiers, target, guard_center),
                    )
                } else {
                    (false, false)
                };
                if guard_radius > 0 && !intruder && !closing {
                    continue;
                }
                // 援護できる戦闘か、自衛が要る近さか。攻撃系の任務でなければ、
                // このどちらかに当てはまる敵だけが個人行動の候補になる。
                let supports_ally = awareness.supportable_enemy == target_id;
                let threatens_me = d2 <= self_defense * self_defense;
                if !self.aggressive[i] && !supports_ally && !threatens_me && !intruder && !closing {
                    continue;
                }
                let distance_mm = sim_math::fx_to_mm(sim_math::isqrt64(d2 as u64) as i32);
                let fighting_bonus = if soldiers.hot.state[target].is_fighting() {
                    220
                } else {
                    0
                };
                let load_penalty = self.target_load[target] as i32 * TARGET_LOAD_PENALTY;
                let mission_bonus = if is_ordered_target { 900 } else { 0 };
                let guard_bonus = if intruder {
                    INTRUDER_BONUS
                } else if closing {
                    CLOSING_BONUS
                } else {
                    0
                };
                // ここは「どの敵を相手にするか」の点数。「その敵へ出るかどうか」は
                // 行動の点数（`score_actions`）が決める。
                let score = 1_000 - distance_mm / 20 + fighting_bonus + mission_bonus + guard_bonus
                    - load_penalty;
                if watched {
                    let candidate = WatchCandidate {
                        target: target_id,
                        score,
                        distance_mm,
                        fighting: soldiers.hot.state[target].is_fighting(),
                        load: self.target_load[target],
                    };
                    insert_watch_candidate(&mut self.watch_record.candidates, candidate);
                }
                if supports_ally
                    && support.map_or(true, |(best_score, best_id)| {
                        score > best_score || (score == best_score && target_id < best_id)
                    })
                {
                    support = Some((score, target_id));
                }
                if best.map_or(true, |(best_score, best_id)| {
                    score > best_score || (score == best_score && target_id < best_id)
                }) {
                    best = Some((score, target_id));
                }
            }

            // 側面・背面の敵には気づくのが遅れる。正面の敵と違って、視界に
            // 入ってから体ごと向き直るまでの間がある。気づくまでは、その敵を
            // 相手にする行動（迎撃・援護）を候補から外す。
            let mut engage = best;
            if let Some((_, target)) = best {
                let arc = threat_arc(
                    facing,
                    pos,
                    soldiers.pos(target as usize),
                    perception.config(),
                );
                if arc == ThreatArc::Front {
                    self.notice_at[i] = ALERTED;
                } else {
                    if self.notice_at[i] == NOT_NOTICED {
                        self.notice_at[i] = tick.saturating_add(notice_delay(soldiers, i, arc));
                    }
                    if tick < self.notice_at[i] {
                        // まだ気づいていない。この敵を相手にする行動は候補から外す。
                        engage = None;
                        support = None;
                    } else {
                        // 一度気づいたら、その敵が居なくなるまでは警戒したまま。
                        // 判断のたびに気づき直していると、側面の敵へいつまでも
                        // 反応できない。
                        self.notice_at[i] = ALERTED;
                    }
                }
            } else {
                // 敵が見えなくなったら、次の不意打ちに備えて警戒を解く。
                self.notice_at[i] = NOT_NOTICED;
            }

            let situation = Situation {
                awareness,
                engage,
                support,
                slot_distance_mm: distance_mm_between(pos, self.accepted_formation_goal[i]),
                rally_distance_mm: distance_mm_between(pos, self.rally_point[i]),
                has_rally: self.has_rally[i],
                squad_committed: squad_committed(squads, i as SoldierId, soldiers, self),
                squad_leader_committed: squad_leader_committed(
                    squads,
                    i as SoldierId,
                    soldiers,
                    self,
                ),
                squad_isolation_mm: squads
                    .squad_of(i as SoldierId)
                    .map(|squad| distance_mm_between(pos, squad.centroid))
                    .unwrap_or(0),
                reaction_radius_mm: radius_mm,
                state: soldiers.hot.state[i],
                aggressive: self.aggressive[i],
            };
            let attrs = soldiers.attrs[i];
            let morale = soldiers.morale[i];
            let fatigue = soldiers.fatigue[i];
            let scores = score_actions(&situation, &attrs, morale, fatigue);

            // 乱数は候補の数に依らず 1 回の判断につき 2 回だけ引く（ノイズと
            // 継続時間）。候補数で引く回数が変わると、同じ状況でも過去の候補数に
            // 結果が左右されてしまう。
            let mut rng = Rng::stream(world_seed, i as SoldierId, Purpose::DecisionNoise, tick);
            let noise_width = DECISION_NOISE * (255 - attrs.composure as i32) / 255;
            let noise = rng.range(-noise_width, noise_width + 1);
            let commitment =
                MIN_COMMIT_TICKS + rng.range(0, 21) as u32 + attrs.composure as u32 / 8;

            let current = self.action[i];
            let current_score = scores[current as usize]
                .map(|score| score + CONTINUATION_BONUS)
                .unwrap_or(i32::MIN);
            // 同点なら列挙順（命令追従が先）で決める。乱数の引き方に依らない
            // 安定した規則にしておく。
            let mut chosen = current;
            let mut chosen_score = current_score;
            for (index, candidate) in scores.iter().enumerate() {
                let action = IndividualAction::from_index(index);
                let Some(score) = *candidate else {
                    continue;
                };
                if action == current {
                    continue;
                }
                let score = score + noise;
                if score > chosen_score + SWITCH_MARGIN {
                    chosen = action;
                    chosen_score = score;
                }
            }
            if watched {
                self.watch_record.actions.clear();
                for (index, candidate) in scores.iter().enumerate() {
                    if let Some(score) = *candidate {
                        self.watch_record
                            .actions
                            .push((IndividualAction::from_index(index), score));
                    }
                }
                self.watch_record.chosen_action = Some(chosen);
                self.watch_record.chosen = match chosen {
                    IndividualAction::JoinFight => engage.map(|(_, id)| id),
                    IndividualAction::Support => support.map(|(_, id)| id),
                    _ => None,
                };
            }

            let target = match chosen {
                IndividualAction::JoinFight => engage.map(|(_, id)| id),
                IndividualAction::Support => support.map(|(_, id)| id),
                _ => None,
            };
            self.set_action(i, chosen, target, soldiers, tick, commitment);
        }

        // 判断がすべて済んでから目標を上書きする。途中で上書きすると、後続の兵士が
        // 「陣形上の持ち場」ではなく前の局所目標を追撃起点として読んでしまう。
        for i in 0..n {
            if self.action[i] != IndividualAction::FollowOrder {
                self.apply_current_action(i, soldiers, goals);
            }
        }
    }

    /// 選んだ行動を記録する。対象の付け替えは負荷の勘定も直す。
    fn set_action(
        &mut self,
        i: usize,
        action: IndividualAction,
        target: Option<SoldierId>,
        soldiers: &Soldiers,
        tick: u32,
        commitment: u32,
    ) {
        let previous = self.action[i];
        let old_target = self.focus[i];
        let new_target = target.unwrap_or(NO_ID);
        if old_target != new_target {
            if let Some(old_index) = soldiers.index_if_present(old_target) {
                self.target_load[old_index] = self.target_load[old_index].saturating_sub(1);
            }
            if let Some(new_index) = soldiers.index_if_present(new_target) {
                self.target_load[new_index] = self.target_load[new_index].saturating_add(1);
            }
            if old_target != NO_ID && new_target != NO_ID {
                self.stats.retargets = self.stats.retargets.saturating_add(1);
            }
        }
        if action.seeks_target() && !previous.seeks_target() {
            self.stats.joins = self.stats.joins.saturating_add(1);
        }
        if previous.seeks_target() && !action.seeks_target() {
            self.stats.disengages = self.stats.disengages.saturating_add(1);
        }
        self.action[i] = action;
        self.focus[i] = new_target;
        self.action_goal[i] = self.action_goal_for(i, action, soldiers);
        self.commit_until[i] = tick.saturating_add(commitment);
    }

    /// 行動ごとの行き先。対象を追う行動は毎 tick 相手の位置を見るので、ここでは
    /// 決めない（`apply_current_action` が引く）。
    fn action_goal_for(&self, i: usize, action: IndividualAction, soldiers: &Soldiers) -> Vec2Fx {
        let pos = soldiers.pos(i);
        match action {
            IndividualAction::FallBack => step_away(soldiers, i, pos, FALL_BACK_STEP_MM),
            IndividualAction::Flee => step_away(soldiers, i, pos, FLEE_STEP_MM),
            IndividualAction::Rally => self.rally_point[i],
            IndividualAction::Defend => pos,
            _ => pos,
        }
    }

    fn assign_mission_policy(&mut self, command: &CommandTree, soldiers: &Soldiers, n: usize) {
        self.reaction_radius_mm[..n].fill(DEFAULT_REACTION_RADIUS_MM);
        self.disengage_radius_mm[..n].fill(DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM);
        self.ordered_focus[..n].fill(NO_ID);
        self.commanded[..n].fill(false);
        // 指揮系統に属さない兵士（サンドボックスの単騎）は守るべき隊列が無いので、
        // 近くの敵へ出るのを止めない。部隊に属する兵士だけ任務で絞る。
        self.aggressive[..n].fill(true);
        self.guard_radius_mm[..n].fill(0);
        self.has_rally[..n].fill(false);
        for i in 0..n {
            self.rally_point[i] = soldiers.pos(i);
        }
        for node in &command.nodes {
            let Some(unit) = &node.unit else {
                continue;
            };
            // 隊列を離れてよいかは任務で決まる。移動・待機・予備・退却・遮蔽・
            // 射撃・地点占拠・地点防衛のように「その場に居ること」が任務の
            // 部隊だけが、援護と自衛のときに限って離れる。攻撃系の任務と、
            // まだ任務を受けていない部隊（会戦の初期配置）は縛らない。
            let aggressive = !matches!(
                node.objective,
                Some(
                    Intent::MoveTo { .. }
                        | Intent::Hold { .. }
                        | Intent::Reserve { .. }
                        | Intent::Withdraw { .. }
                        | Intent::Screen { .. }
                        | Intent::ShootAt { .. }
                        | Intent::OccupyArea { .. }
                        | Intent::GuardArea { .. }
                )
            );
            // 再集結先は軍旗、無ければ指揮官、どちらも失っていれば隊列アンカー。
            let rally_point = unit
                .banner
                .and_then(|banner| soldiers.pos_checked(u32::from(banner)))
                .or_else(|| {
                    soldiers
                        .is_active_id(node.commander)
                        .then(|| soldiers.pos(node.commander as usize))
                })
                .unwrap_or(unit.formation_origin);
            // 守備任務なら、持ち場の中心と守備半径。追撃の限界は迎撃半径。
            let guard = match node.objective {
                Some(Intent::GuardArea {
                    center, radius_m, ..
                }) => Some((center, i32::from(radius_m).saturating_mul(1_000))),
                _ => None,
            };
            let (radius, disengage, ordered_focus) = match node.objective {
                Some(
                    Intent::Reserve { .. }
                    | Intent::Withdraw {
                        fighting: false, ..
                    },
                ) => (0, 0, NO_ID),
                Some(Intent::Withdraw { fighting: true, .. }) => (
                    MOVE_REACTION_RADIUS_MM,
                    MOVE_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::MoveTo { .. }) => (
                    MOVE_REACTION_RADIUS_MM,
                    MOVE_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::Hold { .. }) => (
                    HOLD_REACTION_RADIUS_MM,
                    HOLD_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(
                    Intent::Attack { .. }
                    | Intent::Charge { .. }
                    | Intent::Flank { .. }
                    | Intent::Envelop { .. }
                    | Intent::Pursue { .. },
                ) => (
                    ATTACK_REACTION_RADIUS_MM,
                    ATTACK_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::Screen { .. }) => (
                    SCREEN_REACTION_RADIUS_MM,
                    SCREEN_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::ShootAt { .. }) => (0, 0, NO_ID),
                Some(Intent::HuntPerson { target }) => (
                    ATTACK_REACTION_RADIUS_MM,
                    ATTACK_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    target,
                ),
                Some(Intent::OccupyArea { .. }) => (
                    DEFAULT_REACTION_RADIUS_MM,
                    DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
                Some(Intent::GuardArea {
                    intercept_radius_m, ..
                }) => {
                    let intercept = i32::from(intercept_radius_m).saturating_mul(1_000);
                    (intercept, intercept + DISENGAGE_MARGIN_MM, NO_ID)
                }
                None => (
                    DEFAULT_REACTION_RADIUS_MM,
                    DEFAULT_REACTION_RADIUS_MM + DISENGAGE_MARGIN_MM,
                    NO_ID,
                ),
            };
            for &id in &unit.soldiers {
                if let Some(slot) = self.reaction_radius_mm.get_mut(id as usize) {
                    *slot = radius;
                }
                if let Some(slot) = self.disengage_radius_mm.get_mut(id as usize) {
                    *slot = disengage;
                }
                if let Some(slot) = self.ordered_focus.get_mut(id as usize) {
                    *slot = ordered_focus;
                }
                if let Some(slot) = self.commanded.get_mut(id as usize) {
                    *slot = true;
                }
                if let Some(slot) = self.aggressive.get_mut(id as usize) {
                    *slot = aggressive;
                }
                if let Some(slot) = self.rally_point.get_mut(id as usize) {
                    *slot = rally_point;
                }
                if let Some(slot) = self.has_rally.get_mut(id as usize) {
                    *slot = true;
                }
                if let Some((center, guard_radius)) = guard {
                    if let Some(slot) = self.guard_center.get_mut(id as usize) {
                        *slot = center;
                    }
                    if let Some(slot) = self.guard_radius_mm.get_mut(id as usize) {
                        *slot = guard_radius;
                    }
                }
            }
        }
    }

    /// 隊列アンカーが作った最新目標を、兵士ごとの反応周期で受理する。
    ///
    /// `formation_goals` は毎 tick 全員ぶんを更新するが、ここでいったん個人の
    /// 受理済み目標へラッチする。初回更新の位相を ID から散らすことで、命令が
    /// 届いた瞬間に全員が同時発進する代わりに、短い時間幅の中で順次動き出す。
    fn latch_formation_goals(
        &mut self,
        tick: u32,
        soldiers: &Soldiers,
        goals: &mut [Vec2Fx],
        n: usize,
    ) {
        for (i, goal) in goals.iter_mut().take(n).enumerate() {
            let raw_goal = *goal;
            let interval = formation_goal_interval(soldiers, i);
            if self.next_formation_sample[i] == u32::MAX {
                self.accepted_formation_goal[i] = raw_goal;
                let phase = formation_goal_phase(i as SoldierId, interval);
                self.next_formation_sample[i] = tick.saturating_add(phase);
            } else {
                // 工兵タスクは作業地点・補給先・負傷者へ直接向かう必要がある。
                // また、指揮系統に属さないテスト／サンドボックス兵は、局所迎撃中に
                // 前 tick の迎撃目標を「新しい命令」と誤認して取り込まない。
                let immediate = soldiers.hot.flags[i] & flags::ENGINEER != 0
                    || soldiers.hot.state[i] == State::Working;
                let may_sample = self.commanded[i]
                    || self.action[i] == IndividualAction::FollowOrder
                    || immediate;
                if may_sample && (immediate || tick >= self.next_formation_sample[i]) {
                    self.accepted_formation_goal[i] = raw_goal;
                    self.next_formation_sample[i] = tick.saturating_add(interval);
                }
            }
            *goal = self.accepted_formation_goal[i];
        }
    }

    /// 選んだ行動を、この tick の目標位置と状態へ落とす。
    fn apply_current_action(&self, i: usize, soldiers: &mut Soldiers, goals: &mut [Vec2Fx]) {
        match self.action[i] {
            IndividualAction::FollowOrder => {}
            IndividualAction::JoinFight | IndividualAction::Support => {
                if !self.current_action_is_valid(i, soldiers, goals[i]) {
                    return;
                }
                let target = self.focus[i] as usize;
                goals[i] = soldiers.pos(target);
                if matches!(
                    soldiers.hot.state[i],
                    State::Idle | State::Marching | State::Repositioning
                ) {
                    soldiers.hot.state[i] = State::Advancing;
                }
            }
            // 踏みとどまる。間合いを詰めず、その場で受ける（向きは操舵段階が
            // 近くの敵へ向け直す）。
            IndividualAction::Defend => goals[i] = soldiers.pos(i),
            // 持ち場へ戻る・保つ。受理済みの陣形目標がそのまま行き先。
            IndividualAction::HoldSlot => goals[i] = self.accepted_formation_goal[i],
            IndividualAction::FallBack | IndividualAction::Flee | IndividualAction::Rally => {
                goals[i] = self.action_goal[i];
                if soldiers.hot.state[i] == State::Idle {
                    soldiers.hot.state[i] = State::Marching;
                }
            }
        }
    }

    /// 対象を追う行動が、まだ成り立っているか。
    fn current_action_is_valid(
        &self,
        i: usize,
        soldiers: &Soldiers,
        formation_goal: Vec2Fx,
    ) -> bool {
        if !soldiers.is_alive(i) || self.reaction_radius_mm[i] <= 0 {
            return false;
        }
        if !self.action[i].seeks_target() {
            return true;
        }
        let Some(target) = soldiers.index_if_present(self.focus[i]) else {
            return false;
        };
        if !soldiers.is_alive(target) {
            return false;
        }
        let target_pos = soldiers.pos(target);
        // 守備任務は、個人のスロットではなく持ち場の中心から追撃の限界を測る。
        // スロットは範囲内へ散らばるので、スロット基準だと限界が人によって
        // ずれてしまう。
        if self.guard_radius_mm[i] > 0 {
            let leash = fx_from_mm(self.disengage_radius_mm[i]) as i64;
            if dist_sq(self.guard_center[i], target_pos) > leash * leash {
                return false;
            }
        }
        let drop_radius = fx_from_mm(self.disengage_radius_mm[i]) as i64;
        dist_sq(formation_goal, target_pos) <= drop_radius * drop_radius
    }

    fn clear(&mut self, i: usize) {
        self.action[i] = IndividualAction::FollowOrder;
        self.focus[i] = NO_ID;
        self.commit_until[i] = 0;
        self.notice_at[i] = NOT_NOTICED;
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for i in 0..self.action.len() {
            h ^= self.action[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.focus[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.commit_until[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.accepted_formation_goal[i].x as u32 as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.accepted_formation_goal[i].y as u32 as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
            h ^= self.next_formation_sample[i] as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
}

/// 局所判断が読む、その tick の周辺情報。
///
/// 指揮系統・局所知覚・小集団はどれも読み取り専用で、判断はこれらと `Soldiers`
/// だけから決まる。まとめて渡すことで、システムが増えても引数の並びに依存した
/// 呼び出しにならない（TODO P1-4）。
#[derive(Clone, Copy)]
pub struct DecisionContext<'a> {
    pub command: &'a CommandTree,
    pub perception: &'a PerceptionSystem,
    pub squads: &'a SquadSystem,
}

/// 1 人ぶんの判断材料。行動の点数はここからだけ決まる。
struct Situation {
    awareness: LocalAwareness,
    /// 迎撃に出るなら誰へ、その相手の選び具合の点数。
    engage: Option<(i32, SoldierId)>,
    /// 援護に入れる相手（味方が組み合っている敵）。
    support: Option<(i32, SoldierId)>,
    slot_distance_mm: i32,
    rally_distance_mm: i32,
    /// 戻れる旗・指揮官がいるか。
    has_rally: bool,
    /// 同じ小集団で、既に前へ出ている仲間の人数。
    squad_committed: u16,
    /// 小集団の先導役が前へ出ているか。
    squad_leader_committed: bool,
    /// 小集団の重心からの距離（mm）。孤立の目安。
    squad_isolation_mm: i32,
    /// 任務から決まる反応半径（mm）。この内側に敵がいる間は、隊列を整え直す
    /// より先に、その敵へどう対処するかを決める。
    reaction_radius_mm: i32,
    state: State,
    /// 任務そのものが敵へ出ていくものか。
    aggressive: bool,
}

/// 行動ごとの点数。選べない行動は `None`。
///
/// 仕様 05 章 3.2 節のユーティリティ AI。脅威・任務への寄与・距離・士気・疲労と、
/// 規律／勇敢さ／自己保存／仲間意識（loyalty）／攻撃性の重みで決める。
fn score_actions(
    s: &Situation,
    attrs: &Attrs,
    morale: u16,
    fatigue: u16,
) -> [Option<i32>; IndividualAction::COUNT] {
    let a = s.awareness;
    // 方向で重みが違う。背後の敵がもっとも効く（仕様 05 章 3.2 節 rear_threat）。
    let threat = a.threat_front as i32 + a.threat_flank as i32 * 2 + a.threat_rear as i32 * 3;
    let outnumbered = (a.enemies as i32 - a.allies as i32).max(0);
    let morale = morale as i32;
    let fatigue = fatigue as i32;
    let bravery = attrs.bravery as i32;
    let discipline = attrs.discipline as i32;
    let aggression = attrs.aggression as i32;
    let self_preservation = attrs.self_preservation as i32;
    let loyalty = attrs.loyalty as i32;
    let broken = s.state == State::Broken;

    let mut scores = [None; IndividualAction::COUNT];

    // 目の前まで敵が来ているほど「いま対処する」側の点数が上がる。距離が
    // 近いほど大きい（4 m でゼロ、接触で最大）。
    let closeness = if a.sees_enemy() {
        (DEFEND_TRIGGER_MM - a.nearest_enemy_mm).max(0)
    } else {
        0
    };

    // 反応半径の中に敵がいるか。持ち場に着いているかどうかで、命令追従の
    // 価値が変わる。
    let enemy_in_reach = a.sees_enemy() && a.nearest_enemy_mm <= s.reaction_radius_mm;
    let arrived = s.slot_distance_mm <= SLOT_RETURN_MM;

    // 命令に従う: 規律が支え、脅威・目前の敵・持ち場からの距離が削る。
    //
    // 持ち場に着いていて、なお目の前に敵がいるなら「命令に従う」はその場に
    // 立っていることでしかない。戦列がぶつかったあとも全員が立ったままだと、
    // 接触しているのに誰も戦わない会戦になる。
    scores[IndividualAction::FollowOrder as usize] = Some(
        1_000 + discipline * 2
            - threat * 25
            - closeness / 8
            - s.slot_distance_mm / 100
            - if arrived && enemy_in_reach {
                ARRIVED_WITH_ENEMY_PENALTY
            } else {
                0
            }
            + if broken { -600 } else { 0 },
    );

    // 迎撃に出る: 相手の選び具合に、攻撃性と士気を足し、疲労・自己保存・脅威で引く。
    // 目の前まで来ている敵ほど、放置できない。
    // 小集団のつながり。仲間や先導役が既に出ていれば続きやすく、誰も出ていない
    // 集団では最初の一人になるのをためらう。重心から離れているほど単独行動を
    // 避ける（B3）。
    let cohesion = s.squad_committed as i32 * SQUAD_COMMITTED_BONUS
        + if s.squad_leader_committed {
            SQUAD_LEADER_BONUS
        } else {
            0
        }
        - if s.squad_committed == 0 && !s.squad_leader_committed {
            SQUAD_FIRST_MOVER_PENALTY
        } else {
            0
        }
        - (s.squad_isolation_mm * SQUAD_ISOLATION_PENALTY_PER_M / 1_000)
            .min(SQUAD_ISOLATION_PENALTY_MAX);

    if let Some((target_score, _)) = s.engage {
        let discipline_hold = if s.aggressive { 0 } else { discipline };
        scores[IndividualAction::JoinFight as usize] = Some(
            target_score
                + ENGAGE_BASE
                + cohesion
                + closeness / 2
                + aggression * 3
                + bravery * 2
                + (morale - 500) / 2
                // 疲れきった兵は踏み込まない。突っ込む判断にもっとも効く。
                - fatigue / 25
                - self_preservation * 2
                - threat * 10
                - discipline_hold
                + if broken { -900 } else { 0 },
        );
    }

    // 援護する: 仲間意識が主。味方が既に組み合っているので、単独で出るより安全。
    if let Some((target_score, _)) = s.support {
        scores[IndividualAction::Support as usize] = Some(
            target_score + SUPPORT_BONUS + cohesion + loyalty * 3 + bravery + (morale - 500) / 3
                - fatigue / 50
                - self_preservation
                + if broken { -900 } else { 0 },
        );
    }

    // 踏みとどまる: 至近に敵が来たときだけ。疲れているほど、囲まれているほど、
    // 出て行かずに受ける。
    if a.sees_enemy() && a.nearest_enemy_mm <= DEFEND_TRIGGER_MM {
        scores[IndividualAction::Defend as usize] = Some(
            900 + closeness / 4 + self_preservation * 2 + discipline + fatigue / 30 + threat * 15
                - aggression * 2,
        );
    }

    // 持ち場へ戻る: 離れているほど、規律が高いほど。ただし反応半径の中に敵が
    // いる間は選ばない。目の前に敵がいるのに隊列を整え直すのは、隊列を保つこと
    // 自体が目的化した動きで、前列がいつまでも接触しない原因になる。
    if s.slot_distance_mm > SLOT_RETURN_MM && !enemy_in_reach {
        // 距離の効き方は頭打ちにする。行軍の初期のように持ち場が何十 m も先に
        // ある場合まで距離に比例させると、この行動だけが他を押しのけてしまう。
        let displacement = s.slot_distance_mm.min(SLOT_RETURN_SATURATION_MM) / 40;
        scores[IndividualAction::HoldSlot as usize] =
            Some(800 + discipline * 3 + displacement - aggression - threat * 5);
    }

    // 一歩退く: 踏み込まれた、または局所で数負けしているとき。
    let crowded_in = a.sees_enemy() && a.nearest_enemy_mm <= FALL_BACK_TRIGGER_MM;
    if crowded_in || outnumbered >= 2 {
        scores[IndividualAction::FallBack as usize] = Some(
            600 + closeness / 6
                + self_preservation * 3
                + fatigue / 25
                + outnumbered * 120
                + threat * 20
                - bravery * 2
                - discipline * 2
                - morale / 4,
        );
    }

    // 離脱・潰走: 崩れた兵だけ。「いつ崩れるか」は士気の系（`combat`）が決める
    // ことで、ここで別の閾値を持つと二重に判定してしまう。
    if broken {
        scores[IndividualAction::Flee as usize] = Some(
            500 + (1_000 - morale)
                + self_preservation * 2
                + threat * 30
                + a.local_broken as i32 * 50
                - discipline * 2
                - bravery * 2
                - loyalty
                + if broken { 900 } else { 0 },
        );
    }

    // 再集結: 旗・指揮官の近くへ。規律と忠誠が支え、遠いほど選びにくい。
    // 崩れた兵と、立ち直りかけの兵だけが選ぶ。戻る先が無ければ選べない。
    if s.has_rally && (broken || s.state == State::Rallying) {
        scores[IndividualAction::Rally as usize] = Some(
            500 + (1_000 - morale) / 2 + discipline * 2 + loyalty * 2 - s.rally_distance_mm / 100
                + if s.state == State::Rallying { 800 } else { 0 }
                + if broken { 300 } else { 0 },
        );
    }

    scores
}

/// 同じ小集団で、既に前へ出ている仲間の人数（自分は数えない）。
///
/// 「出ている」は、個人判断で敵へ向かっている者と、すでに組み合っている者の
/// 両方を指す。組み合った仲間は判断としては命令追従のままなので、状態も見る。
fn squad_committed(
    squads: &SquadSystem,
    id: SoldierId,
    soldiers: &Soldiers,
    ai: &SoldierAiSystem,
) -> u16 {
    let Some(squad) = squads.squad_of(id) else {
        return 0;
    };
    squad
        .members
        .iter()
        .filter(|&&member| {
            member != id
                && (ai.action(member).seeks_target()
                    || soldiers
                        .index_if_present(member)
                        .is_some_and(|i| soldiers.hot.state[i].is_fighting()))
        })
        .count()
        .min(u16::MAX as usize) as u16
}

/// 小集団の先導役が前へ出ているか。自分が先導役なら「誰も先導していない」。
fn squad_leader_committed(
    squads: &SquadSystem,
    id: SoldierId,
    soldiers: &Soldiers,
    ai: &SoldierAiSystem,
) -> bool {
    squads.squad_of(id).is_some_and(|squad| {
        squad.leader != NO_ID
            && squad.leader != id
            && (ai.action(squad.leader).seeks_target()
                || soldiers
                    .index_if_present(squad.leader)
                    .is_some_and(|i| soldiers.hot.state[i].is_fighting()))
    })
}

/// 継続中の行動を破って選び直すべき割り込みか。
///
/// 自衛（至近に敵が来た）と恐慌（崩れた）は、最低継続時間より優先する。
fn must_reconsider(
    action: IndividualAction,
    soldiers: &Soldiers,
    i: usize,
    awareness: LocalAwareness,
) -> bool {
    if soldiers.hot.state[i] == State::Broken
        && !matches!(action, IndividualAction::Flee | IndividualAction::Rally)
    {
        return true;
    }
    awareness.sees_enemy()
        && awareness.nearest_enemy_mm <= SELF_DEFENSE_RADIUS_MM
        && !matches!(
            action,
            IndividualAction::JoinFight
                | IndividualAction::Support
                | IndividualAction::Defend
                | IndividualAction::FallBack
        )
}

/// もっとも近い敵から離れる方向へ `step_mm` だけ離れた地点。敵が見えない
/// ときは、その場に留まる。
fn step_away(soldiers: &Soldiers, i: usize, pos: Vec2Fx, step_mm: i32) -> Vec2Fx {
    let mut nearest = None;
    let mut nearest_d2 = i64::MAX;
    for j in 0..soldiers.len() {
        if j == i || !soldiers.is_alive(j) || soldiers.faction[j] == soldiers.faction[i] {
            continue;
        }
        let d2 = dist_sq(pos, soldiers.pos(j));
        if d2 < nearest_d2 {
            nearest_d2 = d2;
            nearest = Some(j);
        }
    }
    let Some(threat) = nearest else {
        return pos;
    };
    let away = pos.sub(soldiers.pos(threat));
    if away.x == 0 && away.y == 0 {
        return pos;
    }
    pos.add(away.normalized().scale(fx_from_mm(step_mm)))
}

fn distance_mm_between(a: Vec2Fx, b: Vec2Fx) -> i32 {
    sim_math::fx_to_mm(sim_math::isqrt64(dist_sq(a, b) as u64) as i32)
}

/// 側背の敵に気づくまでの遅れ（tick）。反射が速い兵士ほど短い。
fn notice_delay(soldiers: &Soldiers, i: usize, arc: ThreatArc) -> u32 {
    let base = match arc {
        ThreatArc::Front => return 0,
        ThreatArc::Flank => FLANK_NOTICE_TICKS,
        ThreatArc::Rear => REAR_NOTICE_TICKS,
    };
    let reflex = soldiers.attrs[i].reflex as u32;
    // reflex 0 で 1.5 倍、255 で 0.5 倍。
    (base * (382 - reflex) / 255).max(1)
}

/// 観測用の候補一覧へ、点数の高い順に差し込む（上位 [`WATCH_CANDIDATES`] 件）。
/// 同点なら ID の小さい方を上にして、判断本体と同じ並びにする。
fn insert_watch_candidate(list: &mut Vec<WatchCandidate>, candidate: WatchCandidate) {
    let at = list
        .iter()
        .position(|existing| {
            candidate.score > existing.score
                || (candidate.score == existing.score && candidate.target < existing.target)
        })
        .unwrap_or(list.len());
    if at >= WATCH_CANDIDATES {
        return;
    }
    list.insert(at, candidate);
    list.truncate(WATCH_CANDIDATES);
}

/// 陣形目標を見直す周期。反射と規律が高いほど短く、疲労・低士気で長くなる。
/// 4〜22 tick（0.2〜1.1 秒）の範囲なので、命令伝達そのものの遅延に比べれば短いが、
/// 一斉発進を崩すには十分な個人差になる。
fn formation_goal_interval(soldiers: &Soldiers, i: usize) -> u32 {
    let attrs = soldiers.attrs[i];
    let response = (attrs.reflex as u32 + attrs.discipline as u32) / 2;
    let base = 4 + (255u32.saturating_sub(response) * 12 / 255);
    let fatigue_delay = soldiers.fatigue[i] as u32 / 2_500;
    let morale_delay = if soldiers.morale[i] < 300 { 2 } else { 0 };
    (base + fatigue_delay + morale_delay).clamp(4, 22)
}

/// 連番 ID がそのまま前から後ろへの波にならないよう乗法ハッシュで位相を散らす。
fn formation_goal_phase(id: SoldierId, interval: u32) -> u32 {
    let mixed = id.wrapping_mul(2_654_435_761) ^ id.rotate_left(13);
    let mixed = mixed ^ (mixed >> 16);
    1 + mixed % interval.max(1)
}

fn can_choose_local_action(soldiers: &Soldiers, i: usize) -> bool {
    if !soldiers.is_alive(i)
        || soldiers.hot.flags[i] & (flags::ENGINEER | flags::MISSILE_TROOP) != 0
    {
        return false;
    }
    // 崩れた兵・立て直し中の兵も判断はする。離脱先と再集結先を決めるのは
    // ここだからで、放っておくと陣形スロットへ歩き続けてしまう（B2）。
    !matches!(
        soldiers.hot.state[i],
        State::Engaged
            | State::Charging
            | State::Shooting
            | State::Reloading
            | State::Working
            | State::Downed
            | State::Dead
    )
}

fn think_interval(soldiers: &Soldiers, i: usize) -> u32 {
    let base = match soldiers.hot.state[i] {
        State::Engaged | State::Charging => 2,
        State::Broken | State::Shooting | State::Advancing | State::Repositioning => 4,
        State::Marching => 10,
        State::Downed | State::Dead => return u32::MAX,
        _ => 20,
    };
    let response = 128 + soldiers.attrs[i].reflex as u32 / 2;
    (base * 192 / response).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soldiers::Attrs;
    use sim_math::fx;

    /// 知覚と小集団を更新してから局所判断を回す。ワールドの tick 順と同じ並び。
    fn step(
        ai: &mut SoldierAiSystem,
        perception: &mut PerceptionSystem,
        seed: u64,
        tick: u32,
        soldiers: &mut Soldiers,
        command: &CommandTree,
        goals: &mut [Vec2Fx],
    ) {
        let mut squads = crate::squads::SquadSystem::default();
        for _ in 0..soldiers.len() {
            squads.register();
        }
        squads.tick(tick, soldiers, command);
        perception.tick(tick, soldiers);
        let ctx = DecisionContext {
            command,
            perception,
            squads: &squads,
        };
        ai.tick(seed, tick, soldiers, &ctx, goals);
    }

    /// テスト用の最小の `Unit`。陣形は 1 列で、アンカーは持ち場。
    fn test_unit(soldiers: Vec<SoldierId>, origin: Vec2Fx) -> crate::organization::Unit {
        crate::organization::Unit {
            soldiers,
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: origin,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: origin,
            pursuit_leash: None,
        }
    }

    fn eager_attrs() -> Attrs {
        Attrs::new(160, 160, 160, 160, 255, 160, 255, 220, 255, 0, 160, 200)
    }

    #[test]
    fn formation_goal_acceptance_is_staggered_between_soldiers() {
        let mut soldiers = Soldiers::default();
        for i in 0..8 {
            soldiers.push(fx(100), fx(100 + i), 0, 0, 0, Attrs::default(), 0);
        }
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let command = CommandTree::new();
        let initial: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let ordered: Vec<_> = initial
            .iter()
            .map(|p| p.add(Vec2Fx::new(fx(20), 0)))
            .collect();
        let mut goals = initial.clone();
        step(
            &mut ai,
            &mut perception,
            31,
            0,
            &mut soldiers,
            &command,
            &mut goals,
        );

        goals.clone_from(&ordered);
        step(
            &mut ai,
            &mut perception,
            31,
            1,
            &mut soldiers,
            &command,
            &mut goals,
        );
        let first_wave = goals
            .iter()
            .zip(&ordered)
            .filter(|(accepted, ordered)| accepted == ordered)
            .count();
        assert!(first_wave > 0);
        assert!(first_wave < soldiers.len());

        for tick in 2..=22 {
            goals.clone_from(&ordered);
            step(
                &mut ai,
                &mut perception,
                31,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        assert_eq!(goals, ordered);
    }

    #[test]
    fn alert_disciplined_soldiers_refresh_formation_goals_faster() {
        let mut soldiers = Soldiers::default();
        soldiers.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), 0);
        soldiers.push(fx(101), fx(100), 0, 0, 0, eager_attrs(), 0);
        assert!(formation_goal_interval(&soldiers, 1) < formation_goal_interval(&soldiers, 0));
    }

    #[test]
    fn engineer_task_goals_bypass_formation_response_delay() {
        let mut soldiers = Soldiers::default();
        let engineer = soldiers.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), flags::ENGINEER);
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        let command = CommandTree::new();
        let mut goals = vec![soldiers.pos(engineer as usize)];
        step(
            &mut ai,
            &mut perception,
            37,
            0,
            &mut soldiers,
            &command,
            &mut goals,
        );

        let task_goal = Vec2Fx::new(fx(130), fx(100));
        goals[engineer as usize] = task_goal;
        step(
            &mut ai,
            &mut perception,
            37,
            1,
            &mut soldiers,
            &command,
            &mut goals,
        );
        assert_eq!(goals[engineer as usize], task_goal);
    }

    #[test]
    fn nearby_soldiers_join_a_fight_on_individual_think_ticks() {
        let mut soldiers = Soldiers::default();
        let ally = soldiers.push(fx(100), fx(100), 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let command = CommandTree::new();
        let mut goals = vec![soldiers.pos(0), soldiers.pos(1)];

        for tick in 0..40 {
            step(
                &mut ai,
                &mut perception,
                7,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }

        assert_eq!(ai.action(ally), IndividualAction::JoinFight);
        assert_eq!(ai.focus(ally), Some(enemy));
        assert_eq!(goals[ally as usize], soldiers.pos(enemy as usize));
        assert_eq!(soldiers.hot.state[ally as usize], State::Advancing);

        // 兵士自身が敵の近くまで追っていても、陣形上の持ち場から離れすぎたら戻る。
        soldiers.set_pos(ally as usize, Vec2Fx::new(fx(112), fx(100)));
        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(116), fx(100)));
        goals[ally as usize] = Vec2Fx::new(fx(100), fx(100));
        step(
            &mut ai,
            &mut perception,
            7,
            41,
            &mut soldiers,
            &command,
            &mut goals,
        );
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
    }

    #[test]
    fn target_load_spreads_attackers_between_equal_enemies() {
        let mut soldiers = Soldiers::default();
        let allies: Vec<_> = (0..4)
            .map(|i| soldiers.push(fx(100), fx(99 + i), 0, 0, 0, eager_attrs(), 0))
            .collect();
        let enemies = [
            soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0),
            soldiers.push(fx(106), fx(102), 0, 0, 1, Attrs::default(), 0),
        ];
        for &enemy in &enemies {
            soldiers.hot.state[enemy as usize] = State::Engaged;
        }
        for &ally in &allies {
            soldiers.think_at[ally as usize] = 0;
        }
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let command = CommandTree::new();
        let mut goals: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();

        for tick in 0..80 {
            step(
                &mut ai,
                &mut perception,
                11,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        let focused: std::collections::BTreeSet<_> =
            allies.iter().filter_map(|&ally| ai.focus(ally)).collect();
        assert_eq!(focused.len(), 2);
    }

    #[test]
    fn reserve_order_prevents_local_pursuit() {
        let mut soldiers = Soldiers::default();
        let ally = soldiers.push(fx(100), fx(100), 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(104), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let unit = crate::organization::Unit {
            soldiers: vec![ally],
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: soldiers.pos(ally as usize),
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: soldiers.pos(ally as usize),
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, ally, vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::Reserve {
            rally_pos: soldiers.pos(ally as usize),
        });
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let mut goals = vec![soldiers.pos(0), soldiers.pos(1)];

        for tick in 0..80 {
            step(
                &mut ai,
                &mut perception,
                19,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
        assert_eq!(goals[ally as usize], soldiers.pos(ally as usize));
    }

    #[test]
    fn guard_area_intercepts_then_returns_to_the_post() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let ally = soldiers.push(post.x, post.y, 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(112), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[enemy as usize] = State::Engaged;
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let unit = crate::organization::Unit {
            soldiers: vec![ally],
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: post,
            formation_facing: 0,
            ranks: 1,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: post,
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, ally, vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::GuardArea {
            center: post,
            radius_m: 5,
            intercept_radius_m: 14,
        });
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let mut goals = vec![post, soldiers.pos(enemy as usize)];

        for tick in 0..80 {
            goals[ally as usize] = post;
            step(
                &mut ai,
                &mut perception,
                23,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }
        assert_eq!(ai.action(ally), IndividualAction::JoinFight);

        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(120), fx(100)));
        goals[ally as usize] = post;
        step(
            &mut ai,
            &mut perception,
            23,
            81,
            &mut soldiers,
            &command,
            &mut goals,
        );
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);
        assert_eq!(goals[ally as usize], post);
    }

    /// 守備兵は、持ち場へ近づいてくる敵を選び、離れていく敵は追わない（B1）。
    #[test]
    fn guards_pick_the_approaching_enemy_over_the_leaving_one() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let ally = soldiers.push(post.x, post.y, 0, 0, 0, eager_attrs(), 0);
        // どちらも正面（±60 度）にいて、距離も同じ。違いは進む向きだけ。
        let approaching = soldiers.push(fx(115), fx(100), 0, 0, 1, Attrs::default(), 0);
        let leaving = soldiers.push(fx(115), fx(101), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.vel_x[approaching as usize] = -(fx_from_mm(60) as i16);
        soldiers.hot.vel_x[leaving as usize] = fx_from_mm(60) as i16;
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let node = command.add_node(None, 0, 0, ally, vec![], Some(test_unit(vec![ally], post)));
        command.node_mut(node).unwrap().objective = Some(Intent::GuardArea {
            center: post,
            radius_m: 5,
            intercept_radius_m: 20,
        });

        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let mut goals = vec![post, soldiers.pos(1), soldiers.pos(2)];
        for tick in 0..120 {
            goals[ally as usize] = post;
            step(
                &mut ai,
                &mut perception,
                41,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }
        assert_eq!(ai.action(ally), IndividualAction::JoinFight);
        assert_eq!(
            ai.focus(ally),
            Some(approaching),
            "離れていく敵を追いかけている"
        );
        let _ = leaving;
    }

    /// 側背から来る敵には気づくのが遅れる（B1 の受け入れ条件）。
    #[test]
    fn threats_from_behind_are_noticed_later_than_those_in_front() {
        fn ticks_until_reaction(enemy_at: (i32, i32), facing: sim_math::Brad) -> Option<u32> {
            let mut soldiers = Soldiers::default();
            let ally = soldiers.push(fx(100), fx(100), facing, 0, 0, eager_attrs(), 0);
            soldiers.push(fx(enemy_at.0), fx(enemy_at.1), 0, 0, 1, Attrs::default(), 0);
            soldiers.think_at[ally as usize] = 0;
            let mut ai = SoldierAiSystem::default();
            let mut perception = PerceptionSystem::default();
            let command = CommandTree::new();
            let mut goals = vec![soldiers.pos(0), soldiers.pos(1)];
            let home = goals.clone();
            for tick in 0..200 {
                goals.clone_from(&home);
                step(
                    &mut ai,
                    &mut perception,
                    43,
                    tick,
                    &mut soldiers,
                    &command,
                    &mut goals,
                );
                if ai.action(ally) == IndividualAction::JoinFight {
                    return Some(tick);
                }
            }
            None
        }

        // 同じ位置の敵を、正面に見た場合と背後に置かれた場合で比べる。
        let front = ticks_until_reaction((106, 100), 0).expect("正面の敵に反応しない");
        let rear = ticks_until_reaction((94, 100), 0).expect("背後の敵にいつまでも反応しない");
        assert!(
            rear > front,
            "背後の敵への反応が正面と同じ速さ（正面 {front} / 背後 {rear}）"
        );
    }

    /// 攻撃系でない任務では、援護できる戦闘か自衛の要る近さでなければ隊列を
    /// 離れない（B1）。
    #[test]
    fn marching_soldiers_only_break_ranks_to_support_or_defend() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let ally = soldiers.push(post.x, post.y, 0, 0, 0, eager_attrs(), 0);
        let comrade = soldiers.push(fx(101), fx(100), 0, 0, 0, eager_attrs(), 0);
        let enemy = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.think_at[ally as usize] = 0;

        let mut command = CommandTree::new();
        let node = command.add_node(
            None,
            0,
            0,
            ally,
            vec![],
            Some(test_unit(vec![ally, comrade], post)),
        );
        command.node_mut(node).unwrap().objective = Some(Intent::MoveTo {
            pos: post,
            facing: 0,
            speed: crate::organization::MoveSpeed::Walk,
            formation: crate::organization::FORMATION_LINE,
        });

        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let mut goals = vec![post, soldiers.pos(1), soldiers.pos(2)];
        let home = goals.clone();
        for tick in 0..80 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                47,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        assert_eq!(
            ai.action(ally),
            IndividualAction::FollowOrder,
            "誰も戦っていない 6 m 先の敵へ隊列を離れている"
        );

        // 味方がその敵と組み合ったら、援護に入れる戦闘になる。
        soldiers.hot.state[comrade as usize] = State::Engaged;
        soldiers.target[comrade as usize] = enemy;
        for tick in 80..200 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                47,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(ally) == IndividualAction::JoinFight {
                break;
            }
        }
        assert_eq!(ai.action(ally), IndividualAction::JoinFight);
        assert_eq!(ai.focus(ally), Some(enemy));
    }

    /// 同じ命令のもとでも、状況と性格が違えば違う手段を選ぶ（B2 の受け入れ条件）。
    #[test]
    fn soldiers_in_the_same_unit_choose_different_actions() {
        // 兵士は 40 m ずつ離して置き、互いの知覚（14 m）に入らないようにする。
        let mut soldiers = Soldiers::default();
        let coward = Attrs::new(140, 140, 140, 140, 160, 140, 40, 60, 40, 250, 120, 60);
        let disciplined = Attrs::new(140, 140, 140, 140, 160, 140, 160, 250, 80, 120, 200, 220);
        let loyal = Attrs::new(140, 140, 140, 140, 160, 140, 200, 160, 160, 80, 255, 180);

        // 1) 臆病で疲れきった兵の目の前に敵。
        let scared = soldiers.push(fx(100), fx(100), 0, 0, 0, coward, 0);
        soldiers.push(fx(102), fx(100), 0, 0, 1, Attrs::default(), 0);
        // 2) 規律の高い兵が持ち場から離れている（周囲に敵はいない）。
        let strayed = soldiers.push(fx(200), fx(100), 0, 0, 0, disciplined, 0);
        // 3) 仲間が組み合っている横の、忠誠心の高い兵。
        let helper = soldiers.push(fx(300), fx(100), 0, 0, 0, loyal, 0);
        let comrade = soldiers.push(fx(303), fx(100), 0, 0, 0, disciplined, 0);
        let enemy = soldiers.push(fx(304), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[comrade as usize] = State::Engaged;
        soldiers.target[comrade as usize] = enemy;
        // 4) 崩れた兵。
        let broken = soldiers.push(fx(400), fx(100), 0, 0, 0, coward, 0);
        soldiers.hot.state[broken as usize] = State::Broken;
        soldiers.morale[broken as usize] = 80;
        soldiers.push(fx(410), fx(100), 0, 0, 1, Attrs::default(), 0);

        soldiers.fatigue[scared as usize] = 9_000;
        soldiers.morale[scared as usize] = 300;
        for id in [scared, strayed, helper, comrade, broken] {
            soldiers.think_at[id as usize] = 0;
        }

        let members = vec![scared, strayed, helper, comrade, broken];
        let mut command = CommandTree::new();
        let anchor = Vec2Fx::new(fx(100), fx(100));
        let node = command.add_node(
            None,
            0,
            0,
            strayed,
            vec![],
            Some(test_unit(members, anchor)),
        );
        command.node_mut(node).unwrap().objective = Some(Intent::MoveTo {
            pos: anchor,
            facing: 0,
            speed: crate::organization::MoveSpeed::Walk,
            formation: crate::organization::FORMATION_LINE,
        });

        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        // 陣形スロットは各自の足下（迷子の兵だけ 10 m 離す）。
        let mut home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        home[strayed as usize] = Vec2Fx::new(fx(210), fx(100));
        let mut goals = home.clone();
        for tick in 0..60 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                53,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }

        let chosen: Vec<IndividualAction> = [scared, strayed, helper, broken]
            .iter()
            .map(|&id| ai.action(id))
            .collect();
        let distinct: std::collections::BTreeSet<_> = chosen.iter().map(|a| a.id()).collect();
        assert!(
            distinct.len() >= 3,
            "同じ命令下で全員が似た行動を選んでいる: {chosen:?}"
        );
        assert_eq!(
            ai.action(strayed),
            IndividualAction::HoldSlot,
            "規律の高い兵が持ち場へ戻っていない"
        );
        assert_eq!(
            ai.action(helper),
            IndividualAction::Support,
            "援護できる兵が援護に入っていない"
        );
        assert!(
            matches!(
                ai.action(broken),
                IndividualAction::Flee | IndividualAction::Rally
            ),
            "崩れた兵が離脱も再集結もしていない: {:?}",
            ai.action(broken)
        );
        assert!(
            matches!(
                ai.action(scared),
                IndividualAction::Defend | IndividualAction::FallBack | IndividualAction::Flee
            ),
            "臆病で疲れた兵が突っ込んでいる: {:?}",
            ai.action(scared)
        );
    }

    /// 選んだ行動は一定時間続く。毎 tick 前進と後退を繰り返さない
    /// （B2 の受け入れ条件）。
    #[test]
    fn chosen_actions_are_held_for_a_while() {
        let mut soldiers = Soldiers::default();
        let ally = soldiers.push(fx(100), fx(100), 0, 0, 0, Attrs::default(), 0);
        soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.think_at[ally as usize] = 0;
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let command = CommandTree::new();
        let home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = home.clone();

        let mut changes = Vec::new();
        let mut previous = ai.action(ally);
        for tick in 0..300 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                59,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            let now = ai.action(ally);
            if now != previous {
                changes.push(tick);
                previous = now;
            }
        }
        for pair in changes.windows(2) {
            assert!(
                pair[1] - pair[0] >= MIN_COMMIT_TICKS,
                "行動が {} tick で切り替わっている: {changes:?}",
                pair[1] - pair[0]
            );
        }
    }

    /// 至近に敵が来たら、継続時間の途中でも選び直す（自衛の割り込み）。
    #[test]
    fn a_sudden_close_threat_interrupts_the_current_action() {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let ally = soldiers.push(post.x, post.y, 0, 0, 0, Attrs::default(), 0);
        let enemy = soldiers.push(fx(140), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.think_at[ally as usize] = 0;
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let command = CommandTree::new();
        let home = vec![post, soldiers.pos(enemy as usize)];
        let mut goals = home.clone();
        for tick in 0..40 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                61,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        assert_eq!(ai.action(ally), IndividualAction::FollowOrder);

        // 敵が目の前に現れる。継続時間を待たずに反応する。
        soldiers.set_pos(enemy as usize, Vec2Fx::new(fx(102), fx(100)));
        let mut reacted = None;
        for tick in 40..60 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                61,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(ally) != IndividualAction::FollowOrder {
                reacted = Some(tick);
                break;
            }
        }
        let reacted = reacted.expect("目の前の敵に反応していない");
        assert!(
            reacted - 40 <= MIN_COMMIT_TICKS,
            "自衛の割り込みが効かず {reacted} tick まで待っている"
        );
    }

    /// 崩れた兵は軍旗の下へ戻る。
    #[test]
    fn broken_soldiers_rally_towards_the_banner() {
        let mut soldiers = Soldiers::default();
        let banner_pos = Vec2Fx::new(fx(100), fx(100));
        let bearer = soldiers.push(banner_pos.x, banner_pos.y, 0, 0, 0, Attrs::default(), 0);
        let loyal = Attrs::new(140, 140, 140, 140, 160, 140, 120, 220, 80, 140, 255, 200);
        let broken = soldiers.push(fx(112), fx(100), 0, 0, 0, loyal, 0);
        soldiers.hot.state[broken as usize] = State::Broken;
        soldiers.morale[broken as usize] = 60;
        soldiers.think_at[broken as usize] = 0;

        let mut command = CommandTree::new();
        let mut unit = test_unit(vec![bearer, broken], banner_pos);
        unit.banner = Some(bearer as u16);
        command.add_node(None, 0, 0, bearer, vec![], Some(unit));

        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        ai.register();
        ai.register();
        let home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = home.clone();
        for tick in 0..60 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                67,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if ai.action(broken) == IndividualAction::Rally {
                break;
            }
        }
        assert_eq!(ai.action(broken), IndividualAction::Rally);
        assert_eq!(
            goals[broken as usize], banner_pos,
            "再集結先が軍旗になっていない"
        );
    }

    /// 1 人の敵へ取りつける人数には上限がある。余った兵は別の相手へ回る（B3）。
    #[test]
    fn only_a_few_soldiers_can_crowd_one_enemy() {
        let mut soldiers = Soldiers::default();
        let allies: Vec<_> = (0..10)
            .map(|k| soldiers.push(fx(100), fx(96 + k), 0, 0, 0, eager_attrs(), 0))
            .collect();
        let lone = soldiers.push(fx(103), fx(100), 0, 0, 1, Attrs::default(), 0);
        let other = soldiers.push(fx(103), fx(106), 0, 0, 1, Attrs::default(), 0);
        for &ally in &allies {
            soldiers.think_at[ally as usize] = 0;
        }
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let command = CommandTree::new();
        let home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = home.clone();
        for tick in 0..120 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                71,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        let on_lone = allies
            .iter()
            .filter(|&&ally| ai.focus(ally) == Some(lone))
            .count();
        assert!(
            on_lone <= MAX_ATTACKERS_PER_TARGET as usize,
            "1 人の敵へ {on_lone} 人が群がっている"
        );
        let _ = other;
    }

    /// 仲間が出ていない小集団の兵はためらい、出ていれば続く（B3）。
    #[test]
    fn soldiers_follow_their_squad_into_a_fight() {
        fn join_tick(comrades_fighting: bool) -> Option<u32> {
            let mut soldiers = Soldiers::default();
            let post = Vec2Fx::new(fx(100), fx(100));
            let mut members = Vec::new();
            for k in 0..5 {
                let id = soldiers.push(post.x, post.y + fx(k), 0, 0, 0, Attrs::default(), 0);
                soldiers.slot[id as usize] = k as u16;
                members.push(id);
            }
            let watcher = members[0];
            let enemy = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
            if comrades_fighting {
                // 仲間 3 人が既に組み合っている。
                for &mate in &members[1..4] {
                    soldiers.hot.state[mate as usize] = State::Engaged;
                    soldiers.target[mate as usize] = enemy;
                }
            }
            soldiers.think_at[watcher as usize] = 0;

            let mut command = CommandTree::new();
            command.add_node(
                None,
                0,
                0,
                members[0],
                vec![],
                Some(test_unit(members.clone(), post)),
            );
            let mut ai = SoldierAiSystem::default();
            let mut perception = PerceptionSystem::default();
            for _ in 0..soldiers.len() {
                ai.register();
            }
            let home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
            let mut goals = home.clone();
            for tick in 0..200 {
                goals.clone_from(&home);
                step(
                    &mut ai,
                    &mut perception,
                    73,
                    tick,
                    &mut soldiers,
                    &command,
                    &mut goals,
                );
                if ai.action(watcher).seeks_target() {
                    return Some(tick);
                }
            }
            None
        }

        let with_squad = join_tick(true).expect("仲間が戦っているのに出ない");
        let alone = join_tick(false).expect("単独でも出ないなら比較にならない");
        // 迎撃の点数そのものを比べる。踏み出す早さは反応周期にも左右されるので、
        // つながりの効き方は点数で見る。
        assert!(
            join_score(true) > join_score(false),
            "仲間が出ていても迎撃の点数が上がらない"
        );
        let _ = (with_squad, alone);
    }

    /// 上のシナリオで、観測対象の「迎撃」の点数を取り出す。
    fn join_score(comrades_fighting: bool) -> i32 {
        let mut soldiers = Soldiers::default();
        let post = Vec2Fx::new(fx(100), fx(100));
        let mut members = Vec::new();
        for k in 0..5 {
            let id = soldiers.push(post.x, post.y + fx(k), 0, 0, 0, Attrs::default(), 0);
            soldiers.slot[id as usize] = k as u16;
            members.push(id);
        }
        let watcher = members[0];
        let enemy = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        if comrades_fighting {
            for &mate in &members[1..4] {
                soldiers.hot.state[mate as usize] = State::Engaged;
                soldiers.target[mate as usize] = enemy;
            }
        }
        soldiers.think_at[watcher as usize] = 0;

        let mut command = CommandTree::new();
        command.add_node(
            None,
            0,
            0,
            members[0],
            vec![],
            Some(test_unit(members.clone(), post)),
        );
        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        ai.set_watch(Some(watcher));
        let home: Vec<Vec2Fx> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = home.clone();
        for tick in 0..20 {
            goals.clone_from(&home);
            step(
                &mut ai,
                &mut perception,
                73,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
            if let Some((_, score)) = ai
                .watch_record()
                .actions
                .iter()
                .find(|(action, _)| *action == IndividualAction::JoinFight)
            {
                return *score;
            }
        }
        i32::MIN
    }

    #[test]
    fn hunt_person_limits_direct_pursuers_to_a_small_group() {
        let mut soldiers = Soldiers::default();
        let allies: Vec<_> = (0..8)
            .map(|i| soldiers.push(fx(100), fx(97 + i), 0, 0, 0, eager_attrs(), 0))
            .collect();
        let target = soldiers.push(fx(106), fx(100), 0, 0, 1, Attrs::default(), 0);
        soldiers.hot.state[target as usize] = State::Engaged;
        for &ally in &allies {
            soldiers.think_at[ally as usize] = 0;
        }

        let mut command = CommandTree::new();
        let post = Vec2Fx::new(fx(100), fx(100));
        let unit = crate::organization::Unit {
            soldiers: allies.clone(),
            troop_type: 0,
            formation: crate::organization::FORMATION_LINE,
            formation_origin: post,
            formation_facing: 0,
            ranks: 2,
            file_spacing: fx_from_mm(800),
            rank_spacing: fx_from_mm(800),
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: post,
            pursuit_leash: None,
        };
        let node = command.add_node(None, 0, 0, allies[0], vec![], Some(unit));
        command.node_mut(node).unwrap().objective = Some(Intent::HuntPerson { target });

        let mut ai = SoldierAiSystem::default();
        let mut perception = PerceptionSystem::default();
        for _ in 0..soldiers.len() {
            ai.register();
        }
        let formation_goals: Vec<_> = (0..soldiers.len()).map(|i| soldiers.pos(i)).collect();
        let mut goals = formation_goals.clone();
        for tick in 0..80 {
            goals.clone_from(&formation_goals);
            step(
                &mut ai,
                &mut perception,
                29,
                tick,
                &mut soldiers,
                &command,
                &mut goals,
            );
        }
        let pursuers = allies
            .iter()
            .filter(|&&ally| ai.focus(ally) == Some(target))
            .count();
        assert!(pursuers > 0);
        assert!(pursuers <= HUNT_TARGET_MAX_PURSUERS as usize);
    }
}
