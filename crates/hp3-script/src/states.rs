//! Object states: switching state, running state code, latent actions, and script events.
//!
//! Written from standard UnrealScript semantics (NOT yet verified against HP3's Core.dll).

use crate::class_db::StructId;
use crate::names::Name;
use crate::objects::ObjKind;
use crate::value::{ObjId, Value};
use crate::vm::{Frame, Latent, RunEnd, Vm, VmError, VmResult};

impl Vm<'_> {
    /// The state an object is in.
    pub fn state_of(&self, obj: ObjId) -> Option<StructId> {
        match &self.db.objects.get(obj).kind {
            ObjKind::Instance { state, .. } => *state,
            _ => None,
        }
    }

    /// Finds a state declared in a class or its ancestors.
    pub fn find_state(&self, class: StructId, name: Name) -> Option<StructId> {
        let mut c = Some(class);
        while let Some(cid) = c {
            if let Some(&s) = self.db.get(cid).states.get(&name) {
                return Some(s);
            }
            c = self.db.get(cid).super_;
        }
        None
    }

    /// The state marked `auto` in a class or its ancestors.
    pub fn auto_state(&self, class: StructId) -> Option<StructId> {
        const STATE_AUTO: u32 = 0x0000_0001;
        let mut c = Some(class);
        while let Some(cid) = c {
            for &s in self.db.get(cid).states.values() {
                if self.db.get(s).state_info.3 & STATE_AUTO != 0 {
                    return Some(s);
                }
            }
            c = self.db.get(cid).super_;
        }
        None
    }

    /// Calls a script event (a function found by name, state first) if the object has one.
    pub fn event(&mut self, obj: ObjId, name: &str, args: &mut [Value]) -> VmResult<Value> {
        let Some(n) = self.db.names.find(name) else { return Ok(Value::Empty) };
        match self.find_function(Some(obj), None, n) {
            Some(f) if !self.is_trivial(f) => self.invoke(f, Some(obj), args),
            _ => Ok(Value::Empty),
        }
    }

    /// Moves an object to a state (`Name::NONE` leaves all states) and starts its state code at `label`.
    pub fn goto_state(&mut self, obj: ObjId, state_name: Name, label: Name) -> VmResult<()> {
        let Some(class) = self.db.objects.get(obj).class else { return Ok(()) };
        // `Auto` names the class's auto state.
        let state_name = if self.db.names.get(state_name).eq_ignore_ascii_case("Auto") {
            match self.auto_state(class) {
                Some(s) => self.db.get(s).name,
                None => return Ok(()),
            }
        } else {
            state_name
        };
        let new_state = if state_name == Name::NONE { None } else { self.find_state(class, state_name) };
        if state_name != Name::NONE && new_state.is_none() {
            let s = format!("{}: no state {}", self.db.names.get(self.db.objects.get(obj).name), self.db.names.get(state_name));
            self.warn(s);
            return Ok(());
        }
        let old = self.state_of(obj);
        if self.trace_states {
            let who = self.db.names.get(self.db.objects.get(obj).name).to_string();
            let from = old.map(|s| self.db.names.get(self.db.get(s).name).to_string());
            let to = new_state.map(|s| self.db.names.get(self.db.get(s).name).to_string());
            let line = format!("state: {who} {from:?} -> {to:?}");
            self.log.push(line);
        }
        if old != new_state {
            if old.is_some() {
                self.event(obj, "EndState", &mut [])?;
            }
            if let ObjKind::Instance { state, .. } = &mut self.db.objects.get_mut(obj).kind {
                *state = new_state;
            }
            self.last_state.insert(obj, old);
        }
        self.state_changed.insert(obj);
        self.latent.remove(&obj);
        self.state_pc.remove(&obj);
        if let Some(st) = new_state {
            let code = self.code(st);
            let label = if label == Name::NONE { self.db.names.intern("Begin") } else { label };
            if let Some(&pc) = code.labels.get(&label) {
                self.state_pc.insert(obj, pc);
            }
            if old != new_state {
                self.event(obj, "BeginState", &mut [])?;
            }
        }
        Ok(())
    }

    /// Advances an object's state code by `dt` seconds: counts down sleeps and runs code until it waits.
    pub fn tick_state(&mut self, obj: ObjId, dt: f32) -> VmResult<()> {
        match self.latent.get_mut(&obj) {
            Some(Latent::Sleep(t)) => {
                *t -= dt;
                if *t > 0.0 {
                    return Ok(());
                }
                self.latent.remove(&obj);
            }
            Some(Latent::Other(_)) => return Ok(()),
            None => {}
        }
        let Some(st) = self.state_of(obj) else { return Ok(()) };
        let Some(pc) = self.state_pc.remove(&obj) else { return Ok(()) };
        let code = self.code(st);
        let locals = self.db.get(st).defaults.clone();
        let mut frame = Frame::new(st, Some(obj), locals);
        self.state_changed.remove(&obj);
        match self.run_state_code(&code, &mut frame, pc)? {
            RunEnd::Suspended(next) => {
                // A GotoState from inside the code already set where to continue.
                self.state_pc.entry(obj).or_insert(next);
            }
            RunEnd::Finished => {}
        }
        Ok(())
    }

    /// Clears a latent wait the world was handling (an animation finished, a move arrived).
    pub fn finish_latent(&mut self, obj: ObjId) {
        if matches!(self.latent.get(&obj), Some(Latent::Other(_))) {
            self.latent.remove(&obj);
        }
    }
}

fn name_arg(a: &[Value], i: usize) -> Name {
    a.get(i).map(Value::as_name).unwrap_or(Name::NONE)
}

pub fn register(vm: &mut Vm<'_>) {
    vm.register("Object.GotoState", |vm, this, a| {
        if let Some(o) = this {
            vm.goto_state(o, name_arg(a, 0), name_arg(a, 1))?;
        }
        Ok(Value::Empty)
    });
    vm.register("Object.IsInState", |vm, this, a| {
        let want = name_arg(a, 0);
        let mut s = this.and_then(|o| vm.state_of(o));
        while let Some(st) = s {
            if vm.db.get(st).name == want {
                return Ok(Value::Bool(true));
            }
            s = vm.db.get(st).super_;
        }
        Ok(Value::Bool(false))
    });
    vm.register("Object.GetStateName", |vm, this, _a| {
        Ok(Value::Name(this.and_then(|o| vm.state_of(o)).map(|s| vm.db.get(s).name).unwrap_or(Name::NONE)))
    });
    vm.register("Object.GetLastStateName", |vm, this, _a| {
        let last = this.and_then(|o| vm.last_state.get(&o).copied().flatten());
        Ok(Value::Name(last.map(|s| vm.db.get(s).name).unwrap_or(Name::NONE)))
    });
    // Probe masks only filter which events reach script; every event is delivered for now.
    vm.register("Object.Enable", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Object.Disable", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Actor.Sleep", |vm, this, a| {
        let Some(o) = this else { return Err(VmError::Runtime("Sleep without an object".into())) };
        vm.latent.insert(o, Latent::Sleep(a.first().map(Value::as_float).unwrap_or(0.0)));
        Ok(Value::Empty)
    });
}
