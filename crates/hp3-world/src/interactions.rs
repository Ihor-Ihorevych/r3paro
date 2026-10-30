//! The local player's viewport objects and its interactions: the `Player`, the `InteractionMaster`, the
//! console and the GUI controller (menus), which get ticks, rendering and keys before the game does.
//!
//! The setup follows the script side (`InteractionMaster.AddInteraction`); what the engine itself does around
//! it (creating the objects, the order of the calls) is NOT yet verified against HP3's Engine.dll.

use hp3_script::{ObjId, Value, Vm};

use crate::{fire, get, set, world};

#[derive(Default, Clone, Copy)]
pub struct Viewport {
    pub player: Option<ObjId>,
    pub master: Option<ObjId>,
    pub console: Option<ObjId>,
    pub gui: Option<ObjId>,
}

fn new_named(vm: &mut Vm<'_>, class: &str) -> Option<ObjId> {
    let c = vm.db.find(class)?;
    let n = vm.db.names.intern(class);
    Some(vm.new_object(c, n))
}

/// Creates the player's viewport objects for `pc` and adds the configured console and GUI controller.
pub fn init(vm: &mut Vm<'_>, pc: ObjId) {
    let Some(player) = new_named(vm, "Viewport").or_else(|| new_named(vm, "Player")) else { return };
    let Some(master) = new_named(vm, "InteractionMaster") else { return };
    set(vm, player, "Actor", Value::Object(Some(pc)));
    set(vm, player, "InteractionMaster", Value::Object(Some(master)));
    set(vm, pc, "Player", Value::Object(Some(player)));
    let console_class = world(vm).config.get("Engine.Engine", "Console").map(str::to_string);
    let gui_class = world(vm).config.get("Engine.Engine", "GUIController").map(str::to_string);
    let mut console = None;
    if let Some(c) = console_class {
        console = fire(vm, master, "AddInteraction", &mut [Value::Str(c), Value::Object(None)]).as_obj();
        set(vm, master, "Console", Value::Object(console));
        set(vm, player, "Console", Value::Object(console));
    }
    let mut gui = None;
    if let Some(g) = gui_class {
        gui = fire(vm, master, "AddInteraction", &mut [Value::Str(g), Value::Object(Some(player))]).as_obj();
        set(vm, player, "GUIController", Value::Object(gui));
        // The engine initializes the controller once created: its styles and fonts.
        if let Some(g) = gui {
            fire(vm, g, "InitializeController", &mut []);
        }
    }
    world(vm).viewport = Viewport { player: Some(player), master: Some(master), console, gui };
}

/// Opens a menu page (`Package.Class`) in the GUI controller.
pub fn open_menu(vm: &mut Vm<'_>, page: &str) -> bool {
    let Some(gui) = world(vm).viewport.gui else { return false };
    fire(vm, gui, "OpenMenu", &mut [Value::Str(page.to_string()), Value::Str(String::new()), Value::Str(String::new())]).as_bool()
}

/// Ticks the interactions.
pub fn tick(vm: &mut Vm<'_>, dt: f32) {
    if let Some(m) = world(vm).viewport.master {
        fire(vm, m, "Process_Tick", &mut [Value::Float(dt)]);
    }
}

/// A key or button event (`action`: 1 press, 2 hold, 3 release, 4 axis): whether an interaction took it.
pub fn key_event(vm: &mut Vm<'_>, key: u8, action: u8, delta: f32) -> bool {
    let Some(m) = world(vm).viewport.master else { return false };
    fire(vm, m, "Process_KeyEvent", &mut [Value::Byte(key), Value::Byte(action), Value::Float(delta)]).as_bool()
}

/// A typed character: whether an interaction took it.
pub fn key_type(vm: &mut Vm<'_>, key: u8, unicode: &str) -> bool {
    let Some(m) = world(vm).viewport.master else { return false };
    fire(vm, m, "Process_KeyType", &mut [Value::Byte(key), Value::Str(unicode.to_string())]).as_bool()
}

pub fn pre_render(vm: &mut Vm<'_>, canvas: ObjId) {
    if let Some(m) = world(vm).viewport.master {
        fire(vm, m, "Process_PreRender", &mut [Value::Object(Some(canvas))]);
    }
}

pub fn post_render(vm: &mut Vm<'_>, canvas: ObjId) {
    if let Some(m) = world(vm).viewport.master {
        fire(vm, m, "Process_PostRender", &mut [Value::Object(Some(canvas))]);
    }
}

/// Whether the GUI has a menu open (it then takes the input).
pub fn menu_open(vm: &mut Vm<'_>) -> bool {
    world(vm).viewport.gui.is_some_and(|g| get(vm, g, "bActive").as_bool())
}

pub fn register(vm: &mut Vm<'_>) {
    // Initialize: the native part calls the script's Initialized event.
    vm.register("Interaction.Initialize", |vm, this, _a| {
        if let Some(o) = this {
            fire(vm, o, "Initialized", &mut []);
        }
        Ok(Value::Empty)
    });
    vm.register("InteractionMaster.Travel", |_vm, _this, _a| Ok(Value::Empty));
    vm.register("Interaction.ConsoleCommand", |_vm, _this, _a| Ok(Value::Bool(false)));
    // WorldToScreen / ScreenToWorld need the frame's view; not available yet.
    vm.register("Interaction.WorldToScreen", |_vm, _this, _a| Ok(crate::physics::value(glam::Vec3::ZERO)));
    vm.register("Interaction.ScreenToWorld", |_vm, _this, _a| Ok(crate::physics::value(glam::Vec3::ZERO)));
}
