//! Native OS menu bar (File / Edit / View / Help) via [`muda`].
//!
//! Menu items dispatch the same [`Action`] values as the toolbar and scripts.

use crate::actions::{Action, Pane};
use eframe::CreationContext;
use muda::{
    accelerator::{Accelerator, Code, Modifiers},
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
#[cfg(target_os = "macos")]
use muda::AboutMetadata;
#[cfg(target_os = "windows")]
use raw_window_handle::HasWindowHandle;
use std::sync::{Mutex, OnceLock};

/// What the user chose from the native menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    NewDocument,
    Open,
    Save,
    SaveAs,
    ExportStl,
    ExportSessionCommands,
    Quit,
    UndoLast,
    Clear,
    About,
    ToggleCommandPalette,
    SetPaneVisible { pane: Pane, visible: bool },
}

/// Stable menu-item ids for mapping [`MenuEvent`]s to [`MenuCommand`]s.
#[derive(Clone, Debug)]
pub struct MenuIds {
    pub new_document: MenuId,
    pub open: MenuId,
    pub save: MenuId,
    pub save_as: MenuId,
    pub export_stl: MenuId,
    pub export_session_commands: MenuId,
    pub quit: MenuId,
    pub undo: MenuId,
    pub clear: MenuId,
    pub about: MenuId,
    pub command_palette: MenuId,
    pub pane_checks: Vec<(Pane, MenuId)>,
}

/// Native menu bar and handles for syncing pane checkboxes.
pub struct NativeMenu {
    #[allow(dead_code)]
    menu: Menu,
    ids: MenuIds,
    pane_checks: Vec<(Pane, CheckMenuItem)>,
}

static PENDING_MENU_EVENTS: Mutex<Vec<MenuEvent>> = Mutex::new(Vec::new());
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

fn primary_modifier() -> Modifiers {
    #[cfg(target_os = "macos")]
    {
        Modifiers::SUPER
    }
    #[cfg(not(target_os = "macos"))]
    {
        Modifiers::CONTROL
    }
}

/// Map a menu item id to a [`MenuCommand`], if it belongs to this app menu.
pub fn command_for_id(
    id: &MenuId,
    ids: &MenuIds,
    pane_visible: impl Fn(Pane) -> bool,
) -> Option<MenuCommand> {
    if ids.new_document == id {
        return Some(MenuCommand::NewDocument);
    }
    if ids.open == id {
        return Some(MenuCommand::Open);
    }
    if ids.save == id {
        return Some(MenuCommand::Save);
    }
    if ids.save_as == id {
        return Some(MenuCommand::SaveAs);
    }
    if ids.export_stl == id {
        return Some(MenuCommand::ExportStl);
    }
    if ids.export_session_commands == id {
        return Some(MenuCommand::ExportSessionCommands);
    }
    if ids.quit == id {
        return Some(MenuCommand::Quit);
    }
    if ids.undo == id {
        return Some(MenuCommand::UndoLast);
    }
    if ids.clear == id {
        return Some(MenuCommand::Clear);
    }
    if ids.about == id {
        return Some(MenuCommand::About);
    }
    if ids.command_palette == id {
        return Some(MenuCommand::ToggleCommandPalette);
    }
    for &(pane, ref check_id) in &ids.pane_checks {
        if check_id == id {
            return Some(MenuCommand::SetPaneVisible {
                pane,
                visible: pane_visible(pane),
            });
        }
    }
    None
}

/// Map a menu event to a [`MenuCommand`], if it belongs to this app menu.
pub fn command_for_event(event: &MenuEvent, menu: &NativeMenu) -> Option<MenuCommand> {
    command_for_id(
        event.id(),
        &menu.ids,
        |pane| {
            menu.pane_checks
                .iter()
                .find(|(p, _)| *p == pane)
                .map(|(_, item)| item.is_checked())
                .unwrap_or(true)
        },
    )
}

impl MenuCommand {
    /// Convert to an [`Action`] where the mapping is direct (no file dialogs).
    pub fn to_action(self) -> Option<Action> {
        match self {
            MenuCommand::NewDocument => Some(Action::NewDocument),
            MenuCommand::Open | MenuCommand::Save | MenuCommand::SaveAs => None,
            // Needs a file-save dialog, handled in the app frame loop.
            MenuCommand::ExportStl | MenuCommand::ExportSessionCommands => None,
            MenuCommand::Quit => None,
            MenuCommand::UndoLast => Some(Action::UndoLast),
            MenuCommand::Clear => Some(Action::Clear),
            MenuCommand::About => None,
            MenuCommand::ToggleCommandPalette => Some(Action::ToggleCommandPalette),
            MenuCommand::SetPaneVisible { pane, visible } => {
                Some(Action::SetPaneVisible { pane, visible })
            }
        }
    }
}

impl NativeMenu {
    /// Build and attach the native menu bar to the running application.
    pub fn install(cc: &CreationContext<'_>) -> Result<Self, muda::Error> {
        let _ = EGUI_CTX.set(cc.egui_ctx.clone());
        install_event_handler();

        let menu = Menu::new();
        let primary = primary_modifier();

        #[cfg(target_os = "macos")]
        {
            let app_menu = Submenu::new("BearCAD", true);
            app_menu.append_items(&[
                &PredefinedMenuItem::about(
                    Some("About BearCAD"),
                    Some(AboutMetadata {
                        name: Some("BearCAD".to_string()),
                        version: Some(env!("CARGO_PKG_VERSION").to_string()),
                        copyright: Some("On-device parametric CAD (prototype)".to_string()),
                        ..Default::default()
                    }),
                ),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ])?;
            menu.append(&app_menu)?;
        }

        let file_menu = Submenu::new("File", true);
        let edit_menu = Submenu::new("Edit", true);
        let view_menu = Submenu::new("View", true);
        let panes_menu = Submenu::new("Panes", true);
        let help_menu = Submenu::new("Help", true);

        let new_document = MenuItem::with_id(
            "new_document",
            "New",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyN)),
        );
        let open = MenuItem::with_id(
            "open",
            "Open…",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyO)),
        );
        let save = MenuItem::with_id(
            "save",
            "Save",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyS)),
        );
        let save_as = MenuItem::with_id(
            "save_as",
            "Save As…",
            true,
            Some(Accelerator::new(
                Some(primary | Modifiers::SHIFT),
                Code::KeyS,
            )),
        );
        let export_stl = MenuItem::with_id("export_stl", "Export STL…", true, None);
        let quit = MenuItem::with_id(
            "quit",
            "Quit",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyQ)),
        );
        let undo = MenuItem::with_id(
            "undo",
            "Undo",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyZ)),
        );
        let clear = MenuItem::with_id("clear", "Clear", true, None);
        let command_palette = MenuItem::with_id(
            "command_palette",
            "Command Palette…",
            true,
            Some(Accelerator::new(Some(primary), Code::KeyP)),
        );
        let about = MenuItem::with_id("about", "About BearCAD", true, None);
        let export_session_commands =
            MenuItem::with_id("export_session_commands", "Export Session Commands…", true, None);

        let mut pane_checks = Vec::new();
        let mut pane_ids = Vec::new();
        for &pane in Pane::ALL {
            let check = CheckMenuItem::with_id(
                pane.script_name(),
                pane.label(),
                true,
                true,
                None,
            );
            pane_ids.push((pane, check.id().clone()));
            pane_checks.push((pane, check));
        }

        let file_sep = PredefinedMenuItem::separator();
        file_menu.append(&new_document)?;
        file_menu.append(&open)?;
        file_menu.append(&file_sep)?;
        file_menu.append(&save)?;
        file_menu.append(&save_as)?;
        file_menu.append(&PredefinedMenuItem::separator())?;
        file_menu.append(&export_stl)?;
        #[cfg(not(target_os = "macos"))]
        {
            let quit_sep = PredefinedMenuItem::separator();
            file_menu.append(&quit_sep)?;
            file_menu.append(&quit)?;
        }

        edit_menu.append(&undo)?;
        edit_menu.append(&PredefinedMenuItem::separator())?;
        edit_menu.append(&clear)?;

        let pane_item_refs: Vec<&dyn muda::IsMenuItem> = pane_checks
            .iter()
            .map(|(_, item)| item as &dyn muda::IsMenuItem)
            .collect();
        panes_menu.append_items(&pane_item_refs)?;
        view_menu.append(&command_palette)?;
        view_menu.append(&PredefinedMenuItem::separator())?;
        view_menu.append(&panes_menu)?;
        help_menu.append(&export_session_commands)?;
        help_menu.append(&PredefinedMenuItem::separator())?;
        help_menu.append(&about)?;

        menu.append_items(&[&file_menu, &edit_menu, &view_menu, &help_menu])?;

        attach_to_platform(&menu, cc)?;

        #[cfg(target_os = "macos")]
        help_menu.set_as_help_menu_for_nsapp();

        let ids = MenuIds {
            new_document: new_document.id().clone(),
            open: open.id().clone(),
            save: save.id().clone(),
            save_as: save_as.id().clone(),
            export_stl: export_stl.id().clone(),
            export_session_commands: export_session_commands.id().clone(),
            quit: quit.id().clone(),
            undo: undo.id().clone(),
            clear: clear.id().clone(),
            about: about.id().clone(),
            command_palette: command_palette.id().clone(),
            pane_checks: pane_ids,
        };

        Ok(Self {
            menu,
            ids,
            pane_checks,
        })
    }

    /// Drain pending native menu events received since the last call.
    pub fn drain_events(&self) -> Vec<MenuEvent> {
        let mut pending = PENDING_MENU_EVENTS.lock().expect("menu event queue");
        std::mem::take(&mut *pending)
    }

    /// Keep pane checkmarks aligned with application state.
    pub fn sync_pane_checks(&self, is_visible: impl Fn(Pane) -> bool) {
        for &(pane, ref check) in &self.pane_checks {
            let _ = check.set_checked(is_visible(pane));
        }
    }
}

fn install_event_handler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        MenuEvent::set_event_handler(Some(|event| {
            if let Ok(mut pending) = PENDING_MENU_EVENTS.lock() {
                pending.push(event);
            }
            if let Some(ctx) = EGUI_CTX.get() {
                ctx.request_repaint();
            }
        }));
    });
}

fn attach_to_platform(menu: &Menu, cc: &CreationContext<'_>) -> Result<(), muda::Error> {
    #[cfg(target_os = "macos")]
    {
        let _ = cc;
        menu.init_for_nsapp();
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::RawWindowHandle;
        let handle = cc
            .window_handle()
            .map_err(|_| muda::Error::NotInitialized)?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => unsafe {
                menu.init_for_hwnd(handle.hwnd.get())
            },
            _ => Err(muda::Error::NotInitialized),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (menu, cc);
        // Native menu bar is macOS/Windows only; egui toolbar/palette cover Linux.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_with_pane(pane_id: &str) -> (MenuIds, MenuId) {
        let pane_menu_id = MenuId::new(pane_id);
        let ids = MenuIds {
            new_document: MenuId::new("new_document"),
            open: MenuId::new("open"),
            save: MenuId::new("save"),
            save_as: MenuId::new("save_as"),
            export_stl: MenuId::new("export_stl"),
            export_session_commands: MenuId::new("export_session_commands"),
            quit: MenuId::new("quit"),
            undo: MenuId::new("undo"),
            clear: MenuId::new("clear"),
            about: MenuId::new("about"),
            command_palette: MenuId::new("command_palette"),
            pane_checks: vec![(Pane::ViewCube, pane_menu_id.clone())],
        };
        (ids, pane_menu_id)
    }

    #[test]
    fn maps_file_and_edit_commands() {
        let ids = ids_with_pane("view_cube").0;
        assert_eq!(
            command_for_id(&ids.new_document, &ids, |_| true),
            Some(MenuCommand::NewDocument)
        );
        assert_eq!(
            command_for_id(&ids.open, &ids, |_| true),
            Some(MenuCommand::Open)
        );
        assert_eq!(
            command_for_id(&ids.save, &ids, |_| true),
            Some(MenuCommand::Save)
        );
        assert_eq!(
            command_for_id(&ids.save_as, &ids, |_| true),
            Some(MenuCommand::SaveAs)
        );
        assert_eq!(
            command_for_id(&ids.undo, &ids, |_| true),
            Some(MenuCommand::UndoLast)
        );
        assert_eq!(
            command_for_id(&ids.clear, &ids, |_| true),
            Some(MenuCommand::Clear)
        );
        assert_eq!(
            command_for_id(&ids.export_session_commands, &ids, |_| true),
            Some(MenuCommand::ExportSessionCommands)
        );
    }

    #[test]
    fn maps_command_palette_menu_item() {
        let ids = ids_with_pane("view_cube").0;
        assert_eq!(
            command_for_id(&ids.command_palette, &ids, |_| true),
            Some(MenuCommand::ToggleCommandPalette)
        );
        assert_eq!(
            MenuCommand::ToggleCommandPalette.to_action(),
            Some(Action::ToggleCommandPalette)
        );
    }

    #[test]
    fn maps_pane_checkbox_state() {
        let (ids, pane_id) = ids_with_pane("view_cube");
        assert_eq!(
            command_for_id(&pane_id, &ids, |_| false),
            Some(MenuCommand::SetPaneVisible {
                pane: Pane::ViewCube,
                visible: false,
            })
        );
    }

    #[test]
    fn ignores_unknown_menu_ids() {
        let ids = ids_with_pane("view_cube").0;
        assert_eq!(
            command_for_id(&MenuId::new("unknown"), &ids, |_| true),
            None
        );
    }

    #[test]
    fn direct_actions_skip_dialog_commands() {
        assert_eq!(
            MenuCommand::Open.to_action(),
            None
        );
        assert_eq!(
            MenuCommand::Save.to_action(),
            None
        );
        assert_eq!(
            MenuCommand::About.to_action(),
            None
        );
        assert_eq!(
            MenuCommand::NewDocument.to_action(),
            Some(Action::NewDocument)
        );
        assert_eq!(
            MenuCommand::SetPaneVisible {
                pane: Pane::ViewCube,
                visible: true
            }
            .to_action(),
            Some(Action::SetPaneVisible {
                pane: Pane::ViewCube,
                visible: true
            })
        );
    }
}