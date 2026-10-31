//! `logs/game.log` in the game folder: what the scripts logged, and once a second what the player is controlling
//! (pawn, its state, physics, place, turn and speed), so what happened in a play session can be read afterwards.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use hp3_script::Vm;
use hp3_world::physics::vec;

pub struct GameLog {
    file: Option<std::io::BufWriter<std::fs::File>>,
    started: Instant,
    last_second: u64,
    /// How many lines of the VM's log have been written.
    written: usize,
    last_pawn: String,
    cursor: Option<hp3_script::ObjId>,
    last_cursor: String,
    last_emitter_log: u128,
    last_pose_log: u128,
}

impl GameLog {
    pub fn new(root: &Path) -> GameLog {
        let file = std::fs::create_dir_all(root.join("logs"))
            .ok()
            .and_then(|_| std::fs::File::create(root.join("logs").join("game.log")).ok())
            .map(std::io::BufWriter::new);
        GameLog { file, started: Instant::now(), last_second: 0, written: 0, last_pawn: String::new(), cursor: None, last_cursor: String::new(), last_emitter_log: 0, last_pose_log: 0 }
    }

    fn line(&mut self, text: &str) {
        let t = self.started.elapsed().as_secs_f32();
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "[{t:8.2}] {text}");
        }
    }

    /// Called every frame; writes new script log lines at once and the player's state once a second.
    pub fn update(&mut self, vm: &mut Vm<'_>) {
        if self.file.is_none() {
            return;
        }
        if vm.log.len() < self.written {
            self.written = 0;
        }
        let fresh: Vec<String> = vm.log[self.written..].to_vec();
        self.written = vm.log.len();
        for l in fresh {
            self.line(&l);
        }
        self.log_cursor(vm);
        let second = self.started.elapsed().as_secs();
        let Some(pc) = hp3_world::world(vm).player else { return };
        let pawn = hp3_world::get(vm, pc, "Pawn").as_obj();
        let describe = |vm: &mut Vm<'_>| -> String {
            let Some(p) = pawn else { return "no pawn".to_string() };
            let path = hp3_script::natives::object_path(vm, p);
            let state = vm.state_of(p).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
            let phys = hp3_world::get(vm, p, "Physics").as_byte();
            let at = vec(&hp3_world::get(vm, p, "Location"));
            let rot: Vec<i32> = hp3_world::get(vm, p, "Rotation").fields().iter().map(hp3_script::Value::as_int).collect();
            let vel = vec(&hp3_world::get(vm, p, "Velocity"));
            let accel = vec(&hp3_world::get(vm, p, "Acceleration"));
            let ctl = hp3_world::get(vm, p, "Controller").as_obj().map(|c| hp3_script::natives::object_path(vm, c)).unwrap_or_default();
            let set = hp3_world::anim::set_of(vm, p);
            let chans = hp3_world::world(vm).anim.actors.get(&p).map(|x| x.channels.clone()).unwrap_or_default();
            let channels: Vec<String> = chans
                .iter()
                .enumerate()
                .filter(|(_, c)| c.sequence.is_some() && c.alpha > 0.001)
                .map(|(i, c)| {
                    let n = c.sequence.and_then(|s| set.as_ref().map(|set| set.sequences[s].name.clone())).unwrap_or_default();
                    format!("ch{i} {n} a{:.2} f{:.1}", c.alpha, c.frame)
                })
                .collect();
            format!("{path} controller {ctl} state {state} phys {phys} at {at} rot {rot:?} vel {vel} accel {accel} anims [{}]", channels.join(" | "))
        };
        // While hanging from a Carpe Retractum line: the pose every 0.15 s, to see how the body leans.
        if let Some(p) = pawn
            && vm.state_of(p).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).is_some_and(|n| n.contains("Retractum"))
            && self.started.elapsed().as_millis() / 150 != self.last_pose_log
        {
            self.last_pose_log = self.started.elapsed().as_millis() / 150;
            let rot: Vec<i32> = hp3_world::get(vm, p, "Rotation").fields().iter().map(hp3_script::Value::as_int).collect();
            let phys = hp3_world::get(vm, p, "Physics").as_byte();
            let roll_to = hp3_world::get(vm, p, "bRollToDesired").as_bool();
            let pose = hp3_world::anim::pose_debug(vm, p);
            let set = hp3_world::anim::set_of(vm, p);
            let chans = hp3_world::world(vm).anim.actors.get(&p).map(|x| x.channels.clone()).unwrap_or_default();
            let anims: Vec<String> = chans
                .iter()
                .enumerate()
                .filter(|(_, c)| c.sequence.is_some() && c.alpha > 0.001)
                .map(|(i, c)| {
                    let n = c.sequence.and_then(|s| set.as_ref().map(|set| set.sequences[s].name.clone())).unwrap_or_default();
                    format!("ch{i} {n} a{:.2} f{:.1}", c.alpha, c.frame)
                })
                .collect();
            self.line(&format!("HANG rot {rot:?} phys {phys} rollToDesired {roll_to} anims [{}] bones{pose}", anims.join(" | ")));
        }
        // A pawn that pushes and does not move: what is around it, once a second.
        if let Some(p) = pawn
            && second != self.last_second
        {
            let accel = hp3_world::physics::vec(&hp3_world::get(vm, p, "Acceleration"));
            let vel = hp3_world::physics::vec(&hp3_world::get(vm, p, "Velocity"));
            let phys = hp3_world::get(vm, p, "Physics").as_byte();
            if accel.length() > 100.0 && vel.truncate().length() < 20.0 && (phys == 1 || phys == 2) {
                let d = hp3_world::physics::debug_blocked(vm, p);
                self.line(&format!("BLOCKED phys {phys} {d}"));
            }
        }
        // A change of the controlled pawn is written at once, the rest once a second.
        let now = pawn.map(|p| hp3_script::natives::object_path(vm, p)).unwrap_or_default();
        if now != self.last_pawn {
            self.last_pawn = now;
            let d = describe(vm);
            self.line(&format!("PLAYER now controls: {d}"));
        } else if second != self.last_second {
            let d = describe(vm);
            self.line(&format!("PLAYER {d}"));
        }
        if second != self.last_second {
            self.last_second = second;
            if let Some(f) = &mut self.file {
                let _ = f.flush();
            }
        }
    }

    /// The aim cursor's state and the fire button, written whenever either changes (to see how long an aim lasts).
    fn log_cursor(&mut self, vm: &mut Vm<'_>) {
        if self.cursor.is_none_or(|c| vm.db.objects.get(c).deleted) {
            let Some(class) = vm.db.find("SpellCursor") else { return };
            self.cursor = hp3_world::world(vm).actors.clone().into_iter().find(|&a| !vm.db.objects.get(a).deleted && vm.db.objects.get(a).class == Some(class));
        }
        let (Some(c), Some(pc)) = (self.cursor, hp3_world::world(vm).player) else { return };
        let state = vm.state_of(c).map(|s| vm.db.names.get(vm.db.get(s).name).to_string()).unwrap_or_default();
        let fire = hp3_world::get(vm, pc, "bFire").as_int();
        let particles = hp3_world::get(vm, c, "CursorParticles").as_obj().map(|o| hp3_script::natives::object_path(vm, o)).unwrap_or_default();
        let target = hp3_world::get(vm, c, "aCurrentTarget").as_obj().map(|o| hp3_script::natives::object_path(vm, o)).unwrap_or_default();
        let now = format!("{state} bFire {fire} particles {particles} target {target}");
        // While the cursor has its aim particles, once a second: where they are and how they are doing.
        if let Some(e) = hp3_world::get(vm, c, "CursorParticles").as_obj().filter(|&e| !vm.db.objects.get(e).deleted)
            && self.started.elapsed().as_millis() / 500 != self.last_emitter_log
        {
            self.last_emitter_log = self.started.elapsed().as_millis() / 500;
            let d = hp3_world::particles::debug_emitter(vm, e);
            let cam = hp3_world::get(vm, c, "Location");
            self.line(&format!("CURSOREMITTER {d} | cursor at {:?}", vec(&cam)));
        }
        if now != self.last_cursor {
            self.line(&format!("CURSOR {now}"));
            self.last_cursor = now;
        }
    }
}
