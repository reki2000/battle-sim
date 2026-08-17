//! 史実の会戦プリセット。
//!
//! 地形（生成パラメータ + 整形）・両軍の陣容・指揮官の性格をひとまとめに宣言し、
//! [`build_world`] が決定論的にワールドを組み立てる。UI はこのプリセットの一覧を
//! 見せ、選ばれた 1 つを起動時に適用する。
//!
//! ## 仕様との関係
//!
//! 仕様 10 章 10 節はシナリオを TOML で書く形式を定めているが、TOML の実行時
//! ローダー（`sim-data`）はまだ無い。そこで `data/formations.toml` と
//! [`crate::organization::formation_def`] の関係と同じく、`data/scenarios/*.toml`
//! を人間が読む正本、ここをその写しとして持つ。ローダーが入ったら、この定数を
//! 読み込み結果に差し替えればよい。
//!
//! 仕様のシナリオ形式のうち、現時点のシミュレータが解釈できるのは次の範囲。
//!
//! - `[terrain]`: 生成パラメータ + シナリオ固有の地形整形（`sim_terrain::shaping`）
//! - `[[army]]`: 軍 → 部隊の 2 階層、部隊ごとの兵科・練度・装備・陣形・配置
//! - `commander`: アーキタイプ（性格）と、軍単位の会戦プラン
//! - 会戦開始前に完成している築城（アジンクールの杭列）
//!
//! 天候・準備時間・勝利条件・3 階層以上の編成はまだ扱わない。天候は「雨が
//! 降ったあと」の結果（泥濘）を地形整形として直接焼き込むことで代用している。

use crate::combat::{Armor, Weapon};
use crate::commander_ai::archetype_index;
use crate::organization::{
    formation_def, BattlePlan, FactionId, FormationId, Unit, FORMATION_LINE, FORMATION_PAVISE_LINE,
    FORMATION_SCHILTRON, FORMATION_SHIELDWALL, FORMATION_WEDGE,
};
use crate::soldiers::{flags, Attrs, SoldierId};
use crate::structures::StructureKind;
use crate::World;
use sim_math::{fx, fx_mul, Brad, Fx, Purpose, Rng, Vec2Fx, BRAD_HALF};
use sim_terrain::shaping::{RampAxis, Stamp};
use sim_terrain::{SeaEdge, Surface, Terrain, TerrainParams};

/// 軍（ルート）の階梯。
const ECHELON_ARMY: u8 = 0;
/// 軍直下の部隊（仕様 04 章の Battle 相当）の階梯。
const ECHELON_BATTLE: u8 = 1;
/// 副官として登録する人数の上限。
const DEPUTY_COUNT: usize = 4;

/// 兵の練度。仕様 04 章 5 節の表に対応する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 農民徴募兵。
    Levy,
    /// 都市民兵。
    Militia,
    /// 傭兵・熟練弓兵。
    Professional,
    /// 歴戦の従士。
    Veteran,
    /// 王の親衛隊・騎士。
    Elite,
}

/// 練度から引く能力値の平均。12 個の能力値それぞれに平均を書くと表が読めなく
/// なるので、4 つの系統に束ねて平均を与える。
struct QualityProfile {
    /// 運動能力（speed / accel / endurance / strength）。
    athletic: u8,
    /// 技量（reflex / skill）。
    skill: u8,
    /// 勇敢さ（bravery / aggression）。
    bravery: u8,
    /// 規律（discipline / loyalty / composure）。
    discipline: u8,
    /// 全能力値に共通の標準偏差。練度が高いほど個体差が小さい。
    stddev: u8,
}

impl Quality {
    const fn profile(self) -> QualityProfile {
        match self {
            Quality::Levy => QualityProfile {
                athletic: 120,
                skill: 60,
                bravery: 70,
                discipline: 55,
                stddev: 35,
            },
            Quality::Militia => QualityProfile {
                athletic: 128,
                skill: 90,
                bravery: 95,
                discipline: 85,
                stddev: 32,
            },
            Quality::Professional => QualityProfile {
                athletic: 140,
                skill: 140,
                bravery: 130,
                discipline: 145,
                stddev: 25,
            },
            Quality::Veteran => QualityProfile {
                athletic: 148,
                skill: 175,
                bravery: 165,
                discipline: 170,
                stddev: 20,
            },
            Quality::Elite => QualityProfile {
                athletic: 155,
                skill: 205,
                bravery: 195,
                discipline: 190,
                stddev: 16,
            },
        }
    }
}

/// 装備一式。武器・防具・矢数・騎乗の有無をまとめて指定する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loadout {
    /// 徒歩の重装兵。剣とプレートアーマー。
    FootMenAtArms,
    /// ポールアックス・戦鎚を持つ徒歩の重装兵。プレートアーマー。
    ///
    /// 板金相手に刃は通らない（`type_factor` の Cut × Plate は 10%）。
    /// 徒歩で armoured man を倒すために打撃武器が使われたのは、まさに
    /// この理由による。
    PollaxeMenAtArms,
    /// 下馬した騎士。詰めた騎槍とプレートアーマー。
    DismountedLancers,
    /// 槍と鎖帷子。
    Spearmen,
    /// 長槍（パイク）と鎖帷子。スコットランドのシルトロンのような、
    /// 後列も穂先を出す密集槍陣に使う。
    ///
    /// `Weapon::pike` は `requires_front_row = false` なので、後列の兵も
    /// 攻撃に参加できる——それが槍衾の本質であり、騎兵の忌避判定
    /// （`cavalry` の `spear_wall`）にも効く。
    Pikemen,
    /// 長弓と胴衣。矢は 2 束（72 本）。
    Longbowmen,
    /// 弩と鎖帷子。
    Crossbowmen,
    /// 騎乗の騎士。ランスと鎖帷子。
    ///
    /// 乗り手は板金も着けていたが、馬はほぼ無防備だった。この区別を
    /// シミュレータはまだ持たないので、騎兵単位の防御を鎖帷子相当に
    /// 落とすことで「矢は馬に当たる」ぶんを表現する。
    MountedKnights,
}

impl Loadout {
    fn weapon(self) -> Weapon {
        match self {
            Loadout::FootMenAtArms => Weapon::sword(),
            Loadout::PollaxeMenAtArms => Weapon::mace(),
            Loadout::DismountedLancers => Weapon::spear(),
            Loadout::Spearmen => Weapon::spear(),
            Loadout::Pikemen => Weapon::pike(),
            Loadout::Longbowmen => Weapon::longbow(),
            Loadout::Crossbowmen => Weapon::crossbow(),
            Loadout::MountedKnights => Weapon::lance(),
        }
    }

    fn armor(self) -> Armor {
        match self {
            Loadout::FootMenAtArms | Loadout::PollaxeMenAtArms | Loadout::DismountedLancers => {
                Armor::plate()
            }
            Loadout::Spearmen
            | Loadout::Pikemen
            | Loadout::Crossbowmen
            | Loadout::MountedKnights => Armor::mail(),
            Loadout::Longbowmen => Armor::cloth(),
        }
    }

    /// 携行する矢・矢弾の数。射撃武器でなければ 0。
    fn ammo(self) -> u16 {
        match self {
            Loadout::Longbowmen => 72,
            Loadout::Crossbowmen => 40,
            _ => 0,
        }
    }

    fn soldier_flags(self) -> u8 {
        match self {
            Loadout::MountedKnights => flags::MOUNTED,
            Loadout::Longbowmen | Loadout::Crossbowmen => flags::MISSILE_TROOP,
            _ => 0,
        }
    }
}

/// 会戦が始まる前に完成している障害物。
///
/// 仕様 07 章 7 節の「会戦前フェーズ」で工兵が作り終えた状態を、工事を待たずに
/// 再現するためのもの。部隊の正面の幅いっぱいに、`ahead_m` だけ前方へ置く。
///
/// 1 本の長い線分ではなく短い区画に分けて建てる。構造物の耐久は 1 本ごとに
/// 独立していて（`StructureKind::hp_max`）、突っ込んだ馬に削られるため、
/// 全長を 1 本にすると数騎の突破で防御線が丸ごと消えてしまう。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObstacleDef {
    pub kind: StructureKind,
    /// 最前列の何 m 前方に置くか。
    pub ahead_m: u16,
}

/// 障害物を区切る 1 区画の長さ（m）。
const OBSTACLE_SEGMENT_M: i32 = 5;

/// 指揮官 1 人。
#[derive(Clone, Copy, Debug)]
pub struct CommanderDef {
    pub name_ja: &'static str,
    /// [`crate::commander_ai::ARCHETYPES`] の名前。未知の名前なら既定の性格になる。
    pub archetype: &'static str,
}

/// 1 部隊（葉ノード = 実体を持つ `Unit`）。
#[derive(Clone, Copy, Debug)]
pub struct ContingentDef {
    pub name_ja: &'static str,
    pub commander: CommanderDef,
    /// 視覚プロファイル（`data/visual-profiles/*.toml`）の兵科 index。
    pub troop_type: u16,
    pub quality: Quality,
    pub loadout: Loadout,
    pub count: u32,
    /// 横に並ぶ人数。奥行き（ranks）は `count` から決まる。
    pub files: u32,
    pub formation: FormationId,
    /// 隊列の間隔（mm）。0 なら陣形プリセットの値を使う。
    ///
    /// 陣形プリセットの間隔は徒歩兵を前提にしているので、騎兵の部隊は
    /// 馬の当たり判定（半径 90 cm = 直径 1.8 m）に合う広さをここで指定する。
    /// 2.0 m だと隣同士が触れ合ったままになり、押し合いが止まらずに
    /// 隊列が自壊する。2.5 m 取れば余裕ができる。
    pub file_spacing_mm: u16,
    pub rank_spacing_mm: u16,
    /// 最前列の中央（m）。隊列は `facing` の逆向きに奥行きぶん伸びる。
    pub front_x_m: i32,
    pub front_y_m: i32,
    /// 部隊の正面。`0` でランクが +Y 方向へ伸びる（`formation_goals` の規約）。
    pub facing: Brad,
    /// この部隊に軍司令官が随伴するか。どの部隊にも無ければ先頭の部隊。
    pub hosts_army_commander: bool,
    /// 会戦前に完成している障害物（杭列・堀など）。無ければ `None`。
    pub obstacle: Option<ObstacleDef>,
}

/// 1 つの軍（ルートノード）。
#[derive(Clone, Copy, Debug)]
pub struct ArmyDef {
    pub faction: FactionId,
    pub name_ja: &'static str,
    pub commander: CommanderDef,
    /// 会戦開始時の方針。`None` なら指揮官 AI が最初の評価で自分で選ぶ。
    pub battle_plan: Option<BattlePlan>,
    pub contingents: &'static [ContingentDef],
}

/// 会戦プリセット 1 つ。
#[derive(Clone, Copy, Debug)]
pub struct ScenarioDef {
    pub id: &'static str,
    pub name_ja: &'static str,
    pub name_en: &'static str,
    /// 西暦。
    pub year: u16,
    pub place_ja: &'static str,
    pub summary_ja: &'static str,
    /// 史実の推定兵力（表示用の文章）。実際に配置する兵数は各部隊の `count` で、
    /// ブラウザで回せる規模に縮尺してある。
    pub historical_strength_ja: &'static str,
    /// 縮尺の説明（表示用）。
    pub scale_note_ja: &'static str,
    pub terrain: TerrainParams,
    /// 生成後に焼き込む地勢。
    pub shaping: &'static [Stamp],
    pub armies: &'static [ArmyDef],
}

impl ScenarioDef {
    /// 実際に配置される総兵数。
    pub fn soldier_count(&self) -> u32 {
        self.armies
            .iter()
            .flat_map(|a| a.contingents.iter())
            .map(|c| c.count)
            .sum()
    }

    /// 陣営ごとの配置兵数。
    pub fn army_soldier_count(&self, army: &ArmyDef) -> u32 {
        army.contingents.iter().map(|c| c.count).sum()
    }
}

// ----------------------------------------------------------------------
// レジストリ
// ----------------------------------------------------------------------

/// 選択できる会戦プリセットの一覧。index が UI・wasm 境界での ID になる。
pub static SCENARIOS: &[ScenarioDef] = &[AGINCOURT_1415, CRECY_1346, BANNOCKBURN_1314];

/// index からプリセットを引く。
pub fn get(index: usize) -> Option<&'static ScenarioDef> {
    SCENARIOS.get(index)
}

/// id からプリセットの index を引く。
pub fn index_of(id: &str) -> Option<usize> {
    SCENARIOS.iter().position(|s| s.id == id)
}

// ----------------------------------------------------------------------
// 組み立て
// ----------------------------------------------------------------------

/// 地形を生成・整形し、両軍を配置したワールドを作る。
pub fn build_world(def: &ScenarioDef) -> World {
    let mut world = World::with_terrain(def.terrain.seed, generate_terrain(def));
    deploy(&mut world, def);
    world
}

/// シナリオの地形を生成し、シナリオ固有の地勢を焼き込む。
///
/// 地形キャッシュ（M9）はこの結果のグリッドを保存する。復元側は
/// [`World::with_terrain`] + [`deploy`] を呼べば同じワールドになる。
pub fn generate_terrain(def: &ScenarioDef) -> Terrain {
    let mut terrain = sim_terrain::generate(&def.terrain);
    sim_terrain::shaping::apply(&mut terrain, def.shaping);
    terrain
}

/// 既に地形を持つワールドへ、両軍・指揮系統・築城を配置する。
///
/// 兵士は最初から陣形スロットの上に立つので、開始直後に隊列を組み直す動きは
/// 起きない。命令は与えない——各軍のルートノードに宿る指揮官 AI（M7）が、
/// 与えられた性格と会戦プランから自分で最初の判断を下す。
pub fn deploy(world: &mut World, def: &ScenarioDef) {
    for (army_idx, army) in def.armies.iter().enumerate() {
        let mut rosters: Vec<Vec<SoldierId>> = Vec::with_capacity(army.contingents.len());
        for (c_idx, cont) in army.contingents.iter().enumerate() {
            let salt = deploy_salt(army_idx, c_idx);
            rosters.push(spawn_contingent(world, cont, army.faction, salt));
        }

        // 軍司令官はいずれかの部隊に随伴する（伝令の距離も彼の位置から測られる）。
        let host = army
            .contingents
            .iter()
            .position(|c| c.hosts_army_commander)
            .unwrap_or(0);
        let Some(host_roster) = rosters.get(host).filter(|r| !r.is_empty()) else {
            continue;
        };
        let root = world.add_command_node(
            None,
            ECHELON_ARMY,
            army.faction,
            host_roster[0],
            host_roster
                .iter()
                .skip(1)
                .take(DEPUTY_COUNT)
                .copied()
                .collect(),
            None,
        );
        world.set_commander_archetype(
            root,
            archetype_index(army.commander.archetype).unwrap_or(0),
            deploy_salt(army_idx, army.contingents.len()),
        );
        if let (Some(plan), Some(node)) = (army.battle_plan, world.command.node_mut(root)) {
            node.battle_plan = Some(plan);
        }

        for (c_idx, cont) in army.contingents.iter().enumerate() {
            let roster = rosters[c_idx].clone();
            if roster.is_empty() {
                continue;
            }
            // 部隊長は軍司令官と別人にする。同一人物だと 1 人の戦死で軍と部隊の
            // 指揮が同時に空白になり、継承（M3）の挙動が実態とずれる。
            let leader_slot = usize::from(c_idx == host && roster.len() > 1);
            let commander = roster[leader_slot];
            let deputies: Vec<SoldierId> = roster
                .iter()
                .filter(|&&id| id != commander)
                .take(DEPUTY_COUNT)
                .copied()
                .collect();
            let unit = build_unit(cont, roster);
            let node = world.add_command_node(
                Some(root),
                ECHELON_BATTLE,
                army.faction,
                commander,
                deputies,
                Some(unit),
            );
            world.set_commander_archetype(
                node,
                archetype_index(cont.commander.archetype).unwrap_or(0),
                deploy_salt(army_idx, c_idx),
            );
            if let Some(obstacle) = cont.obstacle {
                place_obstacle(world, cont, obstacle, army.faction);
            }
        }
    }
}

/// 配置に使う乱数ストリームの salt。軍・部隊ごとに別のストリームにして、
/// 部隊を 1 つ足しても他の部隊の能力値が変わらないようにする。
fn deploy_salt(army_idx: usize, contingent_idx: usize) -> u32 {
    (army_idx as u32 + 1) * 1_000 + contingent_idx as u32
}

/// 部隊の奥行き方向（正面）と横方向の単位ベクトル。
///
/// [`crate::organization::CommandTree::formation_goals`] の回転式と同じ規約：
/// ランクは `(-sin, cos)` へ、ファイルは `(cos, sin)` へ伸びる。
fn axes(facing: Brad) -> (Vec2Fx, Vec2Fx) {
    let sin = sim_math::sin_fx(facing);
    let cos = sim_math::cos_fx(facing);
    (Vec2Fx::new(-sin, cos), Vec2Fx::new(cos, sin))
}

struct Layout {
    ranks: u32,
    files: u32,
    file_spacing: Fx,
    rank_spacing: Fx,
    forward: Vec2Fx,
    right: Vec2Fx,
    /// ランク 0（最後尾）の中央。`Unit::formation_origin` と同じ点。
    origin: Vec2Fx,
}

fn layout(cont: &ContingentDef) -> Layout {
    let def = formation_def(cont.formation);
    let file_spacing = spacing_or(cont.file_spacing_mm, def.file_spacing);
    let rank_spacing = spacing_or(cont.rank_spacing_mm, def.rank_spacing);
    let files = cont.files.max(1);
    let ranks = cont.count.div_ceil(files).max(1);
    let (forward, right) = axes(cont.facing);
    let depth = fx(ranks.saturating_sub(1) as i32);
    let front = Vec2Fx::new(fx(cont.front_x_m), fx(cont.front_y_m));
    Layout {
        ranks,
        files,
        file_spacing,
        rank_spacing,
        forward,
        right,
        origin: front.sub(forward.scale(fx_mul(depth, rank_spacing))),
    }
}

/// 0 なら陣形プリセットの間隔を使う。
fn spacing_or(override_mm: u16, default: Fx) -> Fx {
    if override_mm == 0 {
        default
    } else {
        sim_math::fx_from_mm(override_mm as i32)
    }
}

/// 陣形スロット `slot` の座標。`formation_goals` のスロット計算と一致させる。
fn slot_pos(l: &Layout, slot: u32) -> Vec2Fx {
    let file = (slot % l.files) as i32;
    let rank = (slot / l.files) as i32;
    let local_x = fx_mul(fx(file), l.file_spacing)
        - fx_mul(fx(l.files.saturating_sub(1) as i32), l.file_spacing) / 2;
    let local_y = fx_mul(fx(rank), l.rank_spacing);
    l.origin
        .add(l.right.scale(local_x))
        .add(l.forward.scale(local_y))
}

/// 1 部隊ぶんの兵士を陣形スロットの上に生成する。
fn spawn_contingent(
    world: &mut World,
    cont: &ContingentDef,
    faction: FactionId,
    salt: u32,
) -> Vec<SoldierId> {
    let l = layout(cont);
    let mut rng = Rng::stream(world.seed, salt, Purpose::Spawn, 0);
    let soldier_flags = cont.loadout.soldier_flags();
    let facing = l.forward.angle();
    let mut ids = Vec::with_capacity(cont.count as usize);
    for slot in 0..cont.count {
        let attrs = roll_attrs(&mut rng, cont.quality);
        let id = world.spawn_typed(
            slot_pos(&l, slot),
            facing,
            faction as u16,
            faction,
            cont.troop_type,
            attrs,
            soldier_flags,
        );
        world
            .combat
            .set_loadout(id, cont.loadout.weapon(), cont.loadout.armor());
        world.combat.set_ammo(id, cont.loadout.ammo());
        ids.push(id);
    }
    ids
}

fn build_unit(cont: &ContingentDef, soldiers: Vec<SoldierId>) -> Unit {
    let l = layout(cont);
    Unit {
        soldiers,
        troop_type: cont.troop_type,
        formation: cont.formation,
        formation_origin: l.origin,
        formation_facing: cont.facing,
        ranks: l.ranks.min(u16::MAX as u32) as u16,
        file_spacing: l.file_spacing,
        rank_spacing: l.rank_spacing,
        banner: None,
        formation_change: None,
        path: Vec::new(),
        path_final: l.origin,
        pursuit_leash: None,
    }
}

/// 最前列の前方へ、完成済みの障害物を並べる（アジンクールの杭列、
/// バノックバーンの落とし穴など）。
///
/// 部隊の正面幅を [`OBSTACLE_SEGMENT_M`] ごとに区切り、隙間なく並べた
/// 短い区画として建てる（[`ObstacleDef`] の注記を参照）。
fn place_obstacle(
    world: &mut World,
    cont: &ContingentDef,
    obstacle: ObstacleDef,
    faction: FactionId,
) {
    let l = layout(cont);
    let front_center = Vec2Fx::new(fx(cont.front_x_m), fx(cont.front_y_m));
    let center = front_center.add(l.forward.scale(fx(obstacle.ahead_m as i32)));
    let half_width = fx_mul(fx(l.files.saturating_sub(1) as i32), l.file_spacing) / 2;
    let width_m = (sim_math::fx_to_mm(half_width) * 2 / 1000).max(OBSTACLE_SEGMENT_M);
    let segments = width_m.div_euclid(OBSTACLE_SEGMENT_M).max(1);
    let left = center.sub(l.right.scale(half_width));
    let step = l.right.scale(fx(width_m) / segments);
    for i in 0..segments {
        let a = left.add(step.scale(fx(i)));
        let b = left.add(step.scale(fx(i + 1)));
        let id = world.structures.build(obstacle.kind, a, b, faction);
        world.structures.set_completion(id, 1000);
    }
}

/// 練度から 12 個の能力値を引く。
fn roll_attrs(rng: &mut Rng, quality: Quality) -> Attrs {
    let p = quality.profile();
    // 勇敢な兵ほど自己保存が弱い。半分だけ引いて、極端な値にならないようにする。
    let self_preservation = 200u8.saturating_sub(p.bravery / 2);
    Attrs::new(
        rng.attr(p.athletic, p.stddev),
        rng.attr(p.athletic, p.stddev),
        rng.attr(p.athletic, p.stddev),
        rng.attr(p.athletic, p.stddev),
        rng.attr(p.skill, p.stddev),
        rng.attr(p.skill, p.stddev),
        rng.attr(p.bravery, p.stddev),
        rng.attr(p.discipline, p.stddev),
        rng.attr(p.bravery, p.stddev),
        rng.attr(self_preservation, p.stddev),
        rng.attr(p.discipline, p.stddev),
        rng.attr(p.discipline, p.stddev),
    )
}

// ----------------------------------------------------------------------
// アジンクール 1415
// ----------------------------------------------------------------------
//
// 地勢・配置の数値は `data/scenarios/agincourt_1415.toml` に同じものを
// 人間が読める形で置いてある。数値を変えるときは両方を直すこと。

/// 地図の一辺（m）。会戦場の南北に、追撃と敗走のための余白を取ってある。
const AG_SIZE_M: i32 = 1200;
/// 英軍の最前列（南）。
const AG_ENGLISH_FRONT_Y: i32 = 430;
/// 仏軍前衛の最前列（北）。両軍の間合いは 225 m——長弓の射程 120 m の外側で、
/// かつ指揮官が敵を「目の前の好機」と見なす距離（250 m）の内側。
/// ここから仏軍の突出が始まる。
const AG_FRENCH_FRONT_Y: i32 = 655;
/// 会戦場の中心線（東西）。
const AG_CENTER_X: i32 = 600;

/// アジンクールの会戦（1415 年 10 月 25 日）。
///
/// 森に挟まれて狭まっていく耕地。前夜からの雨で泥濘化しており、重装の徒歩兵に
/// とっては 200 m の前進そのものが消耗になる。英軍は数で劣るが、両翼の長弓兵が
/// 杭列の後ろに立つ。仏軍は指揮系統が分裂しており、名誉に飢えた前衛が命令を
/// 待たずに動きやすい。
///
/// このシミュレータで再現できるのは「泥・狭隘・射撃・杭・指揮の分裂」までで、
/// 装備重量が泥での消耗に効く点（仕様 03 章 3.2）と、矢が尽きた長弓兵が
/// 槌を持って白兵戦に加わる点はまだ実装されていない。
pub const AGINCOURT_1415: ScenarioDef = ScenarioDef {
    id: "agincourt_1415",
    name_ja: "アジンクールの会戦",
    name_en: "Battle of Agincourt",
    year: 1415,
    place_ja: "北フランス・アルトワ",
    summary_ja: "森に挟まれた泥濘の耕地。数で劣る英軍が杭列と長弓で待ち構える。",
    historical_strength_ja: "史実の推定兵力: 英 約6,000〜9,000 / 仏 約12,000〜15,000",
    scale_note_ja: "ブラウザで回せるよう、兵数と会戦場の幅をおよそ 1/4 に縮尺してある",
    terrain: TerrainParams {
        seed: 0x4A17_C0FF_1415_0001,
        size_m: AG_SIZE_M as u32,
        cell_m: 2,
        // 起伏はほぼ無い平野。高低差は下の HeightRamp で与える。
        relief: 0,
        // 会戦場そのものは下の整形で塗り替わる。ここは周囲の林の量。
        forest_cover: 300,
        // 秋の長雨。会戦場の外も水を含んでいる。
        marsh_bias: 250,
        thermal_iterations: 6,
        // 会戦場を横切る川は無い（テルヌワーズ川は地図の外）。
        river_density: 0,
        road_count: 1,
        sea_edge: SeaEdge::None,
        sea_level_cm: 0,
    },
    shaping: AGINCOURT_SHAPING,
    armies: AGINCOURT_ARMIES,
};

/// アジンクールの地勢。
///
/// 適用順に意味がある。耕地を敷いてから泥濘の帯を重ね、最後に森を置くことで、
/// 森の縁が耕地に侵食されない。
const AGINCOURT_SHAPING: &[Stamp] = &[
    // まず会戦場の水を抜く。手続き生成が窪地に作った湖が両軍の展開地に
    // かかっていると、そこだけ騎兵が動けず、隊列が押し合いで自壊する。
    Stamp::Drain {
        surface: Surface::Farmland,
        x_m: 380,
        y_m: 150,
        w_m: 440,
        h_m: 950,
    },
    // 会戦場は刈り取りの済んだ耕地。
    Stamp::SurfaceRect {
        surface: Surface::Farmland,
        x_m: 380,
        y_m: 150,
        w_m: 440,
        h_m: 950,
    },
    // 両軍の間と仏軍の展開地は、前夜からの雨と数千の足で捏ねられた泥。
    // `Surface::Mud` は移動を鈍らせ、疲労を大きく増やす（仕様 06 章 2 節）。
    Stamp::SurfaceRect {
        surface: Surface::Mud,
        x_m: 430,
        y_m: 400,
        w_m: 340,
        h_m: 360,
    },
    // 西のアジャンクール村の森。南（英軍側）へ行くほど東へ張り出す。
    Stamp::SurfaceBelt {
        surface: Surface::DenseForest,
        ax_m: 300,
        ay_m: 1200,
        bx_m: 400,
        by_m: 0,
        half_width_m: 130,
    },
    // 東のトラムクール村の森。こちらは南へ行くほど西へ張り出す。
    // 2 つ合わせて、会戦場は北の 340 m から英軍正面の 210 m まで狭まる。
    Stamp::SurfaceBelt {
        surface: Surface::DenseForest,
        ax_m: 900,
        ay_m: 1200,
        bx_m: 800,
        by_m: 0,
        half_width_m: 130,
    },
    // 英軍の立つ南側がわずかに高い。仏軍は緩い登りを泥の中で進むことになる。
    Stamp::HeightRamp {
        axis: RampAxis::Y,
        from_m: 350,
        to_m: 750,
        from_cm: 500,
        to_cm: 0,
    },
];

const AGINCOURT_ARMIES: &[ArmyDef] = &[
    ArmyDef {
        faction: 0,
        name_ja: "イングランド軍",
        commander: CommanderDef {
            name_ja: "ヘンリー5世",
            // 規律と技量で押さえ込み、決めた守勢を崩さない。
            archetype: "professional_marshal",
        },
        // 数で劣り、射撃兵が多く、わずかに高い地面に立つ——守勢が理に適う。
        battle_plan: Some(BattlePlan::DefendHighGround),
        contingents: ENGLISH_CONTINGENTS,
    },
    ArmyDef {
        faction: 1,
        name_ja: "フランス軍",
        commander: CommanderDef {
            name_ja: "シャルル・ダルブレ（大元帥）",
            // 名目上の総司令官。頑固で、諸侯を統制しきれない。
            archetype: "stubborn_baron",
        },
        battle_plan: Some(BattlePlan::CenterPush),
        contingents: FRENCH_CONTINGENTS,
    },
];

/// 英軍。中央に徒歩の重装兵 3 隊、その両翼に長弓兵。長弓兵の前には杭列。
/// 正面 `facing = 0` はランクが +Y（北＝仏軍側）へ伸びる向き。
const ENGLISH_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "左翼 長弓隊",
        commander: CommanderDef {
            name_ja: "トマス・アーピンガム卿",
            archetype: "veteran_mercenary",
        },
        troop_type: 2,
        quality: Quality::Professional,
        loadout: Loadout::Longbowmen,
        count: 700,
        files: 56,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 530,
        front_y_m: AG_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: Some(ObstacleDef {
            kind: StructureKind::Stakes,
            ahead_m: 8,
        }),
    },
    ContingentDef {
        name_ja: "前衛（ヨーク公）",
        commander: CommanderDef {
            name_ja: "ヨーク公エドワード",
            archetype: "disciplinarian",
        },
        troop_type: 0,
        quality: Quality::Veteran,
        loadout: Loadout::PollaxeMenAtArms,
        count: 125,
        files: 32,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 578,
        front_y_m: AG_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "本隊（国王）",
        commander: CommanderDef {
            name_ja: "ヘンリー5世 直率",
            archetype: "disciplinarian",
        },
        troop_type: 0,
        quality: Quality::Elite,
        loadout: Loadout::PollaxeMenAtArms,
        count: 125,
        files: 32,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: AG_CENTER_X + 3,
        front_y_m: AG_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: true,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "後衛（カモイス卿）",
        commander: CommanderDef {
            name_ja: "カモイス卿",
            archetype: "cautious_commander",
        },
        troop_type: 0,
        quality: Quality::Veteran,
        loadout: Loadout::PollaxeMenAtArms,
        count: 125,
        files: 32,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 628,
        front_y_m: AG_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "右翼 長弓隊",
        commander: CommanderDef {
            name_ja: "ジョン・コーンウォール卿",
            archetype: "veteran_mercenary",
        },
        troop_type: 2,
        quality: Quality::Professional,
        loadout: Loadout::Longbowmen,
        count: 700,
        files: 56,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 675,
        front_y_m: AG_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: Some(ObstacleDef {
            kind: StructureKind::Stakes,
            ahead_m: 8,
        }),
    },
];

/// 仏軍。徒歩の重装兵を 2 線、その両翼に騎兵、後方に第 3 線。
/// 正面 `facing = BRAD_HALF` はランクが -Y（南＝英軍側）へ伸びる向き。
const FRENCH_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "前衛（徒歩重装兵）",
        commander: CommanderDef {
            name_ja: "ブシコー元帥",
            // 名誉に飢えた諸侯の集まり。命令を待たずに動く（独断専行、仕様 05 章）。
            archetype: "honor_hungry_knight",
        },
        troop_type: 0,
        quality: Quality::Veteran,
        loadout: Loadout::DismountedLancers,
        count: 1200,
        files: 120,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: AG_CENTER_X,
        front_y_m: AG_FRENCH_FRONT_Y,
        facing: BRAD_HALF as u16,
        hosts_army_commander: true,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "左翼騎兵",
        commander: CommanderDef {
            name_ja: "クリニェ・ド・ブラバン",
            archetype: "reckless_youth",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 200,
        files: 30,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: 520,
        front_y_m: 628,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "右翼騎兵",
        commander: CommanderDef {
            name_ja: "ギヨーム・ド・サヴーズ",
            archetype: "honor_hungry_knight",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 150,
        files: 30,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: 678,
        front_y_m: 628,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "本隊（アランソン公）",
        commander: CommanderDef {
            name_ja: "アランソン公",
            archetype: "stubborn_baron",
        },
        troop_type: 0,
        quality: Quality::Veteran,
        loadout: Loadout::DismountedLancers,
        count: 900,
        files: 90,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: AG_CENTER_X,
        front_y_m: 700,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "第三線（騎乗）",
        commander: CommanderDef {
            name_ja: "ブラバン公",
            // 前 2 線の崩壊を見て動かなかった第 3 線。慎重さが極端に出る。
            archetype: "cautious_commander",
        },
        troop_type: 3,
        quality: Quality::Professional,
        loadout: Loadout::MountedKnights,
        count: 300,
        files: 50,
        formation: FORMATION_LINE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: AG_CENTER_X,
        front_y_m: 760,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
];

// ----------------------------------------------------------------------
// バノックバーン 1314
// ----------------------------------------------------------------------
//
// 数値は `data/scenarios/bannockburn_1314.toml` と同じものを持つ。

/// 地図の一辺（m）。
const BB_SIZE_M: i32 = 1200;
/// スコットランド軍の最前列（南）。森（ニューパーク）を出たところ。
const BB_SCOTS_FRONT_Y: i32 = 560;
/// イングランド軍の最前列（北）。両軍の間合いは 140 m——シルトロンは
/// `move_mult` 300 と極端に遅いので、近づけないと会戦にならない。
const BB_ENGLISH_FRONT_Y: i32 = 700;
/// 会戦場の中心線（東西）。
const BB_CENTER_X: i32 = 600;

/// バノックバーンの会戦（1314 年 6 月 24 日、2 日目）。
///
/// イングランド軍は 2 つの小川に挟まれた低湿地（カース）で夜を明かし、
/// そこへスコットランド軍が森を出て降りてきた。数で 2 倍以上まさりながら、
/// 湿地では騎兵が働かず、後ろは水で退けず、正面の幅も足りない——**数を
/// 使えないまま押し込まれる**のがこの会戦の骨格になる。
///
/// アジンクールが「泥と狭隘と射撃」なら、こちらは「湿地と長槍と密集」。
/// スコットランド軍の主力はパイクのシルトロンで、`Weapon::pike` は
/// `requires_front_row = false`（後列も突ける）かつ間合い 5 m なので、
/// 騎兵の忌避判定（`cavalry` の `spear_wall`）を強く働かせる。
///
/// 小川そのものは `Surface::Marsh` で近似している。地形整形は水深グリッドを
/// 書かないため（`sim_terrain::shaping` 参照）、開けた水面ではなく
/// 「騎兵が入れない（`cavalry_mult` 0）・移動 35%・疲労 2.2 倍」の湿地として
/// 表現する。
pub const BANNOCKBURN_1314: ScenarioDef = ScenarioDef {
    id: "bannockburn_1314",
    name_ja: "バノックバーンの会戦",
    name_en: "Battle of Bannockburn",
    year: 1314,
    place_ja: "スコットランド・スターリング近郊",
    summary_ja: "低湿地に押し込まれたイングランド軍へ、パイクのシルトロンが降りてくる。",
    historical_strength_ja:
        "史実の推定兵力: スコットランド 約6,000〜7,000 / イングランド 約15,000〜20,000",
    scale_note_ja: "ブラウザで回せるよう、兵数と会戦場の幅をおよそ 1/6 に縮尺してある",
    terrain: TerrainParams {
        seed: 0x8A11_0CB0_1314_0001,
        size_m: BB_SIZE_M as u32,
        cell_m: 2,
        relief: 0,
        // 周囲はニューパークの森。
        forest_cover: 350,
        // 6 月とはいえカースは水を含んでいる。
        marsh_bias: 400,
        thermal_iterations: 6,
        river_density: 0,
        road_count: 1,
        sea_edge: SeaEdge::None,
        sea_level_cm: 0,
    },
    shaping: BANNOCKBURN_SHAPING,
    armies: BANNOCKBURN_ARMIES,
};

/// バノックバーンの地勢。カース（低湿地）を敷き、その左右から小川の湿地で
/// 挟み込み、南のスコットランド側を高くする。
const BANNOCKBURN_SHAPING: &[Stamp] = &[
    // 会戦場の水を抜く。低湿地そのものは下の `Marsh` で表現するので、
    // ここで消すのは手続き生成が作った湖（騎兵が完全に止まる）。
    Stamp::Drain {
        surface: Surface::Meadow,
        x_m: 330,
        y_m: 280,
        w_m: 540,
        h_m: 800,
    },
    // スコットランド側は森を出た先の牧草地。
    Stamp::SurfaceRect {
        surface: Surface::Meadow,
        x_m: 330,
        y_m: 300,
        w_m: 540,
        h_m: 360,
    },
    // イングランド軍が夜を明かしたカース。踏めば沈む軟らかい地面で、
    // 騎兵は速度 4 割・歩兵は疲労 1.9 倍になる。
    //
    // ここを `Marsh` にはしない。湿地は騎兵の速度倍率が 0——つまり馬が
    // 1 mm も動けない——ので、部隊が固まったまま押し合って圧死する。
    // 「動けるが最悪の足場」は `Mud` が担う。湿地は下の 2 本の小川、
    // すなわち**入ったら終わりの縁**にだけ使う。
    Stamp::SurfaceRect {
        surface: Surface::Mud,
        x_m: 380,
        y_m: 660,
        w_m: 440,
        h_m: 400,
    },
    // 西のバノック川。北へ行くほど内側へ食い込み、退路を狭める。
    Stamp::SurfaceBelt {
        surface: Surface::Marsh,
        ax_m: 330,
        ay_m: 620,
        bx_m: 450,
        by_m: 1100,
        half_width_m: 110,
    },
    // 東のペルストリーム川。
    Stamp::SurfaceBelt {
        surface: Surface::Marsh,
        ax_m: 870,
        ay_m: 620,
        bx_m: 750,
        by_m: 1100,
        half_width_m: 110,
    },
    // 南（スコットランド側）が高い。降りてくる側にモメンタムが乗る。
    Stamp::HeightRamp {
        axis: RampAxis::Y,
        from_m: 400,
        to_m: 800,
        from_cm: 700,
        to_cm: 0,
    },
];

const BANNOCKBURN_ARMIES: &[ArmyDef] = &[
    ArmyDef {
        faction: 0,
        name_ja: "スコットランド軍",
        commander: CommanderDef {
            name_ja: "ロバート1世（ブルース）",
            // 数で劣る側が地形を選んで仕掛けた——狡猾さと柔軟さ。
            archetype: "cunning_captain",
        },
        // 4 個のシルトロンが順に前へ出て、湿地へ押し込む。
        battle_plan: Some(BattlePlan::EchelonAttack),
        contingents: SCOTS_CONTINGENTS,
    },
    ArmyDef {
        faction: 1,
        name_ja: "イングランド軍",
        commander: CommanderDef {
            name_ja: "エドワード2世",
            archetype: "stubborn_baron",
        },
        // 合意された会戦プランが無いまま夜明けを迎えた軍。ここだけ `None` に
        // して、指揮官 AI にその場で選ばせる。
        battle_plan: None,
        contingents: BB_ENGLISH_CONTINGENTS,
    },
];

/// スコットランド軍。パイクのシルトロン 4 個と、少数の軽騎兵。
/// 正面 `facing = 0` はランクが +Y（北＝イングランド軍側）へ伸びる向き。
const SCOTS_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "左翼シルトロン（ダグラス）",
        commander: CommanderDef {
            name_ja: "ジェームズ・ダグラス",
            archetype: "cunning_captain",
        },
        troop_type: 1,
        quality: Quality::Professional,
        loadout: Loadout::Pikemen,
        count: 260,
        files: 40,
        formation: FORMATION_SCHILTRON,
        // 陣形プリセットのファイル間隔（0.5 m）は兵士の当たり判定の直径
        // （0.7 m）より狭く、そのまま使うと隊列が常時重なって自分で圧死する
        // （`sim-headless` の `form_battle_units` も同じ理由で 0.8 m を明示
        // している）。
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: 470,
        front_y_m: BB_SCOTS_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "中央シルトロン（モレー伯）",
        commander: CommanderDef {
            name_ja: "モレー伯トマス・ランドルフ",
            archetype: "veteran_mercenary",
        },
        troop_type: 1,
        quality: Quality::Veteran,
        loadout: Loadout::Pikemen,
        count: 260,
        files: 40,
        formation: FORMATION_SCHILTRON,
        // 陣形プリセットのファイル間隔（0.5 m）は兵士の当たり判定の直径
        // （0.7 m）より狭く、そのまま使うと隊列が常時重なって自分で圧死する
        // （`sim-headless` の `form_battle_units` も同じ理由で 0.8 m を明示
        // している）。
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: 555,
        front_y_m: BB_SCOTS_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "右翼シルトロン（エドワード・ブルース）",
        commander: CommanderDef {
            name_ja: "エドワード・ブルース",
            archetype: "honor_hungry_knight",
        },
        troop_type: 1,
        quality: Quality::Professional,
        loadout: Loadout::Pikemen,
        count: 260,
        files: 40,
        formation: FORMATION_SCHILTRON,
        // 陣形プリセットのファイル間隔（0.5 m）は兵士の当たり判定の直径
        // （0.7 m）より狭く、そのまま使うと隊列が常時重なって自分で圧死する
        // （`sim-headless` の `form_battle_units` も同じ理由で 0.8 m を明示
        // している）。
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: 640,
        front_y_m: BB_SCOTS_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "国王シルトロン",
        commander: CommanderDef {
            name_ja: "ロバート1世 直率",
            archetype: "professional_marshal",
        },
        troop_type: 1,
        quality: Quality::Elite,
        loadout: Loadout::Pikemen,
        count: 260,
        files: 40,
        formation: FORMATION_SCHILTRON,
        // 陣形プリセットのファイル間隔（0.5 m）は兵士の当たり判定の直径
        // （0.7 m）より狭く、そのまま使うと隊列が常時重なって自分で圧死する
        // （`sim-headless` の `form_battle_units` も同じ理由で 0.8 m を明示
        // している）。
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: 725,
        front_y_m: BB_SCOTS_FRONT_Y,
        facing: 0,
        hosts_army_commander: true,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "キース卿の軽騎兵",
        commander: CommanderDef {
            name_ja: "ロバート・キース",
            archetype: "cunning_captain",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 90,
        files: 18,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: 750,
        front_y_m: 520,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
];

/// イングランド軍。湿地の中で騎兵も弓兵も持て余す。
/// 正面 `facing = BRAD_HALF` はランクが -Y（南＝スコットランド軍側）へ伸びる。
const BB_ENGLISH_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "前衛騎兵（グロスター伯）",
        commander: CommanderDef {
            name_ja: "グロスター伯ギルバート・ド・クレア",
            // 支援を待たずに単独で突っ込み、討たれた。
            archetype: "reckless_youth",
        },
        troop_type: 3,
        quality: Quality::Elite,
        loadout: Loadout::MountedKnights,
        count: 400,
        files: 40,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: 545,
        front_y_m: BB_ENGLISH_FRONT_Y,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "騎兵（ヘレフォード伯）",
        commander: CommanderDef {
            name_ja: "ヘレフォード伯ハンフリー・ド・ボーハン",
            archetype: "honor_hungry_knight",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 300,
        files: 30,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: 665,
        front_y_m: BB_ENGLISH_FRONT_Y,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "本隊（国王）",
        commander: CommanderDef {
            name_ja: "エドワード2世 直率",
            archetype: "stubborn_baron",
        },
        troop_type: 0,
        quality: Quality::Professional,
        loadout: Loadout::FootMenAtArms,
        count: 900,
        files: 90,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: BB_CENTER_X,
        front_y_m: 760,
        facing: BRAD_HALF as u16,
        hosts_army_commander: true,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "長弓隊",
        commander: CommanderDef {
            name_ja: "弓兵隊長",
            // 展開する場所を与えられないまま軽騎兵に蹴散らされた。
            archetype: "cautious_commander",
        },
        troop_type: 2,
        // 展開する場所も指揮も与えられなかった一団。練度も落として扱う。
        quality: Quality::Militia,
        loadout: Loadout::Longbowmen,
        count: 350,
        files: 35,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 700,
        front_y_m: 930,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "徴募歩兵",
        commander: CommanderDef {
            name_ja: "州徴募隊長",
            archetype: "disciplinarian",
        },
        troop_type: 1,
        quality: Quality::Levy,
        loadout: Loadout::Spearmen,
        count: 600,
        files: 60,
        formation: FORMATION_SHIELDWALL,
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: 520,
        front_y_m: 870,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
];

// ----------------------------------------------------------------------
// クレシー 1346
// ----------------------------------------------------------------------
//
// 数値は `data/scenarios/crecy_1346.toml` と同じものを持つ。

/// 地図の一辺（m）。アジンクールより広い開けた斜面。
const CR_SIZE_M: i32 = 1400;
/// イングランド軍・重装兵の最前列（南、斜面の上）。
const CR_ENGLISH_FRONT_Y: i32 = 440;
/// 前方へ張り出した長弓兵の最前列。
const CR_ARCHER_FRONT_Y: i32 = 470;
/// ジェノヴァ弩兵の最前列（北、斜面の下）。
const CR_GENOESE_FRONT_Y: i32 = 700;
/// 会戦場の中心線（東西）。
const CR_CENTER_X: i32 = 700;

/// クレシーの会戦（1346 年 8 月 26 日）。
///
/// アジンクールが「狭い泥の隘路」なら、こちらは**開けた緩斜面**。挟み込む
/// 森は無く、効いているのは高低差・射程差・そして到着順に突っ込む指揮の
/// 混乱になる。
///
/// - 長弓（射程 120 m・発射間隔 3.5 秒）とジェノヴァの弩（80 m・6 秒）が
///   撃ち合う。射程と手数の差がそのまま出る
/// - 長弓兵の前には掘った落とし穴（`StructureKind::Ditch`）。堀は騎兵の
///   進入そのものを塞ぐ（`hard_blocks_cavalry`）
/// - 仏軍は行軍からそのまま到着し、弩兵の頭越しに騎兵が突入した。この
///   「順番待ちのできない軍」を、前後に重ねた 4 つの部隊と、名誉に飢えた
///   指揮官たちで表現する
///
/// 雨で弩の弦が伸びていた点は、弩兵の練度を `Militia` に落として代用して
/// いる（天候はまだ実装されていない）。
pub const CRECY_1346: ScenarioDef = ScenarioDef {
    id: "crecy_1346",
    name_ja: "クレシーの会戦",
    name_en: "Battle of Crécy",
    year: 1346,
    place_ja: "北フランス・ポンチュー",
    summary_ja: "開けた緩斜面。上に立つ英軍の長弓が、順に到着する仏軍を迎え撃つ。",
    historical_strength_ja: "史実の推定兵力: 英 約14,000 / 仏 約25,000〜30,000",
    scale_note_ja: "ブラウザで回せるよう、兵数と会戦場の幅をおよそ 1/8 に縮尺してある",
    terrain: TerrainParams {
        seed: 0xC5EC_1346_0000_0001,
        size_m: CR_SIZE_M as u32,
        cell_m: 2,
        relief: 0,
        forest_cover: 250,
        marsh_bias: 120,
        thermal_iterations: 6,
        river_density: 0,
        road_count: 1,
        sea_edge: SeaEdge::None,
        sea_level_cm: 0,
    },
    shaping: CRECY_SHAPING,
    armies: CRECY_ARMIES,
};

/// クレシーの地勢。開けた耕地と、南（英軍側）へ登る緩斜面。挟む森は西側だけ
/// （クレシーの森）で、東は開いたまま——アジンクールとの対比になる。
const CRECY_SHAPING: &[Stamp] = &[
    // 会戦場の水を抜いてから耕地を敷く（`Stamp::Drain` の注記を参照）。
    Stamp::Drain {
        surface: Surface::Farmland,
        x_m: 350,
        y_m: 200,
        w_m: 700,
        h_m: 960,
    },
    Stamp::SurfaceRect {
        surface: Surface::Farmland,
        x_m: 350,
        y_m: 200,
        w_m: 700,
        h_m: 960,
    },
    // 8 月の刈り入れ後。踏み固められてはいるが泥濘ではない。
    Stamp::SurfaceRect {
        surface: Surface::Meadow,
        x_m: 420,
        y_m: 480,
        w_m: 560,
        h_m: 240,
    },
    // 西のクレシーの森。英軍の右翼を守る。
    Stamp::SurfaceBelt {
        surface: Surface::DenseForest,
        ax_m: 300,
        ay_m: 1400,
        bx_m: 340,
        by_m: 0,
        half_width_m: 120,
    },
    // 斜面。英軍の立つ南が 9 m 高い。登る側は疲れ、下る側の射撃は届く。
    Stamp::HeightRamp {
        axis: RampAxis::Y,
        from_m: 300,
        to_m: 900,
        from_cm: 900,
        to_cm: 0,
    },
];

const CRECY_ARMIES: &[ArmyDef] = &[
    ArmyDef {
        faction: 0,
        name_ja: "イングランド軍",
        commander: CommanderDef {
            name_ja: "エドワード3世",
            archetype: "professional_marshal",
        },
        battle_plan: Some(BattlePlan::DefendHighGround),
        contingents: CR_ENGLISH_CONTINGENTS,
    },
    ArmyDef {
        faction: 1,
        name_ja: "フランス軍",
        commander: CommanderDef {
            name_ja: "フィリップ6世",
            archetype: "stubborn_baron",
        },
        battle_plan: Some(BattlePlan::CenterPush),
        contingents: CRECY_FRENCH_CONTINGENTS,
    },
];

/// 英軍。両翼の長弓兵が重装兵より前へ張り出す（ハロー隊形）。その前には
/// 掘った落とし穴。後方に国王の予備。
const CR_ENGLISH_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "左翼 長弓隊",
        commander: CommanderDef {
            name_ja: "アランデル伯",
            archetype: "veteran_mercenary",
        },
        troop_type: 2,
        quality: Quality::Professional,
        loadout: Loadout::Longbowmen,
        count: 500,
        files: 50,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 610,
        front_y_m: CR_ARCHER_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: Some(ObstacleDef {
            kind: StructureKind::Ditch,
            ahead_m: 10,
        }),
    },
    ContingentDef {
        name_ja: "黒太子隊（下馬重装兵）",
        commander: CommanderDef {
            name_ja: "ウォリック伯（黒太子後見）",
            archetype: "disciplinarian",
        },
        troop_type: 0,
        quality: Quality::Elite,
        loadout: Loadout::PollaxeMenAtArms,
        count: 300,
        files: 50,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: CR_CENTER_X,
        front_y_m: CR_ENGLISH_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "右翼 長弓隊",
        commander: CommanderDef {
            name_ja: "ノーサンプトン伯",
            archetype: "veteran_mercenary",
        },
        troop_type: 2,
        quality: Quality::Professional,
        loadout: Loadout::Longbowmen,
        count: 500,
        files: 50,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: 790,
        front_y_m: CR_ARCHER_FRONT_Y,
        facing: 0,
        hosts_army_commander: false,
        obstacle: Some(ObstacleDef {
            kind: StructureKind::Ditch,
            ahead_m: 10,
        }),
    },
    ContingentDef {
        name_ja: "第二陣（下馬重装兵）",
        commander: CommanderDef {
            name_ja: "ノーサンプトン伯麾下",
            archetype: "veteran_mercenary",
        },
        troop_type: 0,
        quality: Quality::Veteran,
        loadout: Loadout::PollaxeMenAtArms,
        count: 250,
        files: 50,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: CR_CENTER_X,
        front_y_m: 415,
        facing: 0,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "国王予備",
        commander: CommanderDef {
            name_ja: "エドワード3世 直率",
            // 「息子に拍車を勝ち取らせよ」と援軍を送らなかった王。
            archetype: "cautious_commander",
        },
        troop_type: 0,
        quality: Quality::Elite,
        loadout: Loadout::PollaxeMenAtArms,
        count: 200,
        files: 40,
        formation: FORMATION_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: CR_CENTER_X,
        front_y_m: 370,
        facing: 0,
        hosts_army_commander: true,
        obstacle: None,
    },
];

/// 仏軍。弩兵を先に立て、その後ろに騎兵が前後 3 段で詰めている。
const CRECY_FRENCH_CONTINGENTS: &[ContingentDef] = &[
    ContingentDef {
        name_ja: "ジェノヴァ弩兵",
        commander: CommanderDef {
            name_ja: "オットーネ・ドーリア",
            archetype: "veteran_mercenary",
        },
        troop_type: 2,
        // 長い行軍の直後に、パヴィス（大盾）を輜重に置いたまま前へ出された。
        // 雨で弦が伸びていたぶんも含めて練度を落として表現する。
        quality: Quality::Militia,
        loadout: Loadout::Crossbowmen,
        count: 700,
        files: 70,
        formation: FORMATION_PAVISE_LINE,
        file_spacing_mm: 0,
        rank_spacing_mm: 0,
        front_x_m: CR_CENTER_X,
        front_y_m: CR_GENOESE_FRONT_Y,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "アランソン公騎兵",
        commander: CommanderDef {
            name_ja: "アランソン公シャルル",
            // 弩兵の頭越しに突っ込んだ。
            archetype: "honor_hungry_knight",
        },
        troop_type: 3,
        quality: Quality::Elite,
        loadout: Loadout::MountedKnights,
        count: 350,
        files: 50,
        formation: FORMATION_LINE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: CR_CENTER_X - 140,
        front_y_m: 820,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "ボヘミア王隊",
        commander: CommanderDef {
            name_ja: "ボヘミア王ヨハン（盲目王）",
            archetype: "reckless_youth",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 300,
        files: 50,
        formation: FORMATION_WEDGE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: CR_CENTER_X + 140,
        front_y_m: 820,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "第二陣騎兵",
        commander: CommanderDef {
            name_ja: "フィリップ6世 直率",
            archetype: "stubborn_baron",
        },
        troop_type: 3,
        quality: Quality::Veteran,
        loadout: Loadout::MountedKnights,
        count: 350,
        files: 50,
        formation: FORMATION_LINE,
        file_spacing_mm: 2500,
        rank_spacing_mm: 2500,
        front_x_m: CR_CENTER_X - 70,
        front_y_m: 960,
        facing: BRAD_HALF as u16,
        hosts_army_commander: true,
        obstacle: None,
    },
    ContingentDef {
        name_ja: "徴募歩兵",
        commander: CommanderDef {
            name_ja: "民兵隊長",
            archetype: "cautious_commander",
        },
        troop_type: 1,
        quality: Quality::Levy,
        loadout: Loadout::Spearmen,
        count: 600,
        files: 60,
        formation: FORMATION_SHIELDWALL,
        file_spacing_mm: 800,
        rank_spacing_mm: 800,
        front_x_m: CR_CENTER_X,
        front_y_m: 1090,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        obstacle: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organization::CommandState;
    use sim_terrain::SURFACE_EFFECTS;

    fn agincourt() -> World {
        build_world(&AGINCOURT_1415)
    }

    #[test]
    fn registry_is_addressable_by_id_and_index() {
        let index = index_of("agincourt_1415").expect("アジンクールが登録されていない");
        assert_eq!(get(index).map(|s| s.id), Some("agincourt_1415"));
        assert!(get(SCENARIOS.len()).is_none());
    }

    #[test]
    fn every_scenario_is_internally_consistent() {
        for def in SCENARIOS {
            assert!(!def.armies.is_empty(), "{}: 軍がいない", def.id);
            for army in def.armies {
                assert!(
                    archetype_index(army.commander.archetype).is_some(),
                    "{}: 未知のアーキタイプ {}",
                    def.id,
                    army.commander.archetype
                );
                assert!(!army.contingents.is_empty(), "{}: 部隊がいない", def.id);
                for cont in army.contingents {
                    assert!(cont.count > 0, "{}/{}: 兵数 0", def.id, cont.name_ja);
                    assert!(cont.files > 0, "{}/{}: files 0", def.id, cont.name_ja);
                    assert!(
                        archetype_index(cont.commander.archetype).is_some(),
                        "{}/{}: 未知のアーキタイプ {}",
                        def.id,
                        cont.name_ja,
                        cont.commander.archetype
                    );
                    let size = def.terrain.size_m as i32;
                    assert!(
                        cont.front_x_m > 0
                            && cont.front_x_m < size
                            && cont.front_y_m > 0
                            && cont.front_y_m < size,
                        "{}/{}: 配置が地図の外",
                        def.id,
                        cont.name_ja
                    );
                }
            }
        }
    }

    #[test]
    fn deploys_the_declared_number_of_soldiers_into_a_two_level_command_tree() {
        for def in SCENARIOS {
            let w = build_world(def);
            assert_eq!(w.soldiers.len() as u32, def.soldier_count(), "{}", def.id);

            let roots: Vec<_> = w
                .command
                .nodes
                .iter()
                .filter(|n| n.parent.is_none())
                .collect();
            assert_eq!(roots.len(), def.armies.len(), "{}: 軍ノードの数", def.id);
            for root in &roots {
                assert!(root.unit.is_none(), "{}: 軍ノードは実体を持たない", def.id);
                assert!(!root.children.is_empty(), "{}", def.id);
                assert_eq!(root.command_state, CommandState::Commanded, "{}", def.id);
            }
            let leaves = w.command.nodes.iter().filter(|n| n.unit.is_some()).count();
            assert_eq!(
                leaves,
                def.armies
                    .iter()
                    .map(|a| a.contingents.len())
                    .sum::<usize>(),
                "{}",
                def.id
            );
        }
    }

    #[test]
    fn soldiers_start_on_their_formation_slots() {
        for def in SCENARIOS {
            let mut w = build_world(def);
            let before: Vec<Vec2Fx> = (0..w.soldiers.len()).map(|i| w.soldiers.pos(i)).collect();
            w.tick();
            // 陣形スロットの上に生成しているので、開始直後に隊列を組み直す動き
            // （数十 cm 以上の移動）は起きない。押し合いが起きる配置は、
            // 部隊の間隔か陣形の間隔が足りていないということ。
            for (i, &was) in before.iter().enumerate() {
                let moved = sim_math::fx_to_mm(sim_math::dist(was, w.soldiers.pos(i)));
                assert!(
                    moved < 300,
                    "{}: 兵士 {i} が開始直後に {moved} mm 動いた",
                    def.id
                );
            }
        }
    }

    #[test]
    fn commanders_have_the_personality_the_scenario_asked_for() {
        let w = agincourt();
        let english_root = w
            .command
            .nodes
            .iter()
            .find(|n| n.parent.is_none() && n.faction == 0)
            .unwrap();
        let french_root = w
            .command
            .nodes
            .iter()
            .find(|n| n.parent.is_none() && n.faction == 1)
            .unwrap();
        // professional_marshal は stubborn_baron より従順で技量が高い。
        assert!(
            english_root.commander_attrs.tactical_skill
                > french_root.commander_attrs.tactical_skill
        );
        assert!(english_root.commander_attrs.obedience > french_root.commander_attrs.obedience);
        assert_eq!(english_root.battle_plan, Some(BattlePlan::DefendHighGround));
        assert_eq!(french_root.battle_plan, Some(BattlePlan::CenterPush));

        // 仏軍前衛は名誉に飢えた騎士——野心が高く従順さが低い＝独断専行しやすい。
        let vanguard = w
            .command
            .nodes
            .iter()
            .find(|n| n.faction == 1 && n.unit.is_some())
            .unwrap();
        assert!(vanguard.commander_attrs.ambition > vanguard.commander_attrs.obedience);
    }

    #[test]
    fn the_field_is_muddy_between_the_lines_and_wooded_on_both_flanks() {
        let w = agincourt();
        let t = &w.terrain;
        let surface_at = |x: i32, y: i32| t.surface_at(fx(x), fx(y));

        // 両軍の間は泥。移動が鈍り、疲労が大きい。
        let mid_y = (AG_ENGLISH_FRONT_Y + AG_FRENCH_FRONT_Y) / 2;
        assert_eq!(surface_at(AG_CENTER_X, mid_y), Surface::Mud);
        let mud = &SURFACE_EFFECTS[Surface::Mud as usize];
        let farmland = &SURFACE_EFFECTS[Surface::Farmland as usize];
        assert!(mud.move_mult < farmland.move_mult);
        assert!(mud.fatigue_mult > farmland.fatigue_mult);

        // 両翼は深い森で、会戦場は英軍正面へ向かって狭まる。
        assert_eq!(surface_at(380, mid_y), Surface::DenseForest);
        assert_eq!(surface_at(820, mid_y), Surface::DenseForest);
        let width_at = |y: i32| {
            let mut count = 0;
            for x in 300..900 {
                if surface_at(x, y) != Surface::DenseForest {
                    count += 1;
                }
            }
            count
        };
        assert!(
            width_at(AG_ENGLISH_FRONT_Y) < width_at(1000),
            "会戦場が英軍正面へ向かって狭まっていない"
        );
    }

    #[test]
    fn the_english_stand_on_slightly_higher_ground() {
        let w = agincourt();
        let english = w.terrain.height_at(fx(AG_CENTER_X), fx(AG_ENGLISH_FRONT_Y));
        let french = w.terrain.height_at(fx(AG_CENTER_X), fx(AG_FRENCH_FRONT_Y));
        let diff_mm = sim_math::fx_to_mm(english - french);
        assert!(
            diff_mm > 500,
            "英軍が高地に立っていない（高低差 {diff_mm} mm）"
        );
    }

    #[test]
    fn the_longbow_wings_stand_behind_completed_stakes() {
        let w = agincourt();
        let stakes: Vec<_> = w
            .structures
            .structures
            .iter()
            .filter(|s| s.kind == StructureKind::Stakes)
            .collect();
        // 1 本の長い線分ではなく、耐久が独立した短い区画の連なりになっている
        // （数騎の突破で防御線が丸ごと消えないように）。
        assert!(stakes.len() >= 20, "杭列が区画に分かれていない");
        for s in &stakes {
            assert_eq!(s.owner, 0);
            assert_eq!(s.completion_permille, 1000, "会戦前に打ち終えている");
            // 杭列は長弓兵の前（＝仏軍側）にある。
            assert!(sim_math::fx_to_mm(s.a.y) / 1000 > AG_ENGLISH_FRONT_Y);
        }

        // 両翼それぞれの正面を、隙間なく覆っている。
        for wing_center_x in [530, 675] {
            let covered: Vec<&&crate::structures::Structure> = stakes
                .iter()
                .filter(|s| {
                    let mid_x = sim_math::fx_to_mm(s.a.x + s.b.x) / 2000;
                    (mid_x - wing_center_x).abs() < 40
                })
                .collect();
            assert!(
                covered.len() >= 10,
                "x={wing_center_x} の長弓隊の正面が杭列で覆われていない"
            );
        }
    }

    #[test]
    fn troops_carry_the_equipment_their_contingent_declares() {
        let w = agincourt();
        let mounted = (0..w.soldiers.len())
            .filter(|&i| w.soldiers.hot.flags[i] & flags::MOUNTED != 0)
            .count();
        assert_eq!(mounted, 650, "仏軍の騎兵 3 隊ぶん");

        let archers: Vec<usize> = (0..w.soldiers.len())
            .filter(|&i| w.combat.weapons[i].ranged)
            .collect();
        assert_eq!(archers.len(), 1400, "長弓隊 2 隊ぶん");
        for &i in &archers {
            assert_eq!(w.soldiers.faction[i], 0, "長弓兵は英軍だけ");
            assert!(w.combat.ammo[i] > 0);
        }
    }

    #[test]
    fn the_same_scenario_always_produces_the_same_world() {
        for def in SCENARIOS {
            let mut a = build_world(def);
            let mut b = build_world(def);
            assert_eq!(a.terrain.hash(), b.terrain.hash(), "{}", def.id);
            assert_eq!(a.state_hash(), b.state_hash(), "{}", def.id);
            for _ in 0..120 {
                a.tick();
                b.tick();
            }
            assert_eq!(a.state_hash(), b.state_hash(), "{}: 120 tick 後", def.id);
        }
    }

    /// どのプリセットでも、兵士は最初から**歩ける地面**の上に立っている。
    ///
    /// 手続き生成の湖が展開地にかかっていると、そこだけ騎兵が完全に止まり
    /// （`cavalry_mult` 0）、隊列が押し合ったまま圧死していく。目に見える
    /// 戦闘が起きないまま片方の軍が溶けるので、必ずここで捕まえる。
    #[test]
    fn every_soldier_starts_on_ground_they_can_stand_on() {
        for def in SCENARIOS {
            let w = build_world(def);
            for i in 0..w.soldiers.len() {
                let pos = w.soldiers.pos(i);
                let (cx, cy) = w.terrain.world_to_cell(pos.x, pos.y);
                let idx = w.terrain.idx(cx, cy);
                let surface = Surface::from_u8(w.terrain.surface[idx]);
                assert!(
                    w.terrain.passability[idx] > 0,
                    "{}: 兵士 {i} が通行不能な {surface:?} の上にいる",
                    def.id
                );
                assert_eq!(
                    w.terrain.water[idx], 0,
                    "{}: 兵士 {i} が水域（{surface:?}）の上にいる",
                    def.id
                );
                assert!(
                    SURFACE_EFFECTS[surface as usize].cavalry_mult > 0
                        || w.soldiers.hot.flags[i] & flags::MOUNTED == 0,
                    "{}: 騎兵 {i} が馬の入れない {surface:?} の上にいる",
                    def.id
                );
            }
        }
    }

    /// プリセットが違えば、地形も陣容も別物になっている（同じ地形を使い回して
    /// いないことの確認）。
    #[test]
    fn presets_describe_distinct_battles() {
        let mut seen_terrain = Vec::new();
        let mut seen_id = Vec::new();
        for def in SCENARIOS {
            let w = build_world(def);
            assert!(!seen_id.contains(&def.id), "id が重複している: {}", def.id);
            seen_id.push(def.id);
            let hash = w.terrain.hash();
            assert!(
                !seen_terrain.contains(&hash),
                "{} の地形が他のプリセットと同一",
                def.id
            );
            seen_terrain.push(hash);
        }
    }

    /// バノックバーン: イングランド軍は騎兵の使えない低湿地に立ち、退路は
    /// 北へ行くほど狭まる。
    #[test]
    fn bannockburn_pins_the_english_in_a_carse_hemmed_by_impassable_marsh() {
        let w = build_world(&BANNOCKBURN_1314);
        let surface_at = |x: i32, y: i32| w.terrain.surface_at(fx(x), fx(y));

        // 立っているのは軟らかい泥。動けるが、遅く、疲れる。
        assert_eq!(
            surface_at(BB_CENTER_X, BB_ENGLISH_FRONT_Y + 40),
            Surface::Mud,
            "イングランド軍がカースに立っていない"
        );
        let mud = &SURFACE_EFFECTS[Surface::Mud as usize];
        assert!(mud.cavalry_mult > 0 && mud.cavalry_mult < 500);
        assert!(mud.fatigue_mult > 1500);
        // 左右と背後の小川は湿地。馬は 1 mm も入れない。
        assert_eq!(surface_at(400, 900), Surface::Marsh);
        assert_eq!(surface_at(800, 900), Surface::Marsh);
        let marsh = &SURFACE_EFFECTS[Surface::Marsh as usize];
        assert_eq!(marsh.cavalry_mult, 0);
        assert!(marsh.fatigue_mult > 2000);
        // スコットランド側は乾いた牧草地。
        assert_eq!(
            surface_at(BB_CENTER_X, BB_SCOTS_FRONT_Y - 40),
            Surface::Meadow
        );

        // 退路（北）は小川に挟まれて狭まっていく。
        let open_width_at = |y: i32| {
            (300..900)
                .filter(|&x| surface_at(x, y) != Surface::Marsh)
                .count()
        };
        let _ = open_width_at(0);
        assert!(
            open_width_at(1000) < open_width_at(BB_ENGLISH_FRONT_Y),
            "イングランド軍の背後が狭まっていない"
        );

        // 主力はパイクのシルトロン。後列も突ける間合いが騎兵の忌避を誘う。
        let pikes = (0..w.soldiers.len())
            .filter(|&i| w.soldiers.faction[i] == 0 && w.combat.weapons[i] == Weapon::pike())
            .count();
        assert_eq!(pikes, 1040, "シルトロン 4 個ぶんのパイク");
        assert!(!Weapon::pike().requires_front_row);
    }

    /// クレシー: 英軍は斜面の上に立ち、長弓はジェノヴァの弩より遠くへ届く。
    /// 長弓兵の前には騎兵を通さない落とし穴がある。
    #[test]
    fn crecy_gives_the_longbows_range_and_height_over_the_crossbows() {
        let w = build_world(&CRECY_1346);

        let english_h = w.terrain.height_at(fx(CR_CENTER_X), fx(CR_ENGLISH_FRONT_Y));
        let genoese_h = w.terrain.height_at(fx(CR_CENTER_X), fx(CR_GENOESE_FRONT_Y));
        let diff_mm = sim_math::fx_to_mm(english_h - genoese_h);
        assert!(
            diff_mm > 2_000,
            "英軍が斜面の上に立っていない（{diff_mm} mm）"
        );

        assert!(
            Weapon::longbow().reach > Weapon::crossbow().reach,
            "長弓が弩より短射程になっている"
        );

        // 落とし穴は騎兵の進入そのものを塞ぐ。
        let ditches: Vec<_> = w
            .structures
            .structures
            .iter()
            .filter(|s| s.kind == StructureKind::Ditch)
            .collect();
        assert!(ditches.len() >= 18, "長弓隊 2 隊ぶんの落とし穴が足りない");
        assert!(StructureKind::Ditch.hard_blocks_cavalry());
        for d in &ditches {
            assert_eq!(d.owner, 0);
            assert_eq!(d.completion_permille, 1000);
            assert!(sim_math::fx_to_mm(d.a.y) / 1000 > CR_ARCHER_FRONT_Y);
        }

        // 仏軍の第一線は弩兵、その後ろは騎兵。
        let crossbows = (0..w.soldiers.len())
            .filter(|&i| w.soldiers.faction[i] == 1 && w.combat.weapons[i] == Weapon::crossbow())
            .count();
        assert_eq!(crossbows, 700);
        let french_mounted = (0..w.soldiers.len())
            .filter(|&i| {
                w.soldiers.faction[i] == 1 && w.soldiers.hot.flags[i] & flags::MOUNTED != 0
            })
            .count();
        assert_eq!(french_mounted, 1000, "コンロワ 3 隊ぶん");
    }

    /// 会戦が「勝手に始まる」ことの確認。命令を一切与えていないのに、性格の
    /// 違う両軍の指揮官 AI が別々の判断を下し、仏軍が動き出す。
    #[test]
    fn the_battle_starts_from_the_commanders_own_decisions() {
        let mut w = agincourt();
        let french_start: Vec<Vec2Fx> = w
            .command
            .nodes
            .iter()
            .filter(|n| n.faction == 1 && n.unit.is_some())
            .map(|n| n.stats.centroid)
            .collect();

        for _ in 0..600 {
            w.tick();
        }

        let decisions: usize = w
            .command
            .nodes
            .iter()
            .map(|n| n.decision_log.iter().count())
            .sum();
        assert!(decisions > 0, "どの指揮官も判断していない");

        let french_now: Vec<Vec2Fx> = w
            .command
            .nodes
            .iter()
            .filter(|n| n.faction == 1 && n.unit.is_some())
            .map(|n| n.stats.centroid)
            .collect();
        let advanced = french_start
            .iter()
            .zip(&french_now)
            .any(|(before, now)| sim_math::fx_to_mm(sim_math::dist(*before, *now)) > 5_000);
        assert!(advanced, "仏軍がどの部隊も前進していない");
    }
}
