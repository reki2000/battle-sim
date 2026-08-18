//! ブラウザなしでシミュレーションを回す CLI。
//!
//! バランス調整・性能計測・決定論の回帰テストをネイティブの速度で行うため。
//! CI はこれを使って性能とハッシュの回帰を検出する（仕様 11 章 6 節）。
//!
//! ```text
//! sim-headless bench    --soldiers 20000 --ticks 2000
//! sim-headless verify   --ticks 5000
//! sim-headless terrain  --size 2000 --relief 600
//! sim-headless battle   --soldiers 4000                 会戦を最後まで回して死因・所要時間を見る
//! sim-headless winrate  --soldiers 2000 --runs 200       対称な兵力を繰り返し戦わせて勝率を見る
//! sim-headless prep     --soldiers 2000 --runs 100       準備時間の長短が防御側の勝率に効くか見る（M6）
//! sim-headless scenario --scenario agincourt_1415        史実の会戦プリセットを最後まで回す
//! ```

use std::time::Instant;

use sim_core::combat::BattlePhase;
use sim_core::soldiers::{flags, Attrs};
use sim_core::structures::StructureKind;
use sim_core::{deploy_block, World};
use sim_math::{fx, fx_from_mm, Vec2Fx};
use sim_terrain::Terrain;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let opts = Opts::parse(&args);

    match cmd {
        "bench" => bench(&opts),
        "verify" => verify(&opts),
        "terrain" => terrain_report(&opts),
        "battle" => battle(&opts),
        "winrate" => winrate(&opts),
        "prep" => prep_experiment(&opts),
        "scenario" => scenario_run(&opts),
        _ => {
            eprintln!(
                "使い方:\n  \
                 bench   [--soldiers N] [--ticks N] [--size M] [--seed N]   性能を計測する\n  \
                 verify  [--soldiers N] [--ticks N] [--seed N]              決定論を検証する\n  \
                 terrain --scenario ID                                     固定地形（data/terrain/*.bin）の統計を出す\n          \
                 地形の生成は web/src/terrain にあり、tools/gen_terrain_fixtures.mjs で書き出す\n  \
                 battle  [--soldiers N] [--seed N] [--max-ticks N]          会戦を最後まで回し、死因内訳・所要時間を出す（M4 受け入れ条件）\n  \
                 winrate [--soldiers N] [--runs N] [--max-ticks N]          対称な兵力で繰り返し会戦し、勝率が 50%±8% に収まるか見る\n  \
                 prep    [--soldiers N] [--runs N] [--prep-short N] [--prep-long N]\n          \
                 防御側（陣営 1）に杭列・堀を築かせる準備時間を短/長で比較し、\n          \
                 勝率が有意に変わるか見る（M6 受け入れ条件）\n  \
                 scenario [--scenario ID] [--max-ticks N]                  史実の会戦プリセットを決着まで回す\n          \
                 ID を省略すると登録されているプリセットの一覧を出す"
            );
            std::process::exit(2);
        }
    }
}

struct Opts {
    soldiers: u32,
    ticks: u32,
    size_m: u32,
    relief: u16,
    seed: u64,
    runs: u32,
    max_ticks: u32,
    prep_short: u32,
    prep_long: u32,
    scenario: Option<String>,
}

impl Opts {
    fn parse(args: &[String]) -> Opts {
        let mut o = Opts {
            soldiers: 5_000,
            ticks: 1_000,
            size_m: 2_000,
            relief: 450,
            seed: 0x5EED_1234_ABCD_0001,
            runs: 200,
            // 仕様の受け入れ条件（会戦は 20 分〜3 時間）の上限に、判定の余白を
            // 少し足した値。ここに達したら「3 時間以内に終わらなかった」とみなす。
            max_ticks: 4 * 3600 * 20,
            // 準備なし（0 分）と、たっぷり準備できた場合（12.5 分）の対比。
            prep_short: 0,
            prep_long: 15_000,
            scenario: None,
        };
        let mut i = 1;
        while i + 1 < args.len() {
            let v = &args[i + 1];
            match args[i].as_str() {
                "--soldiers" => o.soldiers = v.parse().unwrap_or(o.soldiers),
                "--ticks" => o.ticks = v.parse().unwrap_or(o.ticks),
                "--size" => o.size_m = v.parse().unwrap_or(o.size_m),
                "--relief" => o.relief = v.parse().unwrap_or(o.relief),
                "--seed" => o.seed = parse_seed(v).unwrap_or(o.seed),
                "--runs" => o.runs = v.parse().unwrap_or(o.runs),
                "--max-ticks" => o.max_ticks = v.parse().unwrap_or(o.max_ticks),
                "--prep-short" => o.prep_short = v.parse().unwrap_or(o.prep_short),
                "--prep-long" => o.prep_long = v.parse().unwrap_or(o.prep_long),
                "--scenario" => o.scenario = Some(v.clone()),
                _ => {}
            }
            i += 2;
        }
        o
    }
}

fn parse_seed(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn build_world(o: &Opts) -> World {
    build_world_with_relief(o, o.relief)
}

/// `battle`/`winrate` は「両軍が実際にぶつかって決着がつく」ことが前提なので、
/// 起伏を落として通行不能地形（崖・水域）が進路を塞がないようにする。
/// `bench`/`verify` は決着まで回さないので `o.relief` をそのまま使う。
fn build_world_with_relief(o: &Opts, relief: u16) -> World {
    let mut w = deploy_two_armies(o, relief);

    // 互いに向かって進ませ、中央でぶつかるようにする
    let mid = (o.size_m / 2) as i32;
    for i in 0..w.soldiers.len() {
        let target_y = if w.soldiers.faction[i] == 0 {
            fx(mid + 200)
        } else {
            fx(mid - 200)
        };
        w.set_goal(i as u32, Vec2Fx::new(w.soldiers.pos(i).x, target_y));
    }
    w
}

/// 地形を生成し、両軍を向かい合わせに配置するだけで、まだ動かさない。
/// `build_world_with_relief` はこの直後に個々の兵士へ前進の目標を与えるが、
/// `run_prep_battle` は指揮ツリーの陣形（`form_battle_units`）を組むまで
/// 兵士を `Idle` のまま留めておきたいので、こちらを直接使う。
fn deploy_two_armies(o: &Opts, relief: u16) -> World {
    // 地形生成は `web/src/terrain` にあり Rust からは呼べないので、
    // 性能計測と決定論検証は一様な傾斜地の上で行う。地形の起伏そのものを
    // 見たいときは `terrain --scenario ID` で固定地形を読むこと。
    // `relief` は傾きの強さ（セルあたり cm）に写す。
    let cell_m = 2;
    let dim = (o.size_m / cell_m).max(8);
    let rise_cm_per_cell = 1 + (relief as i32 / 250);
    let mut w = World::with_terrain(
        o.seed,
        Terrain::sloped(dim, cell_m, o.seed, rise_cm_per_cell),
    );
    eprintln!("地形: {} m 四方の傾斜地（relief {relief}）", o.size_m);

    // 両軍を向かい合わせに配置する。1 部隊 = 40 列 × N 列。
    let per_side = o.soldiers / 2;
    let files = 40u32;
    let ranks = per_side.div_ceil(files);
    let mid = (o.size_m / 2) as i32;

    deploy_block(
        &mut w,
        Vec2Fx::new(fx(mid - (files as i32 * 8) / 10 / 2), fx(mid - 200)),
        files,
        ranks,
        800,
        0,
        0,
        1,
    );
    deploy_block(
        &mut w,
        Vec2Fx::new(fx(mid - (files as i32 * 8) / 10 / 2), fx(mid + 200)),
        files,
        ranks,
        800,
        1,
        1,
        2,
    );
    w
}

/// `battle`/`winrate` 用に、両軍を指揮ツリーの陣形（`Unit`）として組む。
///
/// `deploy_block` + 個々の `set_goal` だけだと、各兵士が独立に敵陣の
/// 座標を目指すだけなので、衝突で押し合ううちに隊列が完全に崩れて
/// 混戦の塊になり、白兵戦の前列判定・弧判定を誰も満たせなくなって
/// 会戦が停止してしまう。指揮ツリーの陣形スロット（`formation_goals`）を
/// 使うと、前列が常に前列であり続け、倒れた前列は後列が詰めて補充する
/// ——実際の中世会戦の隊列維持に近い挙動になる。
fn build_battle_world(o: &Opts) -> World {
    let mut w = build_world_with_relief(o, 0);
    form_battle_units(&mut w, o);
    w
}

/// 両軍を指揮ツリーの陣形（`Unit`）として組む（`build_battle_world` の後半）。
/// これを呼ぶまでは兵士は `Idle` のままその場に留まるので、`prep_experiment`
/// はこの呼び出しを遅らせることで「会戦開始前の準備時間」（仕様 07 章 7 節）を
/// 表現する。
fn form_battle_units(w: &mut World, o: &Opts) {
    let per_side = o.soldiers / 2;
    let files = 40u32;
    let ranks = per_side.div_ceil(files);
    let mid = fx((o.size_m / 2) as i32);
    // `formation_facing` は「ランク（奥行き）方向」を回転させる角度で、
    // 兵士個体の見た目の向き（`soldiers.hot.facing`、`Vec2Fx::angle()`）とは
    // 90° ずれた別の規約になっている（`organization::formation_goals` の
    // 回転式は local_y=0 のとき facing=0 で world +Y に伸びる）。
    // ここは実測で確認済み：facing=0 → ランクは +Y（北）へ、
    // facing=180° → ランクは -Y（南）へ伸びる。
    let north = 0u16;
    let south = sim_math::BRAD_HALF as u16;

    // `formation_origin` はランク 0（進行方向の最後尾）の位置で、後続ランクは
    // そこから facing 方向へ `rank_spacing` ずつ前へ出る。両陣営の最前列が
    // ちょうど中央（mid, mid）で向き合うように、後方へ隊列の奥行き分だけ
    // ずらしておく。ここを揃えないと両陣営の重心が完全に重なり、
    // 12 人の近傍上限を超える過密が起きて隊列が崩壊する。
    let rank_spacing = fx_from_mm(800);
    let depth = rank_spacing * (ranks as i32 - 1).max(0);
    // 前列同士の目標座標を完全に一致させると、押し合いの逃げ場がなく
    // 兵士が密着しすぎて（前列判定の 0.8 m 圏内に自分の隣の兵士まで
    // 入ってしまい）どちらも前列と判定されなくなる。剣の間合い（0.9 m）
    // には収まるが密着はしない程度の隙間を残す。
    let contact_gap = fx_from_mm(500);
    let target0 = Vec2Fx::new(mid, mid - depth - contact_gap);
    let target1 = Vec2Fx::new(mid, mid + depth + contact_gap);

    for faction in 0u8..2 {
        // 準備フェーズ用の工兵（`flags::ENGINEER`）は戦列に組み込まない。
        // 非戦闘員なので、隊列に混ざって前列判定を受けさせたくない。
        let ids: Vec<u32> = (0..w.soldiers.len())
            .filter(|&i| {
                w.soldiers.faction[i] == faction && w.soldiers.hot.flags[i] & flags::ENGINEER == 0
            })
            .map(|i| i as u32)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let commander = ids[0];
        let deputies: Vec<u32> = ids.iter().skip(1).take(4).copied().collect();
        let target = if faction == 0 { target0 } else { target1 };
        let unit = sim_core::organization::Unit {
            soldiers: ids,
            troop_type: 0,
            formation: sim_core::organization::FORMATION_SHIELDWALL,
            formation_origin: target,
            formation_facing: if faction == 0 { north } else { south },
            ranks: ranks as u16,
            file_spacing: fx_from_mm(800),
            rank_spacing,
            banner: None,
            formation_change: None,
            path: Vec::new(),
            path_final: target,
            pursuit_leash: None,
        };
        w.add_command_node(None, 0, faction, commander, deputies, Some(unit));
    }
}

/// 各陣営の 4 人に 1 人へ弓を持たせる。両陣営に同じ規則（index % 4）で
/// 適用するので、装備構成は対称になる（`winrate` の前提）。
fn equip_symmetric_army(w: &mut World) {
    for i in 0..w.soldiers.len() {
        if i % 4 == 0 && w.soldiers.hot.flags[i] & flags::ENGINEER == 0 {
            w.combat.set_loadout(
                i as u32,
                sim_core::combat::Weapon::longbow(),
                sim_core::combat::Armor::cloth(),
            );
            w.combat.set_ammo(i as u32, 18);
        }
    }
}

fn alive_by_faction(w: &World) -> (u32, u32) {
    let mut a = 0u32;
    let mut b = 0u32;
    for i in 0..w.soldiers.len() {
        if !w.soldiers.is_alive(i) {
            continue;
        }
        if w.soldiers.faction[i] == 0 {
            a += 1;
        } else {
            b += 1;
        }
    }
    (a, b)
}

struct BattleReport {
    /// 勝った陣営（0/1）。両陣営とも全滅、または `max_ticks` に達した場合は `None`。
    winner: Option<u8>,
    ticks: u32,
    timed_out: bool,
    /// 決着時点の陣営 0 / 1 の生存数。
    alive: (u32, u32),
    stats: sim_core::combat::CombatStats,
    /// 徒歩の助走突撃・転倒の統計。
    charge: sim_core::charge::ChargeStats,
    /// 疲れた前列と後列の交代が起きた回数。
    rotations: u32,
}

fn run_battle(o: &Opts) -> BattleReport {
    let mut w = build_battle_world(o);
    equip_symmetric_army(&mut w);
    finish_battle(w, o)
}

/// 決着がつくか `max_ticks` に達するまで tick を回し、結果をまとめる。
fn finish_battle(mut w: World, o: &Opts) -> BattleReport {
    let mut timed_out = true;
    for _ in 0..o.max_ticks {
        w.tick();
        if w.combat.phase == BattlePhase::Complete {
            timed_out = false;
            break;
        }
    }
    let (alive_a, alive_b) = alive_by_faction(&w);
    let winner = match (alive_a > 0, alive_b > 0) {
        (true, false) => Some(0),
        (false, true) => Some(1),
        _ => None,
    };
    BattleReport {
        winner,
        ticks: w.tick,
        timed_out,
        alive: (alive_a, alive_b),
        stats: w.combat.stats,
        charge: w.charge.stats,
        rotations: w.command.rotations,
    }
}

/// 陣営 1（防御側）に工兵を配置し、`prep_ticks` の間だけ両軍が本隊を組む前に
/// 杭列・堀を築かせてから、通常どおり戦列を組ませて決着まで戦わせる
/// （仕様 07 章 7 節「会戦前フェーズ」、12 章 M6 の受け入れ条件）。
///
/// `form_battle_units` を呼ぶまで両軍の本隊は指揮ツリーに入らず `Idle` の
/// ままその場に留まるので、`prep_ticks` の間は工兵の作業だけが進む。
fn run_prep_battle(o: &Opts, prep_ticks: u32) -> BattleReport {
    let mut w = deploy_two_armies(o, 0);

    let mid = fx((o.size_m / 2) as i32);
    let engineer_attrs = Attrs::new(120, 120, 140, 150, 120, 120, 140, 150, 100, 140, 140, 140);
    let engineer_count = 12u32;
    let stakes_span = fx_from_mm(6_000);
    for k in 0..engineer_count {
        let x = mid - stakes_span / 2 + (stakes_span * k as i32) / (engineer_count.max(1) as i32);
        w.spawn(
            Vec2Fx::new(x, mid + fx_from_mm(3_000)),
            0,
            0,
            1,
            engineer_attrs,
            flags::ENGINEER,
        );
    }
    // 防御側の正面に杭列を、その後ろに堀を築かせる（仕様 07 章 2 節の表の
    // 組み合わせ例）。どちらも防御側（陣営 1）の所有。
    w.queue_build_structure(
        StructureKind::Stakes,
        Vec2Fx::new(mid - stakes_span / 2, mid),
        Vec2Fx::new(mid + stakes_span / 2, mid),
        1,
        10,
    );
    w.queue_build_structure(
        StructureKind::Ditch,
        Vec2Fx::new(mid - stakes_span / 2, mid + fx_from_mm(1_500)),
        Vec2Fx::new(mid + stakes_span / 2, mid + fx_from_mm(1_500)),
        1,
        8,
    );

    for _ in 0..prep_ticks {
        w.tick();
    }

    form_battle_units(&mut w, o);
    equip_symmetric_army(&mut w);
    finish_battle(w, o)
}

fn prep_experiment(o: &Opts) {
    let t0 = Instant::now();
    let mut wins_defender_short = 0u32;
    let mut wins_defender_long = 0u32;
    let mut decisive_short = 0u32;
    let mut decisive_long = 0u32;
    for run in 0..o.runs {
        let seed = (o.seed.wrapping_add(run as u64 * 0x9E37_79B9)) | 1;
        let run_opts = Opts {
            seed,
            ..clone_opts(o)
        };
        let short = run_prep_battle(&run_opts, o.prep_short);
        if let Some(winner) = short.winner {
            decisive_short += 1;
            if winner == 1 {
                wins_defender_short += 1;
            }
        }
        let long = run_prep_battle(&run_opts, o.prep_long);
        if let Some(winner) = long.winner {
            decisive_long += 1;
            if winner == 1 {
                wins_defender_long += 1;
            }
        }
    }
    let rate = |wins: u32, decisive: u32| {
        if decisive == 0 {
            0.0
        } else {
            wins as f64 * 100.0 / decisive as f64
        }
    };
    let rate_short = rate(wins_defender_short, decisive_short);
    let rate_long = rate(wins_defender_long, decisive_long);

    println!("{{");
    println!("  \"runs\": {},", o.runs);
    println!("  \"prep_short_ticks\": {},", o.prep_short);
    println!("  \"prep_long_ticks\": {},", o.prep_long);
    println!("  \"defender_win_rate_short_pct\": {rate_short:.1},");
    println!("  \"defender_win_rate_long_pct\": {rate_long:.1},");
    println!("  \"win_rate_delta_pct\": {:.1},", rate_long - rate_short);
    println!("  \"wall_clock_s\": {:.1}", t0.elapsed().as_secs_f64());
    println!("}}");

    eprintln!(
        "\n防御側の勝率: 準備なし {rate_short:.1}% → 十分な準備 {rate_long:.1}%（差 {:.1} pt）",
        rate_long - rate_short
    );
}

fn battle(o: &Opts) {
    let t0 = Instant::now();
    let report = run_battle(o);
    let elapsed_s = t0.elapsed().as_secs_f64();
    let game_minutes = report.ticks as f64 / sim_math::TICK_HZ as f64 / 60.0;
    let s = &report.stats;
    let total_deaths =
        s.melee_kills + s.missile_kills + s.crush_kills + s.bleed_kills + s.pursuit_kills;
    let pct = |v: u32| {
        if total_deaths == 0 {
            0.0
        } else {
            v as f64 * 100.0 / total_deaths as f64
        }
    };
    let largest = [
        ("追撃", s.pursuit_kills),
        ("白兵", s.melee_kills),
        ("射撃", s.missile_kills),
        ("圧迫", s.crush_kills),
        ("出血", s.bleed_kills),
    ]
    .into_iter()
    .max_by_key(|&(_, v)| v);

    println!("{{");
    println!(
        "  \"winner_faction\": {},",
        report.winner.map_or("null".to_string(), |w| w.to_string())
    );
    println!("  \"timed_out\": {},", report.timed_out);
    println!("  \"ticks\": {},", report.ticks);
    println!("  \"game_minutes\": {game_minutes:.1},");
    println!("  \"wall_clock_s\": {elapsed_s:.1},");
    println!("  \"total_deaths\": {total_deaths},");
    println!(
        "  \"pursuit_kills\": {} ({:.1}%),",
        s.pursuit_kills,
        pct(s.pursuit_kills)
    );
    println!(
        "  \"melee_kills\": {} ({:.1}%),",
        s.melee_kills,
        pct(s.melee_kills)
    );
    println!(
        "  \"missile_kills\": {} ({:.1}%),",
        s.missile_kills,
        pct(s.missile_kills)
    );
    println!(
        "  \"crush_kills\": {} ({:.1}%),",
        s.crush_kills,
        pct(s.crush_kills)
    );
    println!(
        "  \"bleed_kills\": {} ({:.1}%),",
        s.bleed_kills,
        pct(s.bleed_kills)
    );
    println!("  \"friendly_fire_hits\": {},", s.friendly_fire_hits);
    println!("  \"shots_fired\": {},", s.shots_fired);
    println!("  \"foot_charge_impacts\": {},", report.charge.impacts);
    println!("  \"stumbles\": {},", report.charge.stumbles);
    println!("  \"charge_falters\": {},", report.charge.falters);
    println!("  \"braces\": {},", report.charge.braces);
    println!("  \"rank_rotations\": {}", report.rotations);
    println!("}}");

    eprintln!(
        "\n所要 {game_minutes:.1} 分（20〜180 分の範囲内: {}）",
        (20.0..=180.0).contains(&game_minutes)
    );
    if let Some((name, _)) = largest {
        eprintln!(
            "最大の死因: {name}（追撃が最大: {}）",
            largest.map(|(n, _)| n) == Some("追撃")
        );
    }
}

/// 史実の会戦プリセット（`sim_core::scenario`）を決着まで回し、両軍の残存と
/// 死因内訳、指揮官が下した判断を出す。
///
/// ブラウザを開かずにプリセットの妥当性——狭隘・泥濘・杭列・指揮の分裂が
/// 効いているか——を見るための入口。
fn scenario_run(o: &Opts) {
    let Some(id) = o.scenario.as_deref() else {
        println!("登録されている会戦プリセット:");
        for def in sim_core::scenario::SCENARIOS {
            println!(
                "  {:<16} {}（{}年・{}）兵 {}",
                def.id,
                def.name_ja,
                def.year,
                def.place_ja,
                def.soldier_count()
            );
        }
        return;
    };
    let Some(index) = sim_core::scenario::index_of(id) else {
        eprintln!("そのようなプリセットはありません: {id}");
        std::process::exit(2);
    };
    let def = sim_core::scenario::get(index).unwrap();

    let t0 = Instant::now();
    let w = match sim_core::scenario::build_world_from_fixture(def) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "{}（{}年）: 兵 {} を配置（地形と配置に {:.1} ms）",
        def.name_ja,
        def.year,
        w.soldiers.len(),
        t0.elapsed().as_secs_f64() * 1000.0
    );

    let t1 = Instant::now();
    let report = finish_battle(w, o);
    let elapsed_s = t1.elapsed().as_secs_f64();
    let game_minutes = report.ticks as f64 / sim_math::TICK_HZ as f64 / 60.0;
    let s = &report.stats;

    println!("{{");
    println!("  \"scenario\": \"{}\",", def.id);
    println!(
        "  \"winner_faction\": {},",
        report.winner.map_or("null".to_string(), |f| f.to_string())
    );
    println!("  \"timed_out\": {},", report.timed_out);
    println!("  \"ticks\": {},", report.ticks);
    println!("  \"game_minutes\": {game_minutes:.1},");
    println!("  \"wall_clock_s\": {elapsed_s:.1},");
    for (faction, army) in def.armies.iter().enumerate() {
        let alive = if faction == 0 {
            report.alive.0
        } else {
            report.alive.1
        };
        println!(
            "  \"{}\": {{ \"配置\": {}, \"生存\": {alive} }},",
            army.name_ja,
            def.army_soldier_count(army)
        );
    }
    println!("  \"melee_kills\": {},", s.melee_kills);
    println!("  \"missile_kills\": {},", s.missile_kills);
    println!("  \"crush_kills\": {},", s.crush_kills);
    println!("  \"bleed_kills\": {},", s.bleed_kills);
    println!("  \"pursuit_kills\": {},", s.pursuit_kills);
    println!("  \"shots_fired\": {},", s.shots_fired);
    println!("  \"charge_kills\": {},", s.charge_kills);
    println!("  \"horse_refusals\": {},", s.horse_refusals);
    println!("  \"dismounts\": {},", s.dismounts);
    println!("  \"foot_charge_impacts\": {},", report.charge.impacts);
    println!("  \"foot_charge_runups\": {},", report.charge.runups);
    println!("  \"foot_charge_backoffs\": {},", report.charge.backoffs);
    println!("  \"stumbles\": {},", report.charge.stumbles);
    println!("  \"charge_falters\": {},", report.charge.falters);
    println!("  \"braces\": {},", report.charge.braces);
    println!("  \"rank_rotations\": {}", report.rotations);
    println!("}}");
}

fn winrate(o: &Opts) {
    let t0 = Instant::now();
    let mut wins_a = 0u32;
    let mut wins_b = 0u32;
    let mut draws_or_timeouts = 0u32;
    for run in 0..o.runs {
        let mut run_opts = Opts {
            seed: o.seed.wrapping_add(run as u64 * 0x9E37_79B9),
            ..clone_opts(o)
        };
        run_opts.seed |= 1;
        let report = run_battle(&run_opts);
        match report.winner {
            Some(0) => wins_a += 1,
            Some(1) => wins_b += 1,
            _ => draws_or_timeouts += 1,
        }
    }
    let decisive = wins_a + wins_b;
    let win_rate_a = if decisive == 0 {
        0.0
    } else {
        wins_a as f64 * 100.0 / decisive as f64
    };
    let within_band = (42.0..=58.0).contains(&win_rate_a);

    println!("{{");
    println!("  \"runs\": {},", o.runs);
    println!("  \"wins_faction_0\": {wins_a},");
    println!("  \"wins_faction_1\": {wins_b},");
    println!("  \"draws_or_timeouts\": {draws_or_timeouts},");
    println!("  \"win_rate_faction_0_pct\": {win_rate_a:.1},");
    println!("  \"within_50pm8pct\": {within_band},");
    println!("  \"wall_clock_s\": {:.1}", t0.elapsed().as_secs_f64());
    println!("}}");
}

fn clone_opts(o: &Opts) -> Opts {
    Opts {
        soldiers: o.soldiers,
        ticks: o.ticks,
        size_m: o.size_m,
        relief: o.relief,
        seed: o.seed,
        runs: o.runs,
        max_ticks: o.max_ticks,
        prep_short: o.prep_short,
        prep_long: o.prep_long,
        scenario: o.scenario.clone(),
    }
}

fn bench(o: &Opts) {
    let mut w = build_world(o);
    let n = w.soldiers.len();

    // ウォームアップ
    for _ in 0..20 {
        w.tick();
    }

    let t0 = Instant::now();
    let mut max_tick_ms = 0.0f64;
    for _ in 0..o.ticks {
        let t = Instant::now();
        w.tick();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms > max_tick_ms {
            max_tick_ms = ms;
        }
    }
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let per_tick = total_ms / o.ticks as f64;

    // 20 Hz なので 1 tick の実時間予算は 50 ms
    let realtime_headroom = 50.0 / per_tick;

    println!("{{");
    println!("  \"soldiers\": {n},");
    println!("  \"ticks\": {},", o.ticks);
    println!("  \"total_ms\": {total_ms:.1},");
    println!("  \"ms_per_tick\": {per_tick:.3},");
    println!("  \"max_tick_ms\": {max_tick_ms:.3},");
    println!("  \"realtime_multiple\": {realtime_headroom:.1},");
    println!("  \"state_hash\": \"0x{:016X}\"", w.state_hash());
    println!("}}");

    eprintln!(
        "\n{n} 体 / {:.3} ms per tick / 実時間の {realtime_headroom:.1}x まで回る",
        per_tick
    );
}

fn verify(o: &Opts) {
    let run = || {
        let mut w = build_world(o);
        let mut hashes = Vec::with_capacity(o.ticks as usize);
        for _ in 0..o.ticks {
            w.tick();
            hashes.push(w.state_hash());
        }
        hashes
    };

    let a = run();
    let b = run();

    if a == b {
        println!(
            "OK: {} ティックのハッシュ列が一致 (最終 0x{:016X})",
            o.ticks,
            a.last().copied().unwrap_or(0)
        );
    } else {
        let diverged = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(0);
        eprintln!("NG: tick {diverged} でハッシュが分岐した");
        eprintln!("  1 回目: 0x{:016X}", a[diverged]);
        eprintln!("  2 回目: 0x{:016X}", b[diverged]);
        std::process::exit(1);
    }
}

fn terrain_report(o: &Opts) {
    let Some(id) = o.scenario.as_deref() else {
        eprintln!(
            "地形の生成は web/src/terrain にあり、Rust からは呼べません。\n\
             固定地形の統計を見るには --scenario ID を指定してください:"
        );
        for def in sim_core::scenario::SCENARIOS {
            eprintln!("  {}", def.id);
        }
        std::process::exit(2);
    };
    let t0 = Instant::now();
    let t = match sim_terrain::fixture::load_scenario(id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let s = sim_terrain::stats(&t);
    let total = s.cells as f64;
    let impassable = s.cells as u32 - s.passable_cells;
    println!("シナリオ    {id}");
    println!("シード      0x{:016X}", t.seed);
    println!("サイズ      {} m ({}² セル)", t.size_m(), t.dim);
    println!("読み込み    {ms:.1} ms");
    println!(
        "標高        {:.1} m 〜 {:.1} m",
        s.min_height_cm as f64 / 100.0,
        s.max_height_cm as f64 / 100.0
    );
    println!(
        "通行不能    {} セル ({:.1}%)",
        impassable,
        impassable as f64 * 100.0 / total
    );
    println!(
        "水系        河川 {} セル / 湖 {} セル / 海 {} セル",
        s.river_cells, s.lake_cells, s.sea_cells
    );
    println!(
        "人工物      道路 {} セル / 橋 {} セル",
        s.road_cells, s.bridge_cells
    );
    println!("崖          {} セル", s.cliff_cells);
    println!("会戦地候補  {} 件", t.battle_sites.len());
    if let Some(best) = t.battle_sites.first() {
        println!(
            "  最上位候補 ({:>5}, {:>5}) score={} 通行可能={:.0}% 開放度={:.0}%",
            best.x_m,
            best.y_m,
            best.score,
            best.passable_permille as f64 / 10.0,
            best.openness_permille as f64 / 10.0
        );
    }
    let ground_names = [
        "土", "草地", "砂", "岩", "泥", "河床", "海底", "崖錐", "湿地",
    ];
    let veg_names = [
        "なし",
        "短草",
        "疎林",
        "林",
        "密林",
        "低木",
        "牧草地",
        "耕地",
    ];
    println!("地質の内訳:");
    for (i, &c) in s.ground_counts.iter().enumerate() {
        if c > 0 {
            println!(
                "  {:<8} {:>6.2}%",
                ground_names[i],
                c as f64 * 100.0 / total
            );
        }
    }
    println!("植生の内訳:");
    for (i, &c) in s.vegetation_counts.iter().enumerate() {
        if c > 0 {
            println!("  {:<8} {:>6.2}%", veg_names[i], c as f64 * 100.0 / total);
        }
    }
}
