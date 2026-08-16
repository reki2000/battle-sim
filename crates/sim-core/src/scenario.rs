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
    FORMATION_WEDGE,
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
            Loadout::Spearmen | Loadout::Crossbowmen | Loadout::MountedKnights => Armor::mail(),
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
    /// 馬の当たり判定（半径 90 cm）に合う広さをここで指定する。
    pub file_spacing_mm: u16,
    pub rank_spacing_mm: u16,
    /// 最前列の中央（m）。隊列は `facing` の逆向きに奥行きぶん伸びる。
    pub front_x_m: i32,
    pub front_y_m: i32,
    /// 部隊の正面。`0` でランクが +Y 方向へ伸びる（`formation_goals` の規約）。
    pub facing: Brad,
    /// この部隊に軍司令官が随伴するか。どの部隊にも無ければ先頭の部隊。
    pub hosts_army_commander: bool,
    /// 会戦前に完成している杭列を、最前列の何 m 前方に立てるか。0 なら立てない。
    pub stakes_ahead_m: u16,
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
pub static SCENARIOS: &[ScenarioDef] = &[AGINCOURT_1415];

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
            if cont.stakes_ahead_m > 0 {
                plant_stakes(world, cont, army.faction);
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

/// 最前列の前方へ、完成済みの杭列を立てる（アジンクールの長弓兵）。
///
/// 会戦前の準備時間（仕様 07 章 7 節）で工兵が打ち終えた状態を、工事を待たずに
/// 再現するためのもの。
fn plant_stakes(world: &mut World, cont: &ContingentDef, faction: FactionId) {
    let l = layout(cont);
    let front_center = Vec2Fx::new(fx(cont.front_x_m), fx(cont.front_y_m));
    let center = front_center.add(l.forward.scale(fx(cont.stakes_ahead_m as i32)));
    let half_width = fx_mul(fx(l.files.saturating_sub(1) as i32), l.file_spacing) / 2;
    let a = center.sub(l.right.scale(half_width));
    let b = center.add(l.right.scale(half_width));
    let id = world.structures.build(StructureKind::Stakes, a, b, faction);
    world.structures.set_completion(id, 1000);
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
        stakes_ahead_m: 8,
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
        stakes_ahead_m: 0,
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
        stakes_ahead_m: 0,
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
        stakes_ahead_m: 0,
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
        stakes_ahead_m: 8,
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
        stakes_ahead_m: 0,
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
        file_spacing_mm: 2000,
        rank_spacing_mm: 2000,
        front_x_m: 515,
        front_y_m: 640,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        stakes_ahead_m: 0,
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
        file_spacing_mm: 2000,
        rank_spacing_mm: 2000,
        front_x_m: 690,
        front_y_m: 640,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        stakes_ahead_m: 0,
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
        stakes_ahead_m: 0,
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
        file_spacing_mm: 2000,
        rank_spacing_mm: 2000,
        front_x_m: AG_CENTER_X,
        front_y_m: 760,
        facing: BRAD_HALF as u16,
        hosts_army_commander: false,
        stakes_ahead_m: 0,
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
        let w = agincourt();
        assert_eq!(w.soldiers.len() as u32, AGINCOURT_1415.soldier_count());

        let roots: Vec<_> = w
            .command
            .nodes
            .iter()
            .filter(|n| n.parent.is_none())
            .collect();
        assert_eq!(roots.len(), 2, "軍のルートノードは陣営ごとに 1 つ");
        for root in &roots {
            assert!(root.unit.is_none(), "軍ノードは実体を持たない");
            assert!(!root.children.is_empty());
            assert_eq!(root.command_state, CommandState::Commanded);
        }
        let leaves = w.command.nodes.iter().filter(|n| n.unit.is_some()).count();
        assert_eq!(
            leaves,
            AGINCOURT_1415
                .armies
                .iter()
                .map(|a| a.contingents.len())
                .sum::<usize>()
        );
    }

    #[test]
    fn soldiers_start_on_their_formation_slots() {
        let mut w = agincourt();
        let before: Vec<Vec2Fx> = (0..w.soldiers.len()).map(|i| w.soldiers.pos(i)).collect();
        w.tick();
        // 陣形スロットの上に生成しているので、開始直後に隊列を組み直す動き
        // （数十 cm 以上の移動）は起きない。
        for (i, &was) in before.iter().enumerate() {
            let moved = sim_math::fx_to_mm(sim_math::dist(was, w.soldiers.pos(i)));
            assert!(moved < 300, "兵士 {i} が開始直後に {moved} mm 動いた");
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
        assert_eq!(stakes.len(), 2, "長弓隊 2 隊ぶんの杭列");
        for s in &stakes {
            assert_eq!(s.owner, 0);
            assert_eq!(s.completion_permille, 1000, "会戦前に打ち終えている");
            // 杭列は長弓兵の前（＝仏軍側）にある。
            assert!(sim_math::fx_to_mm(s.a.y) / 1000 > AG_ENGLISH_FRONT_Y);
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
        let a = agincourt();
        let b = agincourt();
        assert_eq!(a.terrain.hash(), b.terrain.hash());
        assert_eq!(a.state_hash(), b.state_hash());

        let mut a = a;
        let mut b = b;
        for _ in 0..120 {
            a.tick();
            b.tick();
        }
        assert_eq!(a.state_hash(), b.state_hash());
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
