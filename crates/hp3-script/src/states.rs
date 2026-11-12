//! Obje&& is_cutscene(vm, o)t states: switching state, running state code, latent actions, and script events.
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

    /// Where `label` is in the code of `state` or of the state it inherits (the one of the same name in a parent
    /// class, or the one it extends): the state that holds the code and the statement.
    pub fn find_label(&mut self, state: StructId, label: Name) -> Option<(StructId, usize)> {
        let mut s = Some(state);
        for _ in 0..32 {
            let Some(cur) = s else { break };
            if let Some(&pc) = self.code(cur).labels.get(&label) {
                return Some((cur, pc));
            }
            s = self.db.get(cur).super_;
        }
        None
    }

    /// The state marked `auto` in a class or its ancestors.
    pub fn auto_state(&self, class: StructId) -> Option<StructId> {
        // STATE_Editable is 0x1; auto states carry 0x2.
        const STATE_AUTO: u32 = 0x0000_0002;
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
        // Interned lookup: no lowercasing allocation for names already seen (events fire every tick).
        let n = self.db.names.intern(name);
        if !self.disabled_events.is_empty() && self.disabled_events.get(&obj).is_some_and(|off| off.contains(&n)) {
            return Ok(Value::Empty);
        }
        match self.find_function(Some(obj), None, n) {
            Some(f) if !self.is_trivial(f) => self.invoke(f, Some(obj), args),
            _ => Ok(Value::Empty),
        }
    }

    /// Moves an object to a state (`Name::NONE` leaves all states) and starts its state code at `label`.
    pub fn goto_state(&mut self, obj: ObjId, state_name: Name, label: Name) -> VmResult<()> {
        let Some(class) = self.db.objects.get(obj).class else { return Ok(()) };
        // Any GotoState (even to no state, even to the same one) starts again with every event switched on: a
        // Disable made before the initial state is entered is gone with it, as in the original.
        self.disabled_events.remove(&obj);
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
            // A state the class does not have means no state, as in the original's `UObject::GotoState` ("If state is NAME_None or was
            // not found, goes to no state"): the object leaves the state it was in. (PopupTrigger ends its popup with
            // GotoState('StateIdle') and has no such state; staying in the popup made its Tick run forever.)
            let s = format!("{}: no state {}", self.db.names.get(self.db.objects.get(obj).name), self.db.names.get(state_name));
            self.warn(s);
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
            // The old state hears EndState once: a GotoState made from inside it does not send it again, and when
            // EndState itself moved the object to another state, that move stands (this one is preempted), as in
            // the original (COMC's countdown goes from FadeIn to DoCountdown in FadeIn's EndState).
            if old.is_some() && !self.in_end_state.contains(&obj) {
                let sets = self.state_sets.get(&obj).copied().unwrap_or(0);
                self.in_end_state.insert(obj);
                let r = self.event(obj, "EndState", &mut []);
                self.in_end_state.remove(&obj);
                r?;
                if self.state_sets.get(&obj).copied().unwrap_or(0) != sets {
                    return Ok(());
                }
            }
            if let ObjKind::Instance { state, .. } = &mut self.db.objects.get_mut(obj).kind {
                *state = new_state;
            }
            *self.state_sets.entry(obj).or_insert(0) += 1;
            self.last_state.insert(obj, old);
        }
        self.state_changed.insert(obj);
        self.latent.remove(&obj);
        self.state_pc.remove(&obj);
        if let Some(st) = new_state {
            let code = self.code(st);
            let label = if label == Name::NONE { self.db.names.intern("Begin") } else { label };
            let _ = &code;
            if let Some(at) = self.find_label(st, label) {
                self.state_pc.insert(obj, at);
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
        if self.state_of(obj).is_none() {
            return Ok(());
        }
        let Some((owner, pc)) = self.state_pc.remove(&obj) else { return Ok(()) };
        let code = self.code(owner);
        let locals = self.db.get(owner).defaults.clone();
        let mut frame = Frame::new(owner, Some(obj), locals);
        self.state_changed.remove(&obj);
        match self.run_state_code(&code, &mut frame, pc)? {
            RunEnd::Suspended(next) => {
                // A GotoState from inside the code already set where to continue.
                self.state_pc.entry(obj).or_insert((owner, next));
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
            // A cutscene that starts takes hold of its pawns at once: an event starts it (a boss's defeat) and the pawns it needs
            // (that boss, which destroys itself in its own state code the same frame) must not get ahead of it.
            if vm.db.names.get(name_arg(a, 0)).eq_ignore_ascii_case("Running") && is_cutscene(vm, o) {
                vm.tick_state(o, 0.0)?;
            }
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
    // Enable / Disable switch an event of the object on and off (the probe mask of its state frame).
    vm.register("Object.Enable", |vm, this, a| {
        if let (Some(o), Some(n)) = (this, a.first().map(Value::as_name))
            && let Some(off) = vm.disabled_events.get_mut(&o)
        {
            off.retain(|&x| x != n);
        }
        Ok(Value::Empty)
    });
    vm.register("Object.Disable", |vm, this, a| {
        if let (Some(o), Some(n)) = (this, a.first().map(Value::as_name)) {
            let off = vm.disabled_events.entry(o).or_default();
            if !off.contains(&n) {
                off.push(n);
            }
        }
        Ok(Value::Empty)
    });
    vm.register("Actor.Sleep", |vm, this, a| {
        let Some(o) = this else { return Err(VmError::Runtime("Sleep without an object".into())) };
        vm.latent.insert(o, Latent::Sleep(a.first().map(Value::as_float).unwrap_or(0.0)));
        Ok(Value::Empty)
    });
}

/// Whether the object is a cutscene (a `KWCutScene` or a class made from it).
fn is_cutscene(vm: &Vm<'_>, o: ObjId) -> bool {
    let mut class = vm.db.objects.get(o).class;
    while let Some(c) = class {
        if vm.db.names.get(vm.db.get(c).name).eq_ignore_ascii_case("KWCutScene") {
            return true;
        }
        class = vm.db.get(c).super_;
    }
    false
}
