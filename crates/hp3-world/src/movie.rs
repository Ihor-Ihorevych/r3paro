//! Movies (`Movie`, `MovieTexture`): Bink video is not decoded yet, so a movie "plays" for no time. Asking for
//! one is logged, nothing is shown, and the movie's `MovieEnded` event fires on the next tick so scripts that
//! wait for the end carry on.

use hp3_script::{ObjId, Value, Vm};

use crate::{fire, world};

/// Movies whose end is reported at the next tick.
#[derive(Default)]
pub struct Movies {
    ended: Vec<ObjId>,
}

/// Fires `MovieEnded` for movies asked to play since the last tick.
pub fn tick(vm: &mut Vm<'_>) {
    let ended = std::mem::take(&mut world(vm).movies.ended);
    for m in ended {
        if !vm.db.objects.get(m).deleted {
            fire(vm, m, "MovieEnded", &mut []);
        }
    }
}

pub fn register(vm: &mut Vm<'_>) {
    // Play(string MovieFilename, bool UseSound, bool LoopMovie)
    vm.register("Movie.Play", |vm, this, a| {
        let name = match a.first() {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let looping = a.get(2).is_some_and(Value::as_bool);
        vm.log.push(format!("movie: not played (no video decoder yet): {name}"));
        if let Some(m) = this
            && !looping
        {
            world(vm).movies.ended.push(m);
        }
        Ok(Value::Empty)
    });
    vm.register("Movie.IsPlaying", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Movie.IsPaused", |_vm, _this, _a| Ok(Value::Bool(false)));
    vm.register("Movie.GetWidth", |_vm, _this, _a| Ok(Value::Int(0)));
    vm.register("Movie.GetHeight", |_vm, _this, _a| Ok(Value::Int(0)));
    vm.register("Movie.Pause", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Movie.StopNow", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Movie.StopAtEnd", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("MovieTexture.InitializeMovie", |_vm, _this, _a| Ok(Value::Empty));
}
