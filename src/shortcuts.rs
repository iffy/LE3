//! Default keyboard shortcut labels for in-app UI (SPEC §11.3).
//!
//! Modifier shortcuts use the platform primary key (⌘ on macOS, Ctrl elsewhere).
//! Viewport tool keys are single-letter and shown on toolbar buttons.

use crate::actions::Tool;
use crate::command_palette::PaletteCommandId;
use eframe::egui::{self, Align, Layout, RichText, Ui};

/// A displayable keyboard shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutHint {
    pub key: &'static str,
    pub modifiers: ShortcutModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutModifiers {
    None,
    Primary,
    PrimaryShift,
}

impl ShortcutHint {
    pub const fn plain(key: &'static str) -> Self {
        Self {
            key,
            modifiers: ShortcutModifiers::None,
        }
    }

    pub const fn primary(key: &'static str) -> Self {
        Self {
            key,
            modifiers: ShortcutModifiers::Primary,
        }
    }

    pub const fn primary_shift(key: &'static str) -> Self {
        Self {
            key,
            modifiers: ShortcutModifiers::PrimaryShift,
        }
    }
}

pub fn primary_modifier_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "⌘"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl"
    }
}

pub fn format_shortcut(hint: ShortcutHint) -> String {
    match hint.modifiers {
        ShortcutModifiers::None => hint.key.to_string(),
        ShortcutModifiers::Primary => format!("{}+{}", primary_modifier_label(), hint.key),
        ShortcutModifiers::PrimaryShift => {
            #[cfg(target_os = "macos")]
            {
                format!("{}+⇧+{}", primary_modifier_label(), hint.key)
            }
            #[cfg(not(target_os = "macos"))]
            {
                format!("{}+Shift+{}", primary_modifier_label(), hint.key)
            }
        }
    }
}

pub fn tool_shortcut(tool: Tool) -> Option<ShortcutHint> {
    match tool {
        Tool::Sketch => Some(ShortcutHint::plain("S")),
        Tool::Rectangle => Some(ShortcutHint::plain("R")),
        Tool::Line => Some(ShortcutHint::plain("L")),
        Tool::ConstructionPlane => Some(ShortcutHint::plain("P")),
        Tool::Dimension => Some(ShortcutHint::plain("D")),
        Tool::Select => None,
    }
}

pub const TOGGLE_CONSTRUCTION: ShortcutHint = ShortcutHint::plain("X");
pub const CANCEL_OPERATION: ShortcutHint = ShortcutHint::plain("Esc");
pub const UNDO: ShortcutHint = ShortcutHint::primary("Z");

pub fn palette_command_shortcut(id: PaletteCommandId) -> Option<ShortcutHint> {
    match id {
        PaletteCommandId::NewDocument => Some(ShortcutHint::primary("N")),
        PaletteCommandId::Open => Some(ShortcutHint::primary("O")),
        PaletteCommandId::Save => Some(ShortcutHint::primary("S")),
        PaletteCommandId::SaveAs => Some(ShortcutHint::primary_shift("S")),
        PaletteCommandId::Undo => Some(UNDO),
        PaletteCommandId::ToolSketch => tool_shortcut(Tool::Sketch),
        PaletteCommandId::ToolRectangle => tool_shortcut(Tool::Rectangle),
        PaletteCommandId::ToolLine => tool_shortcut(Tool::Line),
        PaletteCommandId::ToolPlane => tool_shortcut(Tool::ConstructionPlane),
        PaletteCommandId::ToolDimension => tool_shortcut(Tool::Dimension),
        PaletteCommandId::CancelOperation => Some(CANCEL_OPERATION),
        PaletteCommandId::CommitRectangle
        | PaletteCommandId::CommitLine
        | PaletteCommandId::CommitPlane => Some(ShortcutHint::plain("Enter")),
        _ => None,
    }
}

/// Label with an adjacent parenthetical shortcut, e.g. `Sketch (S)`.
pub fn compact_label(label: &str, shortcut: Option<ShortcutHint>) -> String {
    match shortcut {
        Some(hint) => format!("{label} ({})", format_shortcut(hint)),
        None => label.to_string(),
    }
}

fn shortcut_rich_text(hint: ShortcutHint) -> RichText {
    RichText::new(format_shortcut(hint))
        .weak()
        .monospace()
        .size(11.0)
}

/// Row with primary label on the left and shortcut right-aligned (palette-style).
pub fn action_row(ui: &mut Ui, selected: bool, label: &str, shortcut: Option<ShortcutHint>) -> egui::Response {
    ui.horizontal(|ui| {
        let row_w = ui.available_width();
        let response = ui.selectable_label(selected, label);
        if let Some(hint) = shortcut {
            let shortcut_w = (row_w - response.rect.width()).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(shortcut_w, response.rect.height()),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.label(shortcut_rich_text(hint));
                },
            );
        }
        response
    })
    .inner
}

/// Checkbox with shortcut shown to the right of the label.
pub fn checkbox_with_shortcut(
    ui: &mut Ui,
    checked: &mut bool,
    label: &str,
    shortcut: Option<ShortcutHint>,
) -> egui::Response {
    ui.horizontal(|ui| {
        let response = ui.checkbox(checked, label);
        if let Some(hint) = shortcut {
            ui.label(shortcut_rich_text(hint));
        }
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_plain_shortcut() {
        assert_eq!(format_shortcut(ShortcutHint::plain("R")), "R");
        assert_eq!(format_shortcut(ShortcutHint::plain("Esc")), "Esc");
    }

    #[test]
    fn format_primary_shortcut_uses_platform_modifier() {
        let formatted = format_shortcut(ShortcutHint::primary("Z"));
        assert!(formatted.ends_with("+Z"));
        assert!(formatted.contains(primary_modifier_label()));
    }

    #[test]
    fn tool_shortcuts_match_viewport_bindings() {
        assert_eq!(
            tool_shortcut(Tool::Rectangle),
            Some(ShortcutHint::plain("R"))
        );
        assert_eq!(tool_shortcut(Tool::Select), None);
    }

    #[test]
    fn palette_maps_document_shortcuts() {
        assert_eq!(
            palette_command_shortcut(PaletteCommandId::Undo),
            Some(UNDO)
        );
        assert_eq!(
            palette_command_shortcut(PaletteCommandId::CancelOperation),
            Some(CANCEL_OPERATION)
        );
    }

    #[test]
    fn compact_label_includes_shortcut() {
        assert_eq!(
            compact_label("Sketch", tool_shortcut(Tool::Sketch)),
            "Sketch (S)"
        );
        assert_eq!(compact_label("Select", None), "Select");
    }
}