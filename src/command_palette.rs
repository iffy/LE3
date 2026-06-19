//! VS Code-style command palette (SPEC §11.2).
//!
//! Lists context-pertinent commands from the shared action layer. Fuzzy search
//! filters the list; Enter runs the selected command.

use crate::actions::{Action, AppState, CommandPaletteState, Pane, Tool};
use crate::camera::StandardView;
use crate::shortcuts;
use eframe::egui::{self, Key, ScrollArea, TextEdit};

/// Stable command id for scripting and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaletteCommandId {
    NewDocument,
    Open,
    Save,
    SaveAs,
    Undo,
    Clear,
    ToolSelect,
    ToolSketch,
    ToolRectangle,
    ToolLine,
    ToolPlane,
    ExitSketch,
    CommitRectangle,
    CommitLine,
    CommitPlane,
    CancelOperation,
    ViewFront,
    ViewBack,
    ViewLeft,
    ViewRight,
    ViewTop,
    ViewBottom,
    ViewHome,
    SetHomeView,
    ToggleProjection,
    ShowPaneHierarchy,
    HidePaneHierarchy,
    ShowPaneParameters,
    HidePaneParameters,
    ShowPaneContext,
    HidePaneContext,
    ShowPaneViewCube,
    HidePaneViewCube,
}

/// What happens when a palette entry is chosen.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteOutcome {
    Action(Action),
    OpenFile,
    SaveFile,
    SaveFileAs,
}

/// One invokable palette entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteCommand {
    pub id: PaletteCommandId,
    pub label: &'static str,
    pub search_text: &'static str,
}

impl PaletteCommand {
    const fn new(id: PaletteCommandId, label: &'static str, search_text: &'static str) -> Self {
        Self {
            id,
            label,
            search_text,
        }
    }

    pub fn outcome(self) -> PaletteOutcome {
        match self.id {
            PaletteCommandId::NewDocument => PaletteOutcome::Action(Action::NewDocument),
            PaletteCommandId::Open => PaletteOutcome::OpenFile,
            PaletteCommandId::Save => PaletteOutcome::SaveFile,
            PaletteCommandId::SaveAs => PaletteOutcome::SaveFileAs,
            PaletteCommandId::Undo => PaletteOutcome::Action(Action::UndoLast),
            PaletteCommandId::Clear => PaletteOutcome::Action(Action::Clear),
            PaletteCommandId::ToolSelect => PaletteOutcome::Action(Action::SetTool(Tool::Select)),
            PaletteCommandId::ToolSketch => PaletteOutcome::Action(Action::SetTool(Tool::Sketch)),
            PaletteCommandId::ToolRectangle => {
                PaletteOutcome::Action(Action::SetTool(Tool::Rectangle))
            }
            PaletteCommandId::ToolLine => PaletteOutcome::Action(Action::SetTool(Tool::Line)),
            PaletteCommandId::ToolPlane => {
                PaletteOutcome::Action(Action::SetTool(Tool::ConstructionPlane))
            }
            PaletteCommandId::ExitSketch => PaletteOutcome::Action(Action::ExitSketch),
            PaletteCommandId::CommitRectangle => PaletteOutcome::Action(Action::CommitRectangle),
            PaletteCommandId::CommitLine => PaletteOutcome::Action(Action::CommitLine),
            PaletteCommandId::CommitPlane => {
                PaletteOutcome::Action(Action::CommitConstructionPlane)
            }
            PaletteCommandId::CancelOperation => PaletteOutcome::Action(Action::CancelOperation),
            PaletteCommandId::ViewFront => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Front))
            }
            PaletteCommandId::ViewBack => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Back))
            }
            PaletteCommandId::ViewLeft => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Left))
            }
            PaletteCommandId::ViewRight => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Right))
            }
            PaletteCommandId::ViewTop => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Top))
            }
            PaletteCommandId::ViewBottom => {
                PaletteOutcome::Action(Action::SetStandardView(StandardView::Bottom))
            }
            PaletteCommandId::ViewHome => PaletteOutcome::Action(Action::ViewHome),
            PaletteCommandId::SetHomeView => PaletteOutcome::Action(Action::SetHomeView),
            PaletteCommandId::ToggleProjection => {
                PaletteOutcome::Action(Action::ToggleProjectionMode)
            }
            PaletteCommandId::ShowPaneHierarchy => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Hierarchy,
                visible: true,
            }),
            PaletteCommandId::HidePaneHierarchy => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Hierarchy,
                visible: false,
            }),
            PaletteCommandId::ShowPaneParameters => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Parameters,
                visible: true,
            }),
            PaletteCommandId::HidePaneParameters => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Parameters,
                visible: false,
            }),
            PaletteCommandId::ShowPaneContext => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Context,
                visible: true,
            }),
            PaletteCommandId::HidePaneContext => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::Context,
                visible: false,
            }),
            PaletteCommandId::ShowPaneViewCube => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::ViewCube,
                visible: true,
            }),
            PaletteCommandId::HidePaneViewCube => PaletteOutcome::Action(Action::SetPaneVisible {
                pane: Pane::ViewCube,
                visible: false,
            }),
        }
    }
}

/// Fuzzy-match `query` as a subsequence of `target`. Higher scores are better.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    let q: Vec<char> = query.trim().to_ascii_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = target.to_ascii_lowercase().chars().collect();
    let mut score = 0i32;
    let mut qi = 0usize;
    let mut prev_match: Option<usize> = None;
    for (ti, &tc) in t.iter().enumerate() {
        if qi < q.len() && tc == q[qi] {
            score += 1;
            if prev_match == Some(ti.saturating_sub(1)) {
                score += 4;
            }
            if ti == 0 || !t[ti - 1].is_ascii_alphanumeric() {
                score += 8;
            }
            if q[qi].is_ascii_alphanumeric() && (ti == 0 || !t[ti - 1].is_ascii_alphanumeric()) {
                score += 6;
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(score)
    } else {
        None
    }
}

/// Commands available for the current application state.
pub fn commands_for_state(state: &AppState) -> Vec<PaletteCommand> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<PaletteCommand>, cmd: PaletteCommand| out.push(cmd);

    for &cmd in BASE_COMMANDS {
        push(&mut out, cmd);
    }

    if state.sketch_session.is_some() {
        push(
            &mut out,
            PaletteCommand::new(
                PaletteCommandId::ExitSketch,
                "Exit Sketch",
                "exit sketch leave edit mode",
            ),
        );
    }
    if state.creating_rect.is_some() {
        push(
            &mut out,
            PaletteCommand::new(
                PaletteCommandId::CommitRectangle,
                "Commit Rectangle",
                "commit rectangle enter finish",
            ),
        );
    }
    if state.creating_line.is_some() {
        push(
            &mut out,
            PaletteCommand::new(
                PaletteCommandId::CommitLine,
                "Commit Line",
                "commit line enter finish",
            ),
        );
    }
    if state.creating_plane.is_some() {
        push(
            &mut out,
            PaletteCommand::new(
                PaletteCommandId::CommitPlane,
                "Commit Construction Plane",
                "commit plane construction enter finish",
            ),
        );
    }

    for &(pane, show, hide) in PANE_COMMANDS {
        if state.panes.is_visible(pane) {
            push(&mut out, hide);
        } else {
            push(&mut out, show);
        }
    }

    out
}

const PANE_COMMANDS: &[(Pane, PaletteCommand, PaletteCommand)] = &[
    (
        Pane::Hierarchy,
        PaletteCommand::new(
            PaletteCommandId::ShowPaneHierarchy,
            "Show Tree Pane",
            "show tree pane hierarchy dag browser",
        ),
        PaletteCommand::new(
            PaletteCommandId::HidePaneHierarchy,
            "Hide Tree Pane",
            "hide tree pane hierarchy dag browser",
        ),
    ),
    (
        Pane::Parameters,
        PaletteCommand::new(
            PaletteCommandId::ShowPaneParameters,
            "Show Parameters Pane",
            "show parameters pane params variables",
        ),
        PaletteCommand::new(
            PaletteCommandId::HidePaneParameters,
            "Hide Parameters Pane",
            "hide parameters pane params variables",
        ),
    ),
    (
        Pane::Context,
        PaletteCommand::new(
            PaletteCommandId::ShowPaneContext,
            "Show Context Pane",
            "show context pane properties selection",
        ),
        PaletteCommand::new(
            PaletteCommandId::HidePaneContext,
            "Hide Context Pane",
            "hide context pane properties selection",
        ),
    ),
    (
        Pane::ViewCube,
        PaletteCommand::new(
            PaletteCommandId::ShowPaneViewCube,
            "Show Orientation Cube Pane",
            "show orientation cube pane view hud",
        ),
        PaletteCommand::new(
            PaletteCommandId::HidePaneViewCube,
            "Hide Orientation Cube Pane",
            "hide orientation cube pane view hud",
        ),
    ),
];

const BASE_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand::new(
        PaletteCommandId::NewDocument,
        "New Document",
        "new document file create",
    ),
    PaletteCommand::new(PaletteCommandId::Open, "Open…", "open file document load"),
    PaletteCommand::new(PaletteCommandId::Save, "Save", "save file document write"),
    PaletteCommand::new(
        PaletteCommandId::SaveAs,
        "Save As…",
        "save as file document export",
    ),
    PaletteCommand::new(PaletteCommandId::Undo, "Undo", "undo revert last"),
    PaletteCommand::new(PaletteCommandId::Clear, "Clear Document", "clear document delete all"),
    PaletteCommand::new(
        PaletteCommandId::ToolSelect,
        "Select Tool",
        "select tool navigation mode",
    ),
    PaletteCommand::new(
        PaletteCommandId::ToolSketch,
        "Sketch Tool",
        "sketch tool edit face",
    ),
    PaletteCommand::new(
        PaletteCommandId::ToolRectangle,
        "Rectangle Tool",
        "rectangle tool rect draw",
    ),
    PaletteCommand::new(PaletteCommandId::ToolLine, "Line Tool", "line tool draw segment"),
    PaletteCommand::new(
        PaletteCommandId::ToolPlane,
        "Construction Plane Tool",
        "construction plane tool datum",
    ),
    PaletteCommand::new(
        PaletteCommandId::CancelOperation,
        "Cancel Operation",
        "cancel escape abort operation",
    ),
    PaletteCommand::new(PaletteCommandId::ViewFront, "View Front", "view front standard camera"),
    PaletteCommand::new(PaletteCommandId::ViewBack, "View Back", "view back standard camera"),
    PaletteCommand::new(PaletteCommandId::ViewLeft, "View Left", "view left standard camera"),
    PaletteCommand::new(PaletteCommandId::ViewRight, "View Right", "view right standard camera"),
    PaletteCommand::new(PaletteCommandId::ViewTop, "View Top", "view top standard camera"),
    PaletteCommand::new(
        PaletteCommandId::ViewBottom,
        "View Bottom",
        "view bottom standard camera",
    ),
    PaletteCommand::new(PaletteCommandId::ViewHome, "View Home", "view home camera reset"),
    PaletteCommand::new(
        PaletteCommandId::SetHomeView,
        "Set Home View",
        "set home view camera bookmark",
    ),
    PaletteCommand::new(
        PaletteCommandId::ToggleProjection,
        "Toggle Projection Mode",
        "toggle projection orthographic natural perspective camera",
    ),
];

/// Filter and rank commands for the current query.
pub fn filter_commands<'a>(
    query: &str,
    commands: &'a [PaletteCommand],
) -> Vec<(&'a PaletteCommand, i32)> {
    let mut matches: Vec<(&PaletteCommand, i32)> = commands
        .iter()
        .filter_map(|cmd| {
            let label_score = fuzzy_score(query, cmd.label)?;
            let text_score = fuzzy_score(query, cmd.search_text).unwrap_or(0);
            Some((cmd, label_score.max(text_score)))
        })
        .collect();
    matches.sort_by(|a, b| {
        b.1
            .cmp(&a.1)
            .then_with(|| a.0.label.cmp(b.0.label))
    });
    matches
}

/// Best matching command for a query, if any.
pub fn best_match(query: &str, commands: &[PaletteCommand]) -> Option<PaletteCommand> {
    filter_commands(query, commands)
        .first()
        .map(|(cmd, _)| **cmd)
}

/// Draw the palette console and return a chosen outcome.
pub fn show_palette(
    ui: &mut egui::Ui,
    state: &mut CommandPaletteState,
    matches: &[(&PaletteCommand, i32)],
) -> Option<PaletteOutcome> {
    if state.query != state.prior_query {
        state.selected = 0;
        state.prior_query = state.query.clone();
    }

    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    let up = ui.input(|i| i.key_pressed(Key::ArrowUp));
    let down = ui.input(|i| i.key_pressed(Key::ArrowDown));

    if escape {
        state.close_palette();
        return None;
    }

    if !matches.is_empty() {
        if down {
            state.selected = (state.selected + 1).min(matches.len() - 1);
        }
        if up {
            state.selected = state.selected.saturating_sub(1);
        }
    } else {
        state.selected = 0;
    }

    if state.selected >= matches.len() {
        state.selected = matches.len().saturating_sub(1);
    }

    ui.vertical(|ui| {
        if !matches.is_empty() {
            ScrollArea::vertical()
                .max_height(220.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, (cmd, _score)) in matches.iter().enumerate() {
                        let selected = index == state.selected;
                        if shortcuts::action_row(
                            ui,
                            selected,
                            cmd.label,
                            shortcuts::palette_command_shortcut(cmd.id),
                        )
                        .clicked()
                        {
                            state.selected = index;
                        }
                    }
                });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
        } else if !state.query.trim().is_empty() {
            ui.label("No matching commands");
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(">").monospace().strong());
            let response = ui.add(
                TextEdit::singleline(&mut state.query)
                    .hint_text("Type a command…")
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(14.0)),
            );
            if state.request_focus {
                response.request_focus();
                state.request_focus = false;
            }
            if enter && !matches.is_empty() {
                return;
            }
        });
    });

    if enter {
        return matches
            .get(state.selected)
            .map(|(cmd, _)| cmd.outcome());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::SketchSession;

    #[test]
    fn fuzzy_score_matches_subsequence() {
        assert!(fuzzy_score("nd", "New Document").is_some());
        assert!(fuzzy_score("rect", "Rectangle Tool").is_some());
        assert!(fuzzy_score("v fr", "View Front").is_some());
    }

    #[test]
    fn fuzzy_score_rejects_non_matches() {
        assert!(fuzzy_score("xyz", "New Document").is_none());
    }

    #[test]
    fn fuzzy_score_empty_query_matches_all() {
        assert_eq!(fuzzy_score("", "Anything"), Some(0));
    }

    #[test]
    fn filter_commands_ranks_better_matches_first() {
        let cmds = commands_for_state(&AppState::default());
        let filtered = filter_commands("new", &cmds);
        assert!(!filtered.is_empty());
        assert_eq!(filtered[0].0.id, PaletteCommandId::NewDocument);
    }

    #[test]
    fn exit_sketch_only_when_editing_sketch() {
        let mut state = AppState::default();
        assert!(
            !commands_for_state(&state)
                .iter()
                .any(|c| c.id == PaletteCommandId::ExitSketch)
        );
        state.sketch_session = Some(SketchSession { sketch: 0 });
        assert!(
            commands_for_state(&state)
                .iter()
                .any(|c| c.id == PaletteCommandId::ExitSketch)
        );
    }

    #[test]
    fn pane_commands_reflect_visibility() {
        let mut state = AppState::default();
        state.panes.set(Pane::Parameters, false);
        let cmds = commands_for_state(&state);
        assert!(
            cmds.iter()
                .any(|c| c.id == PaletteCommandId::ShowPaneParameters)
        );
        assert!(
            !cmds.iter()
                .any(|c| c.id == PaletteCommandId::HidePaneParameters)
        );
    }

    #[test]
    fn best_match_finds_tool_by_alias() {
        let cmds = commands_for_state(&AppState::default());
        let cmd = best_match("rect", &cmds).unwrap();
        assert_eq!(cmd.id, PaletteCommandId::ToolRectangle);
    }

    #[test]
    fn palette_shortcuts_include_tools_and_commit() {
        assert_eq!(
            shortcuts::palette_command_shortcut(PaletteCommandId::ToolRectangle),
            Some(shortcuts::ShortcutHint::plain("R"))
        );
        assert_eq!(
            shortcuts::palette_command_shortcut(PaletteCommandId::CommitRectangle),
            Some(shortcuts::ShortcutHint::plain("Enter"))
        );
    }

    #[test]
    fn palette_command_maps_to_action() {
        let cmd = PaletteCommand::new(
            PaletteCommandId::ViewTop,
            "View Top",
            "view top",
        );
        assert_eq!(
            cmd.outcome(),
            PaletteOutcome::Action(Action::SetStandardView(StandardView::Top))
        );
    }
}