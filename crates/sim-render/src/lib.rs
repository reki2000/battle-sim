//! 状態保持型の人物モーションと描画ポリゴン生成。
//!
//! シミュレーションの位置・向き・状態を正本とし、ここではスナップショット間の
//! 補間、モーション位相、切替クロスフェード、体格差、騎乗姿勢、LOD、カリング、
//! 最終的な WebGL2 用三角形列だけを扱う。ルート移動は行わない。

use std::f32::consts::{PI, TAU};

pub const VERTEX_FLOATS: usize = 8;
pub const API_MAJOR: u32 = 1;
pub const API_MINOR: u32 = 0;

const FLAG_MOUNTED: u8 = 1 << 0;
const FLAG_STUMBLING: u8 = 1 << 6;
const FLAG_BRACED: u8 = 1 << 7;
const TRANSITION_SECONDS: f32 = 0.24;

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn mul(self, scalar: f32) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }

    fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    fn normalized(self) -> Self {
        let length = self.length();
        if length <= 1.0e-6 {
            Self::new(0.0, 0.0, 1.0)
        } else {
            self.mul(1.0 / length)
        }
    }

    fn lerp(self, other: Self, t: f32) -> Self {
        self.add(other.sub(self).mul(t))
    }
}

type Color = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Idle,
    Walk,
    Run,
    Combat,
    Shoot,
    Work,
    Brace,
    Fall,
    Dead,
}

impl Motion {
    fn cadence(self, speed_mps: f32) -> f32 {
        match self {
            Self::Idle => 0.65,
            Self::Walk => (speed_mps * 0.7).clamp(0.65, 1.7),
            Self::Run => (speed_mps * 0.38).clamp(1.2, 2.5),
            Self::Combat | Self::Shoot | Self::Work | Self::Brace => 1.25,
            Self::Fall => 0.8,
            Self::Dead => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Appearance {
    height: f32,
    build: f32,
    head: f32,
    arm: f32,
    leg: f32,
    skin: Color,
    hair: Color,
    cloth: Color,
    leather: Color,
}

impl Appearance {
    fn from_id(id: usize, faction: u8, troop_type: u16) -> Self {
        let mut seed = (id as u32).wrapping_add(1).wrapping_mul(0x9e37_79b1)
            ^ (u32::from(troop_type).wrapping_mul(0x85eb_ca6b));
        let mut sample = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xffff) as f32 / 65_535.0
        };
        let height = 0.93 + sample() * 0.14;
        let build = 0.86 + sample() * 0.28;
        let head = 0.92 + sample() * 0.16;
        let arm = 0.93 + sample() * 0.14;
        let leg = 0.93 + sample() * 0.14;
        let skin_tone = sample();
        let skin = mix_color([0.46, 0.25, 0.15, 1.0], [0.91, 0.70, 0.52, 1.0], skin_tone);
        let hair_tone = sample();
        let hair = mix_color(
            [0.08, 0.055, 0.035, 1.0],
            [0.35, 0.20, 0.08, 1.0],
            hair_tone,
        );
        let base = if faction & 1 == 0 {
            [0.18, 0.40, 0.67, 1.0]
        } else {
            [0.78, 0.34, 0.12, 1.0]
        };
        let role_tint = match troop_type % 4 {
            1 => [0.18, 0.18, 0.21, 1.0],
            2 => [0.25, 0.42, 0.18, 1.0],
            3 => [0.47, 0.38, 0.20, 1.0],
            _ => base,
        };
        let cloth = mix_color(base, role_tint, 0.18 + sample() * 0.22);
        Self {
            height,
            build,
            head,
            arm,
            leg,
            skin,
            hair,
            cloth,
            leather: [0.20 + sample() * 0.08, 0.12, 0.065, 1.0],
        }
    }
}

#[derive(Clone, Debug)]
struct Agent {
    previous: Vec3,
    target: Vec3,
    previous_facing: f32,
    target_facing: f32,
    state: u8,
    flags: u8,
    faction: u8,
    troop_type: u16,
    from_motion: Motion,
    motion: Motion,
    blend: f32,
    phase: f32,
    speed_mps: f32,
    appearance: Appearance,
}

impl Agent {
    fn new(id: usize, sample: SnapshotAgent) -> Self {
        let motion = motion_for(sample.state, sample.flags);
        Self {
            previous: sample.position,
            target: sample.position,
            previous_facing: sample.facing,
            target_facing: sample.facing,
            state: sample.state,
            flags: sample.flags,
            faction: sample.faction,
            troop_type: sample.troop_type,
            from_motion: motion,
            motion,
            blend: 1.0,
            phase: hash_phase(id),
            speed_mps: 0.0,
            appearance: Appearance::from_id(id, sample.faction, sample.troop_type),
        }
    }

    fn update(&mut self, id: usize, sample: SnapshotAgent, elapsed: f32) {
        self.previous = self.target;
        self.target = sample.position;
        self.previous_facing = self.target_facing;
        self.target_facing = sample.facing;
        self.speed_mps = if elapsed > 0.0 {
            let delta = self.target.sub(self.previous);
            (delta.x * delta.x + delta.y * delta.y).sqrt() / elapsed
        } else {
            0.0
        };
        let next_motion = motion_for(sample.state, sample.flags);
        if next_motion != self.motion {
            self.from_motion = self.motion;
            self.motion = next_motion;
            self.blend = 0.0;
        }
        if sample.faction != self.faction || sample.troop_type != self.troop_type {
            self.appearance = Appearance::from_id(id, sample.faction, sample.troop_type);
        }
        self.state = sample.state;
        self.flags = sample.flags;
        self.faction = sample.faction;
        self.troop_type = sample.troop_type;
    }

    fn root(&self, alpha: f32) -> Vec3 {
        self.previous.lerp(self.target, alpha.clamp(0.0, 1.0))
    }

    fn facing(&self, alpha: f32) -> f32 {
        let mut diff = self.target_facing - self.previous_facing;
        while diff > PI {
            diff -= TAU;
        }
        while diff < -PI {
            diff += TAU;
        }
        self.previous_facing + diff * alpha.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Camera {
    center_x: f32,
    center_y: f32,
    px_per_m: f32,
    width: f32,
    height: f32,
    world_size: f32,
}

#[derive(Clone, Copy)]
struct SnapshotAgent {
    position: Vec3,
    facing: f32,
    troop_type: u16,
    state: u8,
    flags: u8,
    faction: u8,
}

#[derive(Clone, Copy, Default)]
struct MotionPose {
    bob: f32,
    lean: f32,
    crouch: f32,
    left_leg: f32,
    right_leg: f32,
    left_arm: f32,
    right_arm: f32,
    arms_out: f32,
    fall: f32,
}

impl MotionPose {
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            bob: lerp(self.bob, other.bob, t),
            lean: lerp(self.lean, other.lean, t),
            crouch: lerp(self.crouch, other.crouch, t),
            left_leg: lerp(self.left_leg, other.left_leg, t),
            right_leg: lerp(self.right_leg, other.right_leg, t),
            left_arm: lerp(self.left_arm, other.left_arm, t),
            right_arm: lerp(self.right_arm, other.right_arm, t),
            arms_out: lerp(self.arms_out, other.arms_out, t),
            fall: lerp(self.fall, other.fall, t),
        }
    }
}

#[derive(Clone, Copy)]
struct Skeleton {
    pelvis: Vec3,
    chest: Vec3,
    neck: Vec3,
    head: Vec3,
    shoulder_l: Vec3,
    shoulder_r: Vec3,
    elbow_l: Vec3,
    elbow_r: Vec3,
    hand_l: Vec3,
    hand_r: Vec3,
    hip_l: Vec3,
    hip_r: Vec3,
    knee_l: Vec3,
    knee_r: Vec3,
    ankle_l: Vec3,
    ankle_r: Vec3,
    toe_l: Vec3,
    toe_r: Vec3,
}

/// Rust 内部で状態を保持する人物描画エンジン。
#[derive(Default)]
pub struct ArmyRenderer {
    agents: Vec<Agent>,
    camera: Camera,
    vertices: Vec<f32>,
    visible_indices: Vec<usize>,
    last_tick: Option<u32>,
    last_snapshot_hash: u64,
    visible_count: u32,
    drawn_count: u32,
    culled_count: u32,
}

impl ArmyRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 現行シミュレーションの一括スナップショットを取り込む。
    /// 同じ tick・同じ内容の再送は無視し、描画側でルート位置を積算しない。
    pub fn apply_snapshot(
        &mut self,
        bytes: &[u8],
        stride: usize,
        tick: u32,
    ) -> Result<(), &'static str> {
        if stride != 16 && stride != 20 {
            return Err("unsupported soldier snapshot stride");
        }
        if bytes.len() % stride != 0 {
            return Err("snapshot byte length is not a multiple of stride");
        }
        let snapshot_hash = hash_bytes(bytes);
        if self.last_tick == Some(tick) && self.last_snapshot_hash == snapshot_hash {
            return Ok(());
        }
        let elapsed = self
            .last_tick
            .map_or(0.05, |last| tick.wrapping_sub(last).max(1) as f32 * 0.05);
        let count = bytes.len() / stride;
        for id in 0..count {
            let sample = parse_snapshot_agent(bytes, stride, id)?;
            if let Some(agent) = self.agents.get_mut(id) {
                agent.update(id, sample, elapsed);
            } else {
                self.agents.push(Agent::new(id, sample));
            }
        }
        self.agents.truncate(count);
        self.last_tick = Some(tick);
        self.last_snapshot_hash = snapshot_hash;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_camera(
        &mut self,
        center_x: f32,
        center_y: f32,
        px_per_m: f32,
        width: f32,
        height: f32,
        world_size: f32,
    ) {
        self.camera = Camera {
            center_x,
            center_y,
            px_per_m: px_per_m.max(0.0001),
            width: width.max(1.0),
            height: height.max(1.0),
            world_size: world_size.max(1.0),
        };
    }

    /// モーションを進め、現在のカメラとLODに対応する三角形列を再生成する。
    pub fn step(&mut self, dt: f32, alpha: f32, lod: u8) {
        let dt = dt.clamp(0.0, 0.25);
        for agent in &mut self.agents {
            agent.phase = (agent.phase + dt * agent.motion.cadence(agent.speed_mps)) % 4096.0;
            if agent.blend < 1.0 {
                agent.blend = (agent.blend + dt / TRANSITION_SECONDS).min(1.0);
            }
        }

        self.visible_indices.clear();
        for (index, agent) in self.agents.iter().enumerate() {
            if self.is_visible(agent.root(alpha), agent.flags & FLAG_MOUNTED != 0) {
                self.visible_indices.push(index);
            }
        }
        self.visible_count = self.visible_indices.len() as u32;
        self.culled_count = self.agents.len().saturating_sub(self.visible_indices.len()) as u32;
        self.vertices.clear();

        let effective_lod = effective_lod(lod.min(4), self.visible_indices.len());
        let cap = lod_cap(effective_lod);
        let stride = self.visible_indices.len().div_ceil(cap).max(1);
        let indices: Vec<usize> = self
            .visible_indices
            .iter()
            .copied()
            .step_by(stride)
            .collect();
        for index in indices.iter().copied() {
            let agent = self.agents[index].clone();
            self.render_agent(&agent, alpha, effective_lod);
        }
        self.drawn_count = indices.len() as u32;
    }

    pub fn vertices(&self) -> &[f32] {
        &self.vertices
    }

    pub fn agent_count(&self) -> u32 {
        self.agents.len() as u32
    }

    pub fn visible_count(&self) -> u32 {
        self.visible_count
    }

    pub fn drawn_count(&self) -> u32 {
        self.drawn_count
    }

    pub fn culled_count(&self) -> u32 {
        self.culled_count
    }

    fn is_visible(&self, root: Vec3, mounted: bool) -> bool {
        let (sx, sy) = self.screen(root);
        let height = if mounted { 2.6 } else { 2.0 } * self.camera.px_per_m;
        let margin = height.max(12.0);
        sx >= -margin
            && sx <= self.camera.width + margin
            && sy >= -margin
            && sy <= self.camera.height + margin
    }

    fn render_agent(&mut self, agent: &Agent, alpha: f32, lod: u8) {
        let root = agent.root(alpha);
        let facing = agent.facing(alpha);
        let from = motion_pose(agent.from_motion, agent.phase, agent.flags);
        let to = motion_pose(agent.motion, agent.phase, agent.flags);
        let pose = from.lerp(to, smoothstep(agent.blend));
        let mounted = agent.flags & FLAG_MOUNTED != 0;
        let skeleton = build_skeleton(agent.appearance, pose, mounted);
        let transform = |p: Vec3| transform_local(root, facing, p);
        let state_color = state_color(agent.appearance.cloth, agent.state, agent.flags);

        match lod {
            0 => self.render_close(agent, skeleton, transform, state_color, mounted),
            1 => self.render_tactical(agent, skeleton, transform, state_color, mounted),
            2 => self.render_unit(agent, skeleton, transform, state_color, mounted),
            3 => self.render_battle(agent, skeleton, transform, state_color, mounted),
            _ => self.render_theater(agent, skeleton, transform, state_color, mounted),
        }
    }

    fn render_close<F: Fn(Vec3) -> Vec3>(
        &mut self,
        agent: &Agent,
        s: Skeleton,
        transform: F,
        cloth: Color,
        mounted: bool,
    ) {
        if mounted {
            self.render_horse(agent, &transform, 0);
        }
        let a = agent.appearance;
        let skin = state_fade(a.skin, agent.state);
        let leather = state_fade(a.leather, agent.state);
        let right = local_axis_right(agent.facing(1.0));
        let forward = local_axis_forward(agent.facing(1.0));
        let up = Vec3::new(0.0, 0.0, 1.0);

        self.add_oriented_box(
            transform(s.pelvis.lerp(s.chest, 0.25)),
            right,
            forward,
            up,
            Vec3::new(0.17 * a.build, 0.13 * a.build, 0.22 * a.height),
            darken(cloth, 0.78),
        );
        self.add_oriented_box(
            transform(s.pelvis.lerp(s.chest, 0.68)),
            right,
            forward,
            up,
            Vec3::new(0.25 * a.build, 0.14 * a.build, 0.24 * a.height),
            cloth,
        );
        self.add_segment(
            transform(s.shoulder_l),
            transform(s.shoulder_r),
            0.16,
            0.22,
            cloth,
        );
        self.add_segment(transform(s.chest), transform(s.neck), 0.11, 0.11, skin);

        let head_half = Vec3::new(0.115 * a.head, 0.105 * a.head, 0.14 * a.head);
        self.add_oriented_box(transform(s.head), right, forward, up, head_half, skin);
        // 顔の前方には張り出さず、頭頂・つむじ・後頭部の重なる3パーツで覆う。
        self.add_oriented_box(
            transform(s.head.add(Vec3::new(0.0, -0.025, 0.105 * a.head))),
            right,
            forward,
            up,
            Vec3::new(0.12 * a.head, 0.085 * a.head, 0.055 * a.head),
            a.hair,
        );
        self.add_oriented_box(
            transform(s.head.add(Vec3::new(0.0, -0.080 * a.head, 0.035 * a.head))),
            right,
            forward,
            up,
            Vec3::new(0.12 * a.head, 0.055 * a.head, 0.085 * a.head),
            a.hair,
        );
        self.add_oriented_box(
            transform(s.head.add(Vec3::new(0.0, -0.105 * a.head, -0.035 * a.head))),
            right,
            forward,
            up,
            Vec3::new(0.115 * a.head, 0.045 * a.head, 0.105 * a.head),
            a.hair,
        );

        let arm_width = 0.105 * a.build;
        self.add_segment(
            transform(s.shoulder_l),
            transform(s.elbow_l),
            arm_width,
            arm_width,
            cloth,
        );
        self.add_segment(
            transform(s.shoulder_r),
            transform(s.elbow_r),
            arm_width,
            arm_width,
            cloth,
        );
        self.add_segment(
            transform(s.elbow_l),
            transform(s.hand_l),
            arm_width * 0.85,
            arm_width * 0.85,
            leather,
        );
        self.add_segment(
            transform(s.elbow_r),
            transform(s.hand_r),
            arm_width * 0.85,
            arm_width * 0.85,
            leather,
        );
        self.add_segment(
            transform(s.hand_l),
            transform(s.hand_l.add(Vec3::new(0.0, 0.055, -0.025))),
            0.09 * a.build,
            0.07 * a.build,
            skin,
        );
        self.add_segment(
            transform(s.hand_r),
            transform(s.hand_r.add(Vec3::new(0.0, 0.055, -0.025))),
            0.09 * a.build,
            0.07 * a.build,
            skin,
        );

        let leg_width = 0.14 * a.build;
        self.add_segment(
            transform(s.hip_l),
            transform(s.knee_l),
            leg_width,
            leg_width,
            darken(cloth, 0.62),
        );
        self.add_segment(
            transform(s.hip_r),
            transform(s.knee_r),
            leg_width,
            leg_width,
            darken(cloth, 0.62),
        );
        self.add_segment(
            transform(s.knee_l),
            transform(s.ankle_l),
            leg_width * 0.82,
            leg_width * 0.82,
            leather,
        );
        self.add_segment(
            transform(s.knee_r),
            transform(s.ankle_r),
            leg_width * 0.82,
            leg_width * 0.82,
            leather,
        );
        self.add_segment(
            transform(s.ankle_l),
            transform(s.toe_l),
            leg_width,
            leg_width * 0.78,
            leather,
        );
        self.add_segment(
            transform(s.ankle_r),
            transform(s.toe_r),
            leg_width,
            leg_width * 0.78,
            leather,
        );
    }

    fn render_tactical<F: Fn(Vec3) -> Vec3>(
        &mut self,
        agent: &Agent,
        s: Skeleton,
        transform: F,
        cloth: Color,
        mounted: bool,
    ) {
        if mounted {
            self.render_horse(agent, &transform, 1);
        }
        let a = agent.appearance;
        self.add_segment(
            transform(s.pelvis),
            transform(s.neck),
            0.43 * a.build,
            0.27 * a.build,
            cloth,
        );
        self.add_segment(
            transform(s.neck),
            transform(s.head.add(Vec3::new(0.0, 0.0, 0.12))),
            0.23 * a.head,
            0.21 * a.head,
            a.skin,
        );
        self.add_segment(
            transform(s.shoulder_l),
            transform(s.hand_l),
            0.10 * a.build,
            0.10 * a.build,
            cloth,
        );
        self.add_segment(
            transform(s.shoulder_r),
            transform(s.hand_r),
            0.10 * a.build,
            0.10 * a.build,
            cloth,
        );
        self.add_segment(
            transform(s.hip_l),
            transform(s.ankle_l),
            0.13 * a.build,
            0.13 * a.build,
            darken(cloth, 0.65),
        );
        self.add_segment(
            transform(s.hip_r),
            transform(s.ankle_r),
            0.13 * a.build,
            0.13 * a.build,
            darken(cloth, 0.65),
        );
    }

    fn render_unit<F: Fn(Vec3) -> Vec3>(
        &mut self,
        agent: &Agent,
        s: Skeleton,
        transform: F,
        cloth: Color,
        mounted: bool,
    ) {
        let base = transform(if mounted {
            Vec3::new(0.0, 0.0, 0.15)
        } else {
            s.ankle_l.lerp(s.ankle_r, 0.5)
        });
        let top = transform(s.head.add(Vec3::new(0.0, 0.0, 0.16)));
        let width = if mounted {
            0.50
        } else {
            0.27 * agent.appearance.build
        };
        self.add_bipyramid(base, top, width, cloth);
    }

    fn render_battle<F: Fn(Vec3) -> Vec3>(
        &mut self,
        agent: &Agent,
        s: Skeleton,
        transform: F,
        cloth: Color,
        mounted: bool,
    ) {
        let fallen = agent.state >= 12 || agent.flags & FLAG_STUMBLING != 0;
        let height = if fallen {
            0.18
        } else if mounted {
            1.45
        } else {
            1.05
        };
        let base = transform(Vec3::new(0.0, 0.0, 0.03));
        let top = if fallen {
            transform(s.head)
        } else {
            base.add(Vec3::new(0.0, 0.0, height))
        };
        self.add_pyramid(base, top, if mounted { 0.42 } else { 0.24 }, cloth);
    }

    fn render_theater<F: Fn(Vec3) -> Vec3>(
        &mut self,
        agent: &Agent,
        _s: Skeleton,
        transform: F,
        cloth: Color,
        mounted: bool,
    ) {
        let root = transform(Vec3::new(0.0, 0.0, 0.02));
        let size = if mounted { 0.55 } else { 0.34 };
        let right = Vec3::new(size, -size, 0.0);
        self.add_triangle(
            root.sub(right),
            root.add(right),
            root.add(Vec3::new(0.0, 0.0, size * 2.2)),
            cloth,
        );
        let _ = agent;
    }

    fn render_horse<F: Fn(Vec3) -> Vec3>(&mut self, agent: &Agent, transform: &F, lod: u8) {
        let horse = state_fade([0.30, 0.18, 0.09, 1.0], agent.state);
        if lod == 0 {
            self.add_segment(
                transform(Vec3::new(0.0, -0.47, 0.92)),
                transform(Vec3::new(0.0, 0.47, 0.95)),
                0.48,
                0.52,
                horse,
            );
            self.add_segment(
                transform(Vec3::new(0.0, 0.40, 1.02)),
                transform(Vec3::new(0.0, 0.70, 1.36)),
                0.26,
                0.25,
                horse,
            );
            self.add_segment(
                transform(Vec3::new(0.0, 0.68, 1.36)),
                transform(Vec3::new(0.0, 0.88, 1.38)),
                0.25,
                0.22,
                horse,
            );
            let beat = (agent.phase * TAU).sin() * 0.10;
            for side in [-1.0_f32, 1.0] {
                for fore in [-1.0_f32, 1.0] {
                    let y = fore * 0.34;
                    let upper = Vec3::new(side * 0.18, y, 0.86);
                    let hoof = Vec3::new(side * 0.20, y + beat * fore, 0.06);
                    self.add_segment(
                        transform(upper),
                        transform(hoof),
                        0.095,
                        0.095,
                        darken(horse, 0.8),
                    );
                }
            }
        } else {
            self.add_segment(
                transform(Vec3::new(0.0, -0.48, 0.82)),
                transform(Vec3::new(0.0, 0.58, 1.00)),
                0.50,
                0.50,
                horse,
            );
            self.add_segment(
                transform(Vec3::new(0.0, 0.50, 1.0)),
                transform(Vec3::new(0.0, 0.82, 1.38)),
                0.25,
                0.24,
                horse,
            );
        }
    }

    fn add_segment(&mut self, a: Vec3, b: Vec3, width: f32, depth: f32, color: Color) {
        let axis = b.sub(a);
        if axis.length() <= 1.0e-5 {
            return;
        }
        let v = axis.normalized();
        let reference = if v.z.abs() > 0.88 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        };
        let x = v.cross(reference).normalized();
        let y = x.cross(v).normalized();
        self.add_oriented_box(
            a.lerp(b, 0.5),
            x,
            y,
            v,
            Vec3::new(width * 0.5, depth * 0.5, axis.length() * 0.52),
            color,
        );
    }

    fn add_oriented_box(
        &mut self,
        center: Vec3,
        x_axis: Vec3,
        y_axis: Vec3,
        z_axis: Vec3,
        half: Vec3,
        color: Color,
    ) {
        let x = x_axis.normalized().mul(half.x);
        let y = y_axis.normalized().mul(half.y);
        let z = z_axis.normalized().mul(half.z);
        let corners = [
            center.sub(x).sub(y).sub(z),
            center.add(x).sub(y).sub(z),
            center.add(x).add(y).sub(z),
            center.sub(x).add(y).sub(z),
            center.sub(x).sub(y).add(z),
            center.add(x).sub(y).add(z),
            center.add(x).add(y).add(z),
            center.sub(x).add(y).add(z),
        ];
        const FACES: [[usize; 4]; 6] = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        for [a, b, c, d] in FACES {
            self.add_triangle(corners[a], corners[b], corners[c], color);
            self.add_triangle(corners[a], corners[c], corners[d], color);
        }
    }

    fn add_bipyramid(&mut self, base: Vec3, top: Vec3, width: f32, color: Color) {
        let center = base.lerp(top, 0.52);
        let dx = Vec3::new(width, -width, 0.0);
        let dy = Vec3::new(width, width, 0.0);
        let ring = [
            center.add(dx),
            center.add(dy),
            center.sub(dx),
            center.sub(dy),
        ];
        for i in 0..4 {
            let next = (i + 1) & 3;
            self.add_triangle(base, ring[next], ring[i], color);
            self.add_triangle(top, ring[i], ring[next], color);
        }
    }

    fn add_pyramid(&mut self, base: Vec3, top: Vec3, width: f32, color: Color) {
        let x = Vec3::new(width, -width, 0.0);
        let y = Vec3::new(width, width, 0.0);
        let ring = [base.add(x), base.add(y), base.sub(x), base.sub(y)];
        for i in 0..4 {
            self.add_triangle(ring[i], ring[(i + 1) & 3], top, color);
        }
    }

    fn add_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Color) {
        let normal = b.sub(a).cross(c.sub(a)).normalized();
        let light = Vec3::new(-0.42, -0.58, 1.0).normalized();
        let shade = 0.48 + normal.dot(light).abs() * 0.52;
        let shaded = [
            color[0] * shade,
            color[1] * shade,
            color[2] * shade,
            color[3],
        ];
        self.push_vertex(a, shaded);
        self.push_vertex(b, shaded);
        self.push_vertex(c, shaded);
    }

    fn push_vertex(&mut self, p: Vec3, color: Color) {
        let (sx, sy) = self.screen(p);
        let x = sx / self.camera.width * 2.0 - 1.0;
        let y = 1.0 - sy / self.camera.height * 2.0;
        let depth_range = self.camera.world_size * 2.0 + 8.0;
        let depth = 1.0 - ((p.x + p.y + p.z * 0.7) / depth_range).clamp(0.0, 1.0);
        self.vertices
            .extend_from_slice(&[x, y, depth * 2.0 - 1.0, 1.0]);
        self.vertices.extend_from_slice(&color);
    }

    fn screen(&self, p: Vec3) -> (f32, f32) {
        let scale = self.camera.px_per_m;
        let sx = ((p.x - p.y) - (self.camera.center_x - self.camera.center_y)) * scale
            + self.camera.width * 0.5;
        let sy = ((p.x + p.y) - (self.camera.center_x + self.camera.center_y)) * scale * 0.5
            - p.z * scale * 1.5
            + self.camera.height * 0.5;
        (sx, sy)
    }
}

fn parse_snapshot_agent(
    bytes: &[u8],
    stride: usize,
    id: usize,
) -> Result<SnapshotAgent, &'static str> {
    let offset = id * stride;
    let x = f32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .map_err(|_| "invalid x")?,
    );
    let y = f32::from_le_bytes(
        bytes[offset + 4..offset + 8]
            .try_into()
            .map_err(|_| "invalid y")?,
    );
    let z_cm = i16::from_le_bytes(
        bytes[offset + 8..offset + 10]
            .try_into()
            .map_err(|_| "invalid z")?,
    );
    let facing = u16::from_le_bytes(
        bytes[offset + 10..offset + 12]
            .try_into()
            .map_err(|_| "invalid facing")?,
    );
    let (troop_type, state_offset, flags_offset, faction) = if stride == 20 {
        (
            u16::from_le_bytes(
                bytes[offset + 14..offset + 16]
                    .try_into()
                    .map_err(|_| "invalid troop type")?,
            ),
            offset + 16,
            offset + 17,
            bytes[offset + 18],
        )
    } else {
        let unit = u16::from_le_bytes(
            bytes[offset + 12..offset + 14]
                .try_into()
                .map_err(|_| "invalid unit")?,
        );
        (unit, offset + 14, offset + 15, (unit & 1) as u8)
    };
    if !x.is_finite() || !y.is_finite() {
        return Err("non-finite snapshot position");
    }
    Ok(SnapshotAgent {
        position: Vec3::new(x, y, f32::from(z_cm) / 100.0),
        facing: f32::from(facing) / 65_536.0 * TAU,
        troop_type,
        state: bytes[state_offset],
        flags: bytes[flags_offset],
        faction,
    })
}

fn motion_for(state: u8, flags: u8) -> Motion {
    if state >= 13 {
        Motion::Dead
    } else if state == 12 || flags & FLAG_STUMBLING != 0 {
        Motion::Fall
    } else if flags & FLAG_BRACED != 0 {
        Motion::Brace
    } else {
        match state {
            1..=3 | 8 | 10 => Motion::Walk,
            4 | 9 => Motion::Run,
            5 => Motion::Combat,
            6 | 7 => Motion::Shoot,
            11 => Motion::Work,
            _ => Motion::Idle,
        }
    }
}

fn motion_pose(motion: Motion, phase: f32, flags: u8) -> MotionPose {
    let cycle = (phase * TAU).sin();
    let double = (phase * TAU * 2.0).sin();
    let mut pose = match motion {
        Motion::Idle => MotionPose {
            bob: double * 0.008,
            left_arm: 0.04 * cycle,
            right_arm: -0.04 * cycle,
            ..MotionPose::default()
        },
        Motion::Walk => MotionPose {
            bob: double.abs() * 0.025,
            left_leg: cycle * 0.48,
            right_leg: -cycle * 0.48,
            left_arm: -cycle * 0.42,
            right_arm: cycle * 0.42,
            ..MotionPose::default()
        },
        Motion::Run => MotionPose {
            bob: double.abs() * 0.055,
            lean: 0.10,
            crouch: 0.05,
            left_leg: cycle * 0.78,
            right_leg: -cycle * 0.78,
            left_arm: -cycle * 0.72,
            right_arm: cycle * 0.72,
            ..MotionPose::default()
        },
        Motion::Combat => MotionPose {
            bob: double.abs() * 0.025,
            lean: 0.05,
            crouch: 0.08,
            left_leg: -0.18,
            right_leg: 0.22,
            left_arm: 0.85 + cycle * 0.25,
            right_arm: 1.0 - cycle * 0.42,
            arms_out: 0.10,
            ..MotionPose::default()
        },
        Motion::Shoot => MotionPose {
            lean: 0.035,
            crouch: 0.04,
            left_leg: -0.10,
            right_leg: 0.12,
            left_arm: 1.25,
            right_arm: 1.05,
            arms_out: 0.18,
            ..MotionPose::default()
        },
        Motion::Work => MotionPose {
            bob: double.abs() * 0.02,
            crouch: 0.10,
            left_arm: 0.65 + cycle * 0.35,
            right_arm: 0.65 - cycle * 0.35,
            ..MotionPose::default()
        },
        Motion::Brace => MotionPose {
            lean: 0.10,
            crouch: 0.16,
            left_leg: -0.22,
            right_leg: 0.28,
            left_arm: 1.05,
            right_arm: 1.05,
            arms_out: 0.15,
            ..MotionPose::default()
        },
        Motion::Fall => MotionPose {
            crouch: 0.12,
            fall: 0.82,
            left_arm: 0.35,
            right_arm: -0.20,
            ..MotionPose::default()
        },
        Motion::Dead => MotionPose {
            fall: 1.0,
            left_arm: 0.25,
            right_arm: -0.30,
            left_leg: 0.12,
            right_leg: -0.18,
            ..MotionPose::default()
        },
    };
    if flags & FLAG_MOUNTED != 0 {
        pose.bob *= 0.55;
        pose.crouch = pose.crouch.max(0.08);
    }
    pose
}

fn build_skeleton(a: Appearance, pose: MotionPose, mounted: bool) -> Skeleton {
    let h = a.height;
    let shoulder = 0.25 * a.build * h;
    let hip = 0.145 * a.build * h;
    let torso = 0.54 * h;
    let upper_arm = 0.31 * a.arm * h;
    let lower_arm = 0.29 * a.arm * h;
    let thigh = 0.43 * a.leg * h;
    let shin = 0.42 * a.leg * h;
    let pelvis_z = if mounted {
        1.34 * h
    } else {
        (thigh + shin + 0.08) * h.min(1.04)
    };
    let pelvis = Vec3::new(0.0, pose.lean * 0.18, pelvis_z + pose.bob - pose.crouch);
    let chest = pelvis.add(Vec3::new(0.0, pose.lean, torso * 0.72));
    let neck = pelvis.add(Vec3::new(0.0, pose.lean * 1.15, torso));
    let head = neck.add(Vec3::new(0.0, 0.015, 0.17 * a.head));
    let shoulder_l = chest.add(Vec3::new(-shoulder, 0.0, 0.08 * h));
    let shoulder_r = chest.add(Vec3::new(shoulder, 0.0, 0.08 * h));

    let arm_joint = |start: Vec3, angle: f32, side: f32| {
        let out = pose.arms_out * side;
        let elbow = start.add(Vec3::new(
            out,
            angle.sin() * upper_arm,
            -angle.cos() * upper_arm,
        ));
        let fore_angle = angle * 0.62 + if angle > 0.55 { 0.32 } else { 0.0 };
        let hand = elbow.add(Vec3::new(
            out * 0.25,
            fore_angle.sin() * lower_arm,
            -fore_angle.cos() * lower_arm,
        ));
        (elbow, hand)
    };
    let (elbow_l, hand_l) = arm_joint(shoulder_l, pose.left_arm, -1.0);
    let (elbow_r, hand_r) = arm_joint(shoulder_r, pose.right_arm, 1.0);

    let hip_l = pelvis.add(Vec3::new(-hip, 0.0, -0.06));
    let hip_r = pelvis.add(Vec3::new(hip, 0.0, -0.06));
    let leg_joint = |start: Vec3, angle: f32, side: f32| {
        if mounted {
            let knee = start.add(Vec3::new(
                side * 0.20,
                0.08 + angle.sin() * 0.08,
                -thigh * 0.58,
            ));
            let ankle = knee.add(Vec3::new(side * 0.08, -0.10, -shin * 0.62));
            let toe = ankle.add(Vec3::new(0.0, 0.18, -0.02));
            (knee, ankle, toe)
        } else {
            let knee = start.add(Vec3::new(0.0, angle.sin() * thigh, -angle.cos() * thigh));
            let lower = -angle * 0.22 - angle.abs() * 0.18;
            let ankle = knee.add(Vec3::new(0.0, lower.sin() * shin, -lower.cos() * shin));
            let toe = ankle.add(Vec3::new(0.0, 0.20 * h, -0.025));
            (knee, ankle, toe)
        }
    };
    let (knee_l, ankle_l, toe_l) = leg_joint(hip_l, pose.left_leg, -1.0);
    let (knee_r, ankle_r, toe_r) = leg_joint(hip_r, pose.right_leg, 1.0);

    let mut skeleton = Skeleton {
        pelvis,
        chest,
        neck,
        head,
        shoulder_l,
        shoulder_r,
        elbow_l,
        elbow_r,
        hand_l,
        hand_r,
        hip_l,
        hip_r,
        knee_l,
        knee_r,
        ankle_l,
        ankle_r,
        toe_l,
        toe_r,
    };
    if pose.fall > 0.0 {
        let apply = |p: Vec3| apply_fall(p, pose.fall);
        skeleton = Skeleton {
            pelvis: apply(skeleton.pelvis),
            chest: apply(skeleton.chest),
            neck: apply(skeleton.neck),
            head: apply(skeleton.head),
            shoulder_l: apply(skeleton.shoulder_l),
            shoulder_r: apply(skeleton.shoulder_r),
            elbow_l: apply(skeleton.elbow_l),
            elbow_r: apply(skeleton.elbow_r),
            hand_l: apply(skeleton.hand_l),
            hand_r: apply(skeleton.hand_r),
            hip_l: apply(skeleton.hip_l),
            hip_r: apply(skeleton.hip_r),
            knee_l: apply(skeleton.knee_l),
            knee_r: apply(skeleton.knee_r),
            ankle_l: apply(skeleton.ankle_l),
            ankle_r: apply(skeleton.ankle_r),
            toe_l: apply(skeleton.toe_l),
            toe_r: apply(skeleton.toe_r),
        };
    }
    skeleton
}

fn apply_fall(p: Vec3, fall: f32) -> Vec3 {
    let angle = fall.clamp(0.0, 1.0) * PI * 0.48;
    Vec3::new(p.x + p.z * angle.sin(), p.y, 0.07 + p.z * angle.cos())
}

fn transform_local(root: Vec3, facing: f32, p: Vec3) -> Vec3 {
    let forward = local_axis_forward(facing);
    let right = local_axis_right(facing);
    root.add(right.mul(p.x))
        .add(forward.mul(p.y))
        .add(Vec3::new(0.0, 0.0, p.z))
}

fn local_axis_forward(facing: f32) -> Vec3 {
    Vec3::new(facing.cos(), facing.sin(), 0.0)
}

fn local_axis_right(facing: f32) -> Vec3 {
    Vec3::new(-facing.sin(), facing.cos(), 0.0)
}

fn effective_lod(requested: u8, visible: usize) -> u8 {
    match requested {
        0 | 1 if visible > 3_000 => 2,
        0 if visible > 1_200 => 1,
        other => other,
    }
}

fn lod_cap(lod: u8) -> usize {
    match lod {
        0 => 1_200,
        1 => 3_000,
        2 => 15_000,
        3 => 12_000,
        _ => 5_000,
    }
}

fn state_color(base: Color, state: u8, flags: u8) -> Color {
    let mut color = if state == 8 || state == 9 {
        mix_color(base, [0.92, 0.45, 0.12, 1.0], 0.30)
    } else if state == 10 {
        mix_color(base, [0.28, 0.75, 0.42, 1.0], 0.24)
    } else {
        base
    };
    if flags & FLAG_BRACED != 0 {
        color = mix_color(color, [0.65, 0.78, 0.96, 1.0], 0.20);
    }
    state_fade(color, state)
}

fn state_fade(mut color: Color, state: u8) -> Color {
    if state >= 12 {
        color[0] *= 0.46;
        color[1] *= 0.46;
        color[2] *= 0.46;
        color[3] = 0.82;
    }
    color
}

fn mix_color(a: Color, b: Color, t: f32) -> Color {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        lerp(a[3], b[3], t),
    ]
}

fn darken(mut color: Color, amount: f32) -> Color {
    color[0] *= amount;
    color[1] *= amount;
    color[2] *= amount;
    color
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash_phase(id: usize) -> f32 {
    let mixed = (id as u32).wrapping_add(1).wrapping_mul(0x9e37_79b1);
    (mixed & 0xffff) as f32 / 65_535.0
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(x: f32, y: f32, state: u8, flags: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; 20];
        bytes[0..4].copy_from_slice(&x.to_le_bytes());
        bytes[4..8].copy_from_slice(&y.to_le_bytes());
        bytes[8..10].copy_from_slice(&0_i16.to_le_bytes());
        bytes[10..12].copy_from_slice(&0_u16.to_le_bytes());
        bytes[12..14].copy_from_slice(&0_u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&1_u16.to_le_bytes());
        bytes[16] = state;
        bytes[17] = flags;
        bytes[18] = 0;
        bytes
    }

    #[test]
    fn snapshot_is_authoritative_and_step_does_not_move_root() {
        let mut renderer = ArmyRenderer::new();
        renderer
            .apply_snapshot(&snapshot(10.0, 20.0, 4, 0), 20, 1)
            .unwrap();
        renderer.set_camera(10.0, 20.0, 64.0, 800.0, 600.0, 100.0);
        let before = renderer.agents[0].target;
        for _ in 0..60 {
            renderer.step(1.0 / 60.0, 1.0, 0);
        }
        let after = renderer.agents[0].target;
        assert_eq!(before.x, after.x);
        assert_eq!(before.y, after.y);
        assert!(renderer.vertices().iter().all(|v| v.is_finite()));
    }

    #[test]
    fn identical_duplicate_tick_is_ignored_but_changed_snapshot_is_applied() {
        let mut renderer = ArmyRenderer::new();
        renderer
            .apply_snapshot(&snapshot(1.0, 2.0, 0, 0), 20, 7)
            .unwrap();
        let previous = renderer.agents[0].previous;
        renderer
            .apply_snapshot(&snapshot(1.0, 2.0, 0, 0), 20, 7)
            .unwrap();
        assert_eq!(renderer.agents[0].previous.x, previous.x);
        renderer
            .apply_snapshot(&snapshot(9.0, 9.0, 0, 0), 20, 7)
            .unwrap();
        assert_eq!(renderer.agents[0].target.x, 9.0);
    }

    #[test]
    fn motion_change_crossfades_and_keeps_arms_down_at_idle() {
        let mut renderer = ArmyRenderer::new();
        renderer
            .apply_snapshot(&snapshot(0.0, 0.0, 0, 0), 20, 1)
            .unwrap();
        renderer
            .apply_snapshot(&snapshot(0.1, 0.0, 4, 0), 20, 2)
            .unwrap();
        assert_eq!(renderer.agents[0].from_motion, Motion::Idle);
        assert_eq!(renderer.agents[0].motion, Motion::Run);
        assert_eq!(renderer.agents[0].blend, 0.0);
        renderer.step(TRANSITION_SECONDS, 1.0, 0);
        assert_eq!(renderer.agents[0].blend, 1.0);
        let idle = build_skeleton(
            renderer.agents[0].appearance,
            motion_pose(Motion::Idle, 0.0, 0),
            false,
        );
        assert!(idle.hand_l.z < idle.shoulder_l.z);
        assert!(idle.hand_r.z < idle.shoulder_r.z);
    }

    #[test]
    fn camera_culls_before_polygon_generation() {
        let mut bytes = snapshot(0.0, 0.0, 0, 0);
        bytes.extend(snapshot(1_000.0, 1_000.0, 0, 0));
        let mut renderer = ArmyRenderer::new();
        renderer.apply_snapshot(&bytes, 20, 1).unwrap();
        renderer.set_camera(0.0, 0.0, 32.0, 800.0, 600.0, 2_000.0);
        renderer.step(0.016, 1.0, 1);
        assert_eq!(renderer.visible_count(), 1);
        assert_eq!(renderer.culled_count(), 1);
        assert!(renderer.drawn_count() >= 1);
    }

    #[test]
    fn all_lods_emit_finite_triangles_without_sprites() {
        let mut renderer = ArmyRenderer::new();
        renderer
            .apply_snapshot(&snapshot(0.0, 0.0, 5, FLAG_MOUNTED), 20, 1)
            .unwrap();
        renderer.set_camera(0.0, 0.0, 64.0, 800.0, 600.0, 100.0);
        for lod in 0..=4 {
            renderer.step(0.016, 1.0, lod);
            assert!(!renderer.vertices().is_empty(), "lod {lod}");
            assert_eq!(renderer.vertices().len() % (VERTEX_FLOATS * 3), 0);
            assert!(renderer.vertices().iter().all(|v| v.is_finite()));
        }
    }
}
