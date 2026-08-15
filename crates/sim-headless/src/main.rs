//! ブラウザなしでシミュレーションを回す CLI。
//!
//! バランス調整・性能計測・決定論の回帰テストをネイティブの速度で行うため。
//! CI はこれを使って性能とハッシュの回帰を検出する（仕様 11 章 6 節）。
//!
//! ```text
//! sim-headless bench   --soldiers 20000 --ticks 2000
//! sim-headless verify  --ticks 5000
//! sim-headless terrain --size 2000 --relief 600
//! ```

use std::time::Instant;

use sim_core::{deploy_block, World, WorldConfig};
use sim_math::{fx, Vec2Fx};
use sim_terrain::{SeaEdge, TerrainParams};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let opts = Opts::parse(&args);

    match cmd {
        "bench" => bench(&opts),
        "verify" => verify(&opts),
        "terrain" => terrain_report(&opts),
        _ => {
            eprintln!(
                "使い方:\n  \
                 bench   [--soldiers N] [--ticks N] [--size M] [--seed N]   性能を計測する\n  \
                 verify  [--soldiers N] [--ticks N] [--seed N]              決定論を検証する\n  \
                 terrain [--size M] [--relief 0-1000] [--seed N]            地形の統計を出す\n          \
                 [--river-density 0-1000] [--roads N] [--sea north|east|south|west]"
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
    river_density: u16,
    road_count: u16,
    sea_edge: SeaEdge,
}

impl Opts {
    fn parse(args: &[String]) -> Opts {
        let mut o = Opts {
            soldiers: 5_000,
            ticks: 1_000,
            size_m: 2_000,
            relief: 450,
            seed: 0x5EED_1234_ABCD_0001,
            river_density: 500,
            road_count: 2,
            sea_edge: SeaEdge::None,
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
                "--river-density" => o.river_density = v.parse().unwrap_or(o.river_density),
                "--roads" => o.road_count = v.parse().unwrap_or(o.road_count),
                "--sea" => {
                    o.sea_edge = match v.as_str() {
                        "north" => SeaEdge::North,
                        "east" => SeaEdge::East,
                        "south" => SeaEdge::South,
                        "west" => SeaEdge::West,
                        _ => SeaEdge::None,
                    }
                }
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
    let config = WorldConfig {
        seed: o.seed,
        terrain: TerrainParams {
            seed: o.seed,
            size_m: o.size_m,
            relief: o.relief,
            ..Default::default()
        },
    };
    let t0 = Instant::now();
    let mut w = World::new(&config);
    let terrain_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("地形生成: {terrain_ms:.1} ms ({} m 四方)", o.size_m);

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

    // 互いに向かって進ませ、中央でぶつかるようにする
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
    let params = TerrainParams {
        seed: o.seed,
        size_m: o.size_m,
        relief: o.relief,
        river_density: o.river_density,
        road_count: o.road_count,
        sea_edge: o.sea_edge,
        ..Default::default()
    };
    let t0 = Instant::now();
    let t = sim_terrain::generate(&params);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let s = sim_terrain::stats(&t);
    println!("シード      0x{:016X}", o.seed);
    println!("サイズ      {} m ({}² セル)", t.size_m(), t.dim);
    println!("生成時間    {ms:.1} ms");
    println!(
        "標高        {:.1} m 〜 {:.1} m",
        s.min_height_cm as f64 / 100.0,
        s.max_height_cm as f64 / 100.0
    );
    println!(
        "通行不能    {} セル ({:.1}%)",
        s.impassable,
        s.impassable as f64 * 100.0 / t.surface.len() as f64
    );
    println!(
        "最大連結域  通行可能セルの {:.1}%",
        s.largest_passable_component as f64 * 100.0
            / (t.surface.len() - s.impassable as usize).max(1) as f64
    );
    println!(
        "水系        河川 {} セル / 湖 {} セル / 海 {} セル",
        s.river_cells, s.lake_cells, s.sea_cells
    );
    println!("崖          {} エッジ", s.cliff_edges);
    println!("会戦地候補  {} 件", s.battle_sites);
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
    println!("地表の内訳:");
    let total = t.surface.len() as f64;
    let names = [
        "深水",
        "浅瀬",
        "渡渉点",
        "湿地",
        "泥",
        "草地",
        "牧草地",
        "耕地",
        "低木",
        "疎林",
        "密林",
        "岩",
        "崖錐",
        "砂",
        "道路",
        "橋",
    ];
    for (i, &c) in s.surface_counts.iter().enumerate() {
        if c > 0 {
            println!("  {:<8} {:>6.2}%", names[i], c as f64 * 100.0 / total);
        }
    }

    match sim_terrain::validate(&t) {
        Ok(()) => println!("\n検証: OK"),
        Err(e) => {
            eprintln!("\n検証: NG — {e}");
            std::process::exit(1);
        }
    }
}
