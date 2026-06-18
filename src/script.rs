//! Instruction script parser and runner (SPEC §9.3).
//!
//! Scripts are human-readable, one instruction per line. They drive the live UI
//! via synthetic pointer/keyboard events and headless actions.

use crate::actions::{Action, AppState, RectAxis, Tool};
use eframe::egui::{self, Key, Modifiers, PointerButton};
use glam::Vec3;
use std::path::Path;
use std::time::{Duration, Instant};

/// A single script instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    // Document / tool actions
    New,
    Open(String),
    Save(Option<String>),
    Clear,
    Undo,
    Tool(Tool),
    SetDim { axis: RectAxis, value: String },
    FocusDim(RectAxis),
    Orbit { dx: f32, dy: f32 },
    Pan { dx: f32, dy: f32 },
    Zoom { scroll: f32 },

    // Synthetic input (viewport-local pixel coordinates)
    Move { x: f32, y: f32 },
    Click { x: f32, y: f32 },
    /// Move/click at ground-plane world coordinates (millimetres, z = 0).
    MoveGround { x: f32, y: f32 },
    ClickGround { x: f32, y: f32 },
    Drag {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    RightDrag { dx: f32, dy: f32 },
    RightDragShift { dx: f32, dy: f32 },
    Scroll { delta: f32 },
    Key(Key),
    KeyDown(Key),
    KeyUp(Key),
    Type(String),

    // Sequencing
    WaitMs(u64),
    WaitFrames(u32),
    Screenshot(String),
    Quit,
}

impl Instruction {
    /// Format this instruction as a script line (for logging).
    pub fn as_line(&self) -> String {
        match self {
            Instruction::New => "new".to_string(),
            Instruction::Open(path) => format!("open {path}"),
            Instruction::Save(None) => "save".to_string(),
            Instruction::Save(Some(path)) => format!("save {path}"),
            Instruction::Clear => "clear".to_string(),
            Instruction::Undo => "undo".to_string(),
            Instruction::Tool(Tool::Select) => "tool select".to_string(),
            Instruction::Tool(Tool::Rectangle) => "tool rectangle".to_string(),
            Instruction::SetDim { axis, value } => {
                let name = match axis {
                    RectAxis::Width => "width",
                    RectAxis::Height => "height",
                };
                format!("set_dim {name} {value}")
            }
            Instruction::FocusDim(axis) => {
                let name = match axis {
                    RectAxis::Width => "width",
                    RectAxis::Height => "height",
                };
                format!("focus_dim {name}")
            }
            Instruction::Orbit { dx, dy } => format!("orbit {dx} {dy}"),
            Instruction::Pan { dx, dy } => format!("pan {dx} {dy}"),
            Instruction::Zoom { scroll } => format!("zoom {scroll}"),
            Instruction::Move { x, y } => format!("move {x} {y}"),
            Instruction::Click { x, y } => format!("click {x} {y}"),
            Instruction::MoveGround { x, y } => format!("move_ground {x} {y}"),
            Instruction::ClickGround { x, y } => format!("click_ground {x} {y}"),
            Instruction::Drag { x0, y0, x1, y1 } => format!("drag {x0} {y0} {x1} {y1}"),
            Instruction::RightDrag { dx, dy } => format!("right_drag_rel {dx} {dy}"),
            Instruction::RightDragShift { dx, dy } => format!("right_drag_pan {dx} {dy}"),
            Instruction::Scroll { delta } => format!("wheel {delta}"),
            Instruction::Key(key) => format!("key {}", key_name(*key)),
            Instruction::KeyDown(key) => format!("keydown {}", key_name(*key)),
            Instruction::KeyUp(key) => format!("keyup {}", key_name(*key)),
            Instruction::Type(text) => {
                if text.contains(' ') {
                    format!("type \"{text}\"")
                } else {
                    format!("type {text}")
                }
            }
            Instruction::WaitMs(ms) => format!("wait {ms}ms"),
            Instruction::WaitFrames(n) => format!("wait {n}"),
            Instruction::Screenshot(path) => format!("screenshot {path}"),
            Instruction::Quit => "quit".to_string(),
        }
    }
}

/// Parse errors from script files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Parse a script from its text content.
pub fn parse(source: &str) -> Result<Vec<Instruction>, ParseError> {
    let mut instructions = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim();
        let line_no = i + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        instructions.push(parse_line(line, line_no)?);
    }
    Ok(instructions)
}

/// Parse a script file from disk.
pub fn parse_file(path: &Path) -> Result<Vec<Instruction>, ParseError> {
    let source = std::fs::read_to_string(path).map_err(|e| ParseError {
        line: 0,
        message: format!("failed to read {}: {e}", path.display()),
    })?;
    parse(&source)
}

fn parse_line(line: &str, line_no: usize) -> Result<Instruction, ParseError> {
    let err = |msg: &str| ParseError {
        line: line_no,
        message: msg.to_string(),
    };

    let (cmd, rest) = line
        .split_once(char::is_whitespace)
        .map(|(c, r)| (c, r.trim()))
        .unwrap_or((line, ""));

    match cmd.to_ascii_lowercase().as_str() {
        "new" => Ok(Instruction::New),
        "clear" => Ok(Instruction::Clear),
        "undo" => Ok(Instruction::Undo),
        "quit" | "exit" => Ok(Instruction::Quit),

        "open" => {
            let path = rest.trim_matches('"');
            if path.is_empty() {
                return Err(err("open requires a path"));
            }
            Ok(Instruction::Open(path.to_string()))
        }

        "save" => {
            if rest.is_empty() {
                Ok(Instruction::Save(None))
            } else {
                Ok(Instruction::Save(Some(rest.trim_matches('"').to_string())))
            }
        }

        "tool" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            Tool::from_name(name).map(Instruction::Tool).ok_or_else(|| {
                err(&format!("unknown tool '{name}' (expected select or rectangle)"))
            })
        }

        "set_dim" | "setdim" => {
            let mut parts = rest.split_whitespace();
            let axis_name = parts.next().ok_or_else(|| err("set_dim requires axis and value"))?;
            let value = parts.next().ok_or_else(|| err("set_dim requires a value"))?;
            let axis = RectAxis::from_name(axis_name)
                .ok_or_else(|| err(&format!("unknown axis '{axis_name}'")))?;
            Ok(Instruction::SetDim {
                axis,
                value: value.to_string(),
            })
        }

        "focus_dim" | "focusdim" => {
            let axis_name = rest.split_whitespace().next().unwrap_or("");
            let axis = RectAxis::from_name(axis_name)
                .ok_or_else(|| err(&format!("unknown axis '{axis_name}'")))?;
            Ok(Instruction::FocusDim(axis))
        }

        "orbit" | "right_drag" | "rightdrag" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Orbit { dx, dy })
        }

        "pan" | "right_drag_shift" | "rightdragshift" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Pan { dx, dy })
        }

        "zoom" | "scroll" => {
            let delta = parse_one_float(rest, &err)?;
            Ok(Instruction::Zoom { scroll: delta })
        }

        "move" | "mousemove" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Move { x, y })
        }

        "click" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Click { x, y })
        }

        "move_ground" | "moveground" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::MoveGround { x, y })
        }

        "click_ground" | "clickground" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::ClickGround { x, y })
        }

        "drag" => {
            let parts: Vec<f32> = rest
                .split_whitespace()
                .map(|s| s.parse::<f32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| err("drag requires four numbers: x0 y0 x1 y1"))?;
            if parts.len() != 4 {
                return Err(err("drag requires four numbers: x0 y0 x1 y1"));
            }
            Ok(Instruction::Drag {
                x0: parts[0],
                y0: parts[1],
                x1: parts[2],
                y1: parts[3],
            })
        }

        "right_drag_rel" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::RightDrag { dx, dy })
        }

        "right_drag_pan" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::RightDragShift { dx, dy })
        }

        "wheel" => {
            let delta = parse_one_float(rest, &err)?;
            Ok(Instruction::Scroll { delta })
        }

        "key" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name).map(Instruction::Key).map_err(|m| err(&m))
        }

        "keydown" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name)
                .map(Instruction::KeyDown)
                .map_err(|m| err(&m))
        }

        "keyup" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name)
                .map(Instruction::KeyUp)
                .map_err(|m| err(&m))
        }

        "type" => {
            let text = parse_type_text(rest);
            Ok(Instruction::Type(text))
        }

        "wait" => {
            if rest.ends_with("ms") {
                let ms: u64 = rest
                    .trim_end_matches("ms")
                    .trim()
                    .parse()
                    .map_err(|_| err("wait requires a duration like 100ms or 5"))?;
                Ok(Instruction::WaitMs(ms))
            } else {
                let frames: u32 = rest
                    .parse()
                    .map_err(|_| err("wait requires a frame count or duration like 100ms"))?;
                Ok(Instruction::WaitFrames(frames))
            }
        }

        "screenshot" => {
            let path = rest.trim_matches('"');
            if path.is_empty() {
                return Err(err("screenshot requires an output path"));
            }
            Ok(Instruction::Screenshot(path.to_string()))
        }

        _ => Err(err(&format!("unknown instruction '{cmd}'"))),
    }
}

fn parse_type_text(rest: &str) -> String {
    let rest = rest.trim();
    if (rest.starts_with('"') && rest.ends_with('"')) || (rest.starts_with('\'') && rest.ends_with('\'')) {
        rest[1..rest.len() - 1].to_string()
    } else {
        rest.to_string()
    }
}

fn parse_one_float(rest: &str, err: &impl Fn(&str) -> ParseError) -> Result<f32, ParseError> {
    rest.split_whitespace()
        .next()
        .ok_or_else(|| err("expected a number"))?
        .parse()
        .map_err(|_| err("expected a number"))
}

fn parse_two_floats(rest: &str, err: &impl Fn(&str) -> ParseError) -> Result<(f32, f32), ParseError> {
    let mut parts = rest.split_whitespace();
    let x: f32 = parts
        .next()
        .ok_or_else(|| err("expected two numbers"))?
        .parse()
        .map_err(|_| err("expected a number"))?;
    let y: f32 = parts
        .next()
        .ok_or_else(|| err("expected two numbers"))?
        .parse()
        .map_err(|_| err("expected a number"))?;
    Ok((x, y))
}

/// Map a human-readable key name to an egui [`Key`].
pub fn parse_key(name: &str) -> Result<Key, String> {
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Ok(Key::Enter),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" => Ok(Key::Backspace),
        "delete" | "del" => Ok(Key::Delete),
        "left" => Ok(Key::ArrowLeft),
        "right" => Ok(Key::ArrowRight),
        "up" => Ok(Key::ArrowUp),
        "down" => Ok(Key::ArrowDown),
        "space" => Ok(Key::Space),
        "r" => Ok(Key::R),
        "a" => Ok(Key::A),
        "b" => Ok(Key::B),
        "c" => Ok(Key::C),
        "d" => Ok(Key::D),
        "e" => Ok(Key::E),
        "f" => Ok(Key::F),
        "g" => Ok(Key::G),
        "h" => Ok(Key::H),
        "i" => Ok(Key::I),
        "j" => Ok(Key::J),
        "k" => Ok(Key::K),
        "l" => Ok(Key::L),
        "m" => Ok(Key::M),
        "n" => Ok(Key::N),
        "o" => Ok(Key::O),
        "p" => Ok(Key::P),
        "q" => Ok(Key::Q),
        "s" => Ok(Key::S),
        "t" => Ok(Key::T),
        "u" => Ok(Key::U),
        "v" => Ok(Key::V),
        "w" => Ok(Key::W),
        "x" => Ok(Key::X),
        "y" => Ok(Key::Y),
        "z" => Ok(Key::Z),
        "0" => Ok(Key::Num0),
        "1" => Ok(Key::Num1),
        "2" => Ok(Key::Num2),
        "3" => Ok(Key::Num3),
        "4" => Ok(Key::Num4),
        "5" => Ok(Key::Num5),
        "6" => Ok(Key::Num6),
        "7" => Ok(Key::Num7),
        "8" => Ok(Key::Num8),
        "9" => Ok(Key::Num9),
        _ => Err(format!("unknown key '{name}'")),
    }
}

fn key_name(key: Key) -> &'static str {
    match key {
        Key::Enter => "enter",
        Key::Tab => "tab",
        Key::Escape => "escape",
        Key::Backspace => "backspace",
        Key::Delete => "delete",
        Key::ArrowLeft => "left",
        Key::ArrowRight => "right",
        Key::ArrowUp => "up",
        Key::ArrowDown => "down",
        Key::Space => "space",
        Key::R => "r",
        Key::A => "a",
        Key::B => "b",
        Key::C => "c",
        Key::D => "d",
        Key::E => "e",
        Key::F => "f",
        Key::G => "g",
        Key::H => "h",
        Key::I => "i",
        Key::J => "j",
        Key::K => "k",
        Key::L => "l",
        Key::M => "m",
        Key::N => "n",
        Key::O => "o",
        Key::P => "p",
        Key::Q => "q",
        Key::S => "s",
        Key::T => "t",
        Key::U => "u",
        Key::V => "v",
        Key::W => "w",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        _ => "?",
    }
}

/// Queued synthetic pointer/keyboard events injected into egui each frame.
#[derive(Default)]
pub struct SyntheticInput {
    events: Vec<egui::Event>,
    pointer_pos: Option<egui::Pos2>,
    /// When set, secondary-button drag deltas are applied via events.
    pending_right_drag: Option<(egui::Vec2, Modifiers)>,
}

impl SyntheticInput {
    pub fn inject(&mut self, ctx: &egui::Context) {
        if self.events.is_empty() && self.pending_right_drag.is_none() {
            return;
        }
        ctx.input_mut(|input| {
            input.events.extend(self.events.drain(..));
        });
    }

    /// Apply secondary-button drag after egui has processed pointer state.
    pub fn apply_pending_drag(&mut self, viewport: egui::Rect, on_drag: impl FnMut(egui::Vec2, Modifiers, f32)) {
        if let Some((delta, modifiers)) = self.pending_right_drag.take() {
            let mut callback = on_drag;
            callback(delta, modifiers, viewport.height());
        }
    }

    fn viewport_pos(viewport: egui::Rect, x: f32, y: f32) -> egui::Pos2 {
        viewport.min + egui::vec2(x, y)
    }

    pub fn move_to(&mut self, viewport: egui::Rect, x: f32, y: f32) {
        let pos = Self::viewport_pos(viewport, x, y);
        self.pointer_pos = Some(pos);
        self.events.push(egui::Event::PointerMoved(pos));
    }

    pub fn click(&mut self, viewport: egui::Rect, x: f32, y: f32) {
        let pos = Self::viewport_pos(viewport, x, y);
        self.pointer_pos = Some(pos);
        self.events.push(egui::Event::PointerMoved(pos));
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn drag(&mut self, viewport: egui::Rect, x0: f32, y0: f32, x1: f32, y1: f32) {
        let p0 = Self::viewport_pos(viewport, x0, y0);
        let p1 = Self::viewport_pos(viewport, x1, y1);
        self.pointer_pos = Some(p1);
        self.events.push(egui::Event::PointerMoved(p0));
        self.events.push(egui::Event::PointerButton {
            pos: p0,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.events.push(egui::Event::PointerMoved(p1));
        self.events.push(egui::Event::PointerButton {
            pos: p1,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn right_drag(&mut self, viewport: egui::Rect, dx: f32, dy: f32, shift: bool) {
        let pos = self
            .pointer_pos
            .unwrap_or_else(|| viewport.center());
        self.events.push(egui::Event::PointerMoved(pos));
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        });
        self.pending_right_drag = Some((egui::vec2(dx, dy), if shift { Modifiers::SHIFT } else { Modifiers::NONE }));
        self.events.push(egui::Event::PointerButton {
            pos: pos + egui::vec2(dx, dy),
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        });
    }

    pub fn scroll(&mut self, delta: f32) {
        self.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, delta),
            modifiers: Modifiers::NONE,
        });
    }

    pub fn key(&mut self, key: Key) {
        self.push_key(key, true);
        self.push_key(key, false);
    }

    pub fn key_down(&mut self, key: Key) {
        self.push_key(key, true);
    }

    pub fn key_up(&mut self, key: Key) {
        self.push_key(key, false);
    }

    fn push_key(&mut self, key: Key, pressed: bool) {
        self.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn type_text(&mut self, text: &str) {
        self.events.push(egui::Event::Text(text.to_string()));
    }
}

/// Drives a script through the live application, one step at a time.
pub struct ScriptRunner {
    instructions: Vec<Instruction>,
    pc: usize,
    wait_until: Option<Instant>,
    wait_frames_remaining: u32,
    screenshot_pending: Option<String>,
    /// Prevents re-printing an instruction while waiting (e.g. for viewport layout).
    logged_pc: Option<usize>,
    pub verbose: bool,
    pub done: bool,
    pub error: Option<String>,
    pub should_quit: bool,
}

impl ScriptRunner {
    pub fn new(instructions: Vec<Instruction>) -> Self {
        Self {
            instructions,
            pc: 0,
            wait_until: None,
            wait_frames_remaining: 0,
            screenshot_pending: None,
            logged_pc: None,
            verbose: true,
            done: false,
            error: None,
            should_quit: false,
        }
    }

    pub fn from_file(path: &Path) -> Result<Self, ParseError> {
        let runner = Self::new(parse_file(path)?);
        if runner.verbose {
            println!("Running script: {}", path.display());
            println!("---");
        }
        Ok(runner)
    }

    fn log_instruction(&mut self, instr: &Instruction) {
        if self.verbose && self.logged_pc != Some(self.pc) {
            println!("{}", instr.as_line());
            self.logged_pc = Some(self.pc);
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.wait_until.is_some() || self.wait_frames_remaining > 0 || self.screenshot_pending.is_some()
    }

    /// Advance the script. Returns true if a repaint should be requested.
    pub fn tick(
        &mut self,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> bool {
        if self.done {
            return false;
        }

        if let Some(until) = self.wait_until {
            if Instant::now() < until {
                return true;
            }
            self.wait_until = None;
            self.pc += 1;
            self.logged_pc = None;
        }

        if self.wait_frames_remaining > 0 {
            self.wait_frames_remaining -= 1;
            if self.wait_frames_remaining == 0 {
                self.pc += 1;
                self.logged_pc = None;
            }
            return true;
        }

        if self.screenshot_pending.is_some() {
            // Wait for screenshot event to be processed elsewhere.
            return true;
        }

        while self.pc < self.instructions.len() {
            let instr = self.instructions[self.pc].clone();
            self.log_instruction(&instr);
            match self.execute_one(instr, state, synthetic, viewport, ctx) {
                StepResult::Continue => {
                    self.pc += 1;
                }
                StepResult::Wait => return true,
                StepResult::Done => {
                    self.done = true;
                    return false;
                }
            }
        }

        self.done = true;
        if self.verbose {
            println!("---");
            println!("Script complete.");
        }
        false
    }

    /// Called when egui delivers a screenshot response for a pending request.
    pub fn on_screenshot(&mut self, image: &egui::ColorImage) -> Result<(), String> {
        let Some(path) = self.screenshot_pending.take() else {
            return Ok(());
        };
        save_screenshot(&path, image)?;
        self.pc += 1;
        Ok(())
    }
}

enum StepResult {
    Continue,
    Wait,
    Done,
}

impl ScriptRunner {
    fn ground_pointer(
        synthetic: &mut SyntheticInput,
        state: &AppState,
        viewport: Option<egui::Rect>,
        x: f32,
        y: f32,
        click: bool,
    ) {
        let Some(vp) = viewport else { return };
        let world = Vec3::new(x, y, 0.0);
        let mat = state.cam.view_proj(vp);
        let Some(screen) = state.cam.project(world, vp, &mat) else {
            return;
        };
        let local_x = screen.x - vp.min.x;
        let local_y = screen.y - vp.min.y;
        if click {
            synthetic.click(vp, local_x, local_y);
        } else {
            synthetic.move_to(vp, local_x, local_y);
        }
    }

    fn execute_one(
        &mut self,
        instr: Instruction,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> StepResult {
        match instr {
            Instruction::New => {
                state.apply(Action::NewDocument);
                StepResult::Continue
            }
            Instruction::Open(path) => {
                state.apply(Action::Open { path });
                StepResult::Continue
            }
            Instruction::Save(path) => {
                state.apply(Action::Save { path });
                StepResult::Continue
            }
            Instruction::Clear => {
                state.apply(Action::Clear);
                StepResult::Continue
            }
            Instruction::Undo => {
                state.apply(Action::UndoLast);
                StepResult::Continue
            }
            Instruction::Tool(tool) => {
                state.apply(Action::SetTool(tool));
                StepResult::Continue
            }
            Instruction::SetDim { axis, value } => {
                let _ = state.apply(Action::SetRectDimension { axis, value });
                StepResult::Continue
            }
            Instruction::FocusDim(axis) => {
                let _ = state.apply(Action::FocusRectDimension { axis });
                if viewport.is_some() {
                    let id = if axis.index() == 0 {
                        egui::Id::new("cr_width")
                    } else {
                        egui::Id::new("cr_height")
                    };
                    ctx.memory_mut(|m| m.request_focus(id));
                }
                StepResult::Continue
            }
            Instruction::Orbit { dx, dy } => {
                state.apply(Action::OrbitCamera { delta: (dx, dy) });
                StepResult::Continue
            }
            Instruction::Pan { dx, dy } => {
                let h = viewport.map(|r| r.height()).unwrap_or(640.0);
                state.apply(Action::PanCamera {
                    delta: (dx, dy),
                    viewport_height: h,
                });
                StepResult::Continue
            }
            Instruction::Zoom { scroll } => {
                state.apply(Action::ZoomCamera { scroll });
                StepResult::Continue
            }

            Instruction::Move { x, y } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.move_to(vp, x, y);
                StepResult::Continue
            }
            Instruction::Click { x, y } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.click(vp, x, y);
                StepResult::Continue
            }
            Instruction::MoveGround { x, y } => {
                if viewport.is_none() {
                    return StepResult::Wait;
                }
                Self::ground_pointer(synthetic, state, viewport, x, y, false);
                StepResult::Continue
            }
            Instruction::ClickGround { x, y } => {
                if viewport.is_none() {
                    return StepResult::Wait;
                }
                Self::ground_pointer(synthetic, state, viewport, x, y, true);
                StepResult::Continue
            }
            Instruction::Drag { x0, y0, x1, y1 } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.drag(vp, x0, y0, x1, y1);
                StepResult::Continue
            }
            Instruction::RightDrag { dx, dy } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.right_drag(vp, dx, dy, false);
                StepResult::Continue
            }
            Instruction::RightDragShift { dx, dy } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.right_drag(vp, dx, dy, true);
                StepResult::Continue
            }
            Instruction::Scroll { delta } => {
                synthetic.scroll(delta);
                StepResult::Continue
            }
            Instruction::Key(key) => {
                synthetic.key(key);
                StepResult::Continue
            }
            Instruction::KeyDown(key) => {
                synthetic.key_down(key);
                StepResult::Continue
            }
            Instruction::KeyUp(key) => {
                synthetic.key_up(key);
                StepResult::Continue
            }
            Instruction::Type(text) => {
                synthetic.type_text(&text);
                StepResult::Continue
            }

            Instruction::WaitMs(ms) => {
                self.wait_until = Some(Instant::now() + Duration::from_millis(ms));
                StepResult::Wait
            }
            Instruction::WaitFrames(n) => {
                if n == 0 {
                    StepResult::Continue
                } else {
                    self.wait_frames_remaining = n;
                    StepResult::Wait
                }
            }
            Instruction::Screenshot(path) => {
                self.screenshot_pending = Some(path);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                StepResult::Wait
            }
            Instruction::Quit => {
                self.should_quit = true;
                StepResult::Done
            }
        }
    }
}

/// Save an egui [`egui::ColorImage`] to a PNG file.
pub fn save_screenshot(path: &str, image: &egui::ColorImage) -> Result<(), String> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| format!("failed to save screenshot to {path}: {e}"))
}

/// CLI options for script execution.
#[derive(Clone, Debug, Default)]
pub struct ScriptOptions {
    pub script_path: Option<String>,
    pub exit_on_complete: bool,
}

/// Parse command-line arguments for script mode.
pub fn parse_args(args: impl IntoIterator<Item = impl AsRef<str>>) -> ScriptOptions {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.as_ref().to_string())
        .collect();
    let mut opts = ScriptOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--script" => {
                i += 1;
                if i < args.len() {
                    opts.script_path = Some(args[i].clone());
                }
            }
            "--exit" | "--exit-on-complete" => {
                opts.exit_on_complete = true;
            }
            arg if !arg.starts_with('-') && opts.script_path.is_none() => {
                if arg.ends_with(".le3script")
                    || arg.ends_with(".script")
                    || Path::new(arg).extension().is_some_and(|e| e == "le3script")
                {
                    opts.script_path = Some(arg.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    opts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_instructions() {
        let script = r#"
            # setup
            new
            tool rectangle
            click 100 200
            key enter
            screenshot out.png
        "#;
        let ins = parse(script).unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::New,
                Instruction::Tool(Tool::Rectangle),
                Instruction::Click { x: 100.0, y: 200.0 },
                Instruction::Key(Key::Enter),
                Instruction::Screenshot("out.png".to_string()),
            ]
        );
    }

    #[test]
    fn parses_wait_variants() {
        let ins = parse("wait 100ms\nwait 3").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::WaitMs(100), Instruction::WaitFrames(3)]
        );
    }

    #[test]
    fn parses_type_with_quotes() {
        let ins = parse(r#"type "12.5""#).unwrap();
        assert_eq!(ins, vec![Instruction::Type("12.5".to_string())]);
    }

    #[test]
    fn parses_open_save_paths() {
        let ins = parse("open /tmp/test.le3\nsave /tmp/out.le3").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::Open("/tmp/test.le3".to_string()),
                Instruction::Save(Some("/tmp/out.le3".to_string())),
            ]
        );
    }

    #[test]
    fn parse_error_on_unknown_instruction() {
        let err = parse("foobar").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("unknown"));
    }

    #[test]
    fn parse_key_names() {
        assert_eq!(parse_key("enter").unwrap(), Key::Enter);
        assert_eq!(parse_key("ESC").unwrap(), Key::Escape);
        assert!(parse_key("notakey").is_err());
    }

    #[test]
    fn parse_args_finds_script_flag() {
        let opts = parse_args(["le3", "--script", "test.le3script", "--exit"]);
        assert_eq!(opts.script_path.as_deref(), Some("test.le3script"));
        assert!(opts.exit_on_complete);
    }

    #[test]
    fn parse_args_finds_positional_script() {
        let opts = parse_args(["le3", "demo.le3script"]);
        assert_eq!(opts.script_path.as_deref(), Some("demo.le3script"));
    }

    #[test]
    fn instruction_as_line_round_trips() {
        let line = "click 100 200";
        let ins = parse(line).unwrap().into_iter().next().unwrap();
        assert_eq!(ins.as_line(), line);
    }

    #[test]
    fn wait_frames_advances_to_next_instruction() {
        let script = "wait 2\nclear";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
        runner.verbose = false;
        let mut state = AppState::default();
        state.doc.rects.push(crate::model::Rect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        });
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0));

        // Frame 1: start wait 2
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 2);

        // Frame 2: 2 -> 1
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 1);

        // Frame 3: 1 -> 0, advance past wait
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 1);
        assert_eq!(runner.wait_frames_remaining, 0);

        // Frame 4: run clear
        runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
        assert!(state.doc.rects.is_empty());
        assert!(runner.done);
    }

    #[test]
    fn runner_executes_headless_actions() {
        let script = "new\ntool rectangle\nset_dim width 50\norbit 10 5\nclear";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.doc.rects.push(crate::model::Rect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        });

        while !runner.done {
            runner.tick(&mut state, &mut synthetic, Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 40.0),
                egui::vec2(960.0, 560.0),
            )), &egui::Context::default());
        }

        assert!(state.doc.rects.is_empty());
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(runner.error.is_none());
    }
}