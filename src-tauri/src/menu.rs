//! macOS 原生菜单（只有 macOS 才有）。

#[cfg(target_os = "macos")]
pub fn install_menu(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, PredefinedMenuItem, SubmenuBuilder};

    let handle = app.handle();
    let app_menu = SubmenuBuilder::new(handle, "VoiceNotes")
        .item(&PredefinedMenuItem::about(handle, None, None)?)
        .separator()
        .item(&PredefinedMenuItem::hide(handle, None)?)
        .item(&PredefinedMenuItem::hide_others(handle, None)?)
        .item(&PredefinedMenuItem::show_all(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::quit(handle, None)?)
        .build()?;
    let edit_menu = SubmenuBuilder::new(handle, "Edit")
        .item(&PredefinedMenuItem::undo(handle, None)?)
        .item(&PredefinedMenuItem::redo(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(handle, None)?)
        .item(&PredefinedMenuItem::copy(handle, None)?)
        .item(&PredefinedMenuItem::paste(handle, None)?)
        .item(&PredefinedMenuItem::select_all(handle, None)?)
        .build()?;
    let view_menu = SubmenuBuilder::new(handle, "View")
        .item(&PredefinedMenuItem::fullscreen(handle, None)?)
        .build()?;
    let menu = MenuBuilder::new(handle)
        .items(&[&app_menu, &edit_menu, &view_menu])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

/// 更新 Edit / View 两个子菜单的标题。
///
/// 文案由前端字典（`menu.edit` / `menu.view`）传进来：Rust 里不存任何菜单文案，
/// 加语言只改字典。
#[cfg(target_os = "macos")]
pub fn set_texts(app: &tauri::AppHandle, edit: &str, view: &str) -> tauri::Result<()> {
    use tauri::menu::MenuItemKind;
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let items = menu.items()?;
    for (index, text) in [(1, edit), (2, view)] {
        if let Some(MenuItemKind::Submenu(item)) = items.get(index) {
            item.set_text(text)?;
        }
    }
    Ok(())
}
