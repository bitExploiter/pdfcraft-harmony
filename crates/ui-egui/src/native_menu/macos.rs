//! Main-thread-only AppKit adapter. Keep handles alive; update items in place, not the menu bar
//! on every frame. A bounded channel wakes egui even when its window was idle or minimized.

use std::{
    cell::RefCell,
    sync::mpsc::{Receiver, sync_channel},
};

use muda::accelerator::{Key, KeyAccelerator, Modifiers, NamedKey};
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use super::{Entry, Shortcut};

thread_local! {
    static INSTALLED: RefCell<Option<NativeMenus>> = const { RefCell::new(None) };
}

struct NativeMenus {
    _menu: Menu,
    about: MenuItem,
    preferences: MenuItem,
    quit: MenuItem,
    nodes: Vec<Node>,
}

struct Node {
    state: Entry,
    kind: Kind,
}

enum Kind {
    Menu(Submenu, Vec<Node>),
    Item(MenuItem),
    Check(CheckMenuItem),
}

impl Node {
    fn inspect(&self) -> serde_json::Value {
        use serde_json::json;
        match &self.kind {
            Kind::Menu(menu, children) => json!({"id": self.state.id, "label": menu.text(), "enabled": menu.is_enabled(),
                "children": children.iter().map(Self::inspect).collect::<Vec<_>>() }),
            Kind::Item(item) => json!({"id": self.state.id, "label": item.text(), "enabled": item.is_enabled(),
                "shortcut": self.state.shortcut.map(|s| s.label(true))}),
            Kind::Check(item) => json!({"id": self.state.id, "label": item.text(), "enabled": item.is_enabled(), "checked": item.is_checked()}),
        }
    }

    fn new(entry: &Entry) -> muda::Result<Self> {
        let text = entry.label.replace('&', "&&");
        let kind = if !entry.children.is_empty() {
            let submenu = Submenu::with_id(&entry.id, text, entry.enabled);
            let children: Vec<_> = entry.children.iter().map(Self::new).collect::<muda::Result<_>>()?;
            for child in &children {
                submenu.append(child.item())?;
            }
            Kind::Menu(submenu, children)
        } else if let Some(checked) = entry.checked {
            let item = CheckMenuItem::with_id(&entry.id, text, entry.enabled, checked, None);
            item.set_key_accelerator(entry.shortcut.map(accelerator))?;
            Kind::Check(item)
        } else {
            let item = MenuItem::with_id(&entry.id, text, entry.enabled, None);
            item.set_key_accelerator(entry.shortcut.map(accelerator))?;
            Kind::Item(item)
        };
        Ok(Self { state: entry.clone(), kind })
    }

    fn item(&self) -> &dyn IsMenuItem {
        match &self.kind {
            Kind::Menu(menu, _) => menu,
            Kind::Item(item) => item,
            Kind::Check(item) => item,
        }
    }

    fn same_shape(&self, entry: &Entry) -> bool {
        self.state.id == entry.id
            && self.state.checked.is_some() == entry.checked.is_some()
            && self.state.children.is_empty() == entry.children.is_empty()
    }

    fn sync(&mut self, entry: &Entry) -> muda::Result<()> {
        let text = entry.label.replace('&', "&&");
        match &mut self.kind {
            Kind::Menu(menu, children) => {
                if entry.label != self.state.label {
                    menu.set_text(&text);
                }
                if entry.enabled != self.state.enabled {
                    menu.set_enabled(entry.enabled);
                }
                if children.len() != entry.children.len() || children.iter().zip(&entry.children).any(|(a, b)| !a.same_shape(b)) {
                    // Build first: a failed allocation/platform operation leaves the old menu usable.
                    let replacement: Vec<_> = entry.children.iter().map(Self::new).collect::<muda::Result<_>>()?;
                    for old in children.iter() {
                        menu.remove(old.item())?;
                    }
                    for child in &replacement {
                        menu.append(child.item())?;
                    }
                    *children = replacement;
                } else {
                    for (child, state) in children.iter_mut().zip(&entry.children) {
                        child.sync(state)?;
                    }
                }
            }
            Kind::Item(item) => {
                if entry.label != self.state.label {
                    item.set_text(&text);
                }
                if entry.enabled != self.state.enabled {
                    item.set_enabled(entry.enabled);
                }
            }
            Kind::Check(item) => {
                if entry.label != self.state.label {
                    item.set_text(&text);
                }
                if entry.enabled != self.state.enabled {
                    item.set_enabled(entry.enabled);
                }
                // AppKit toggles check items before sending their event. The app is authoritative,
                // including when a stale event is refused because a modal just opened.
                if let Some(checked) = entry.checked
                    && item.is_checked() != checked
                {
                    item.set_checked(checked);
                }
            }
        }
        self.state = entry.clone();
        Ok(())
    }
}

fn accelerator(shortcut: Shortcut) -> KeyAccelerator {
    let mut modifiers = Modifiers::empty();
    if shortcut.command {
        modifiers |= Modifiers::META;
    }
    if shortcut.mac_ctrl {
        modifiers |= Modifiers::CONTROL;
    }
    if shortcut.shift {
        modifiers |= Modifiers::SHIFT;
    }
    let key = match shortcut.key {
        "Tab" => Key::Named(NamedKey::Tab),
        "−" => Key::Character("-".into()),
        key => Key::Character(key.to_lowercase()),
    };
    KeyAccelerator::new(modifiers, key)
}

pub(super) fn install(ctx: &egui::Context, entries: &[Entry]) -> muda::Result<Receiver<String>> {
    let menu = Menu::new();
    let application = Submenu::new("PdfCraft", true);
    let about = MenuItem::with_id("help.about", tl!("About PdfCraft"), true, None);
    let preferences = MenuItem::with_id("app.preferences", tl!("Preferences…"), true, None);
    preferences.set_key_accelerator(Some(accelerator(Shortcut::cmd(","))))?;
    let quit = MenuItem::with_id(craft_quit::QUIT_MENU_ID, format!("{} PdfCraft", tl!("Quit")), true, None);
    quit.set_key_accelerator(Some(accelerator(Shortcut::cmd("Q"))))?;
    application.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &preferences,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some("Hide PdfCraft")),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    menu.append(&application)?;
    let nodes: Vec<_> = entries.iter().map(Node::new).collect::<muda::Result<_>>()?;
    for node in &nodes {
        menu.append(node.item())?;
        if node.state.id == "Help"
            && let Kind::Menu(submenu, _) = &node.kind
        {
            submenu.set_as_help_menu_for_nsapp();
        }
    }
    let (tx, rx) = sync_channel(64);
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if !craft_quit::on_menu_event(&ctx, event.id.as_ref()) {
            let _ = tx.try_send(event.id.0);
        }
        ctx.request_repaint();
    }));
    menu.init_for_nsapp();
    INSTALLED.set(Some(NativeMenus { _menu: menu, about, preferences, quit, nodes }));
    Ok(rx)
}

pub(super) fn sync(entries: &[Entry]) -> muda::Result<()> {
    INSTALLED.with_borrow_mut(|installed| {
        if let Some(installed) = installed {
            for (id, item) in [("help.about", &installed.about), ("app.preferences", &installed.preferences)] {
                if let Some(state) = entries.iter().find_map(|entry| entry.find(id)) {
                    let text = state.label.replace('&', "&&");
                    if item.text() != text {
                        item.set_text(text);
                    }
                    if item.is_enabled() != state.enabled {
                        item.set_enabled(state.enabled);
                    }
                }
            }
            let quit_text = format!("{} PdfCraft", tl!("Quit"));
            if installed.quit.text() != quit_text {
                installed.quit.set_text(quit_text);
            }
            for (node, entry) in installed.nodes.iter_mut().zip(entries) {
                node.sync(entry)?;
            }
        }
        Ok(())
    })
}

pub(super) fn inspect() -> serde_json::Value {
    INSTALLED.with_borrow(|installed| match installed {
        Some(menus) => {
            let app = serde_json::json!({"id": "PdfCraft", "label": "PdfCraft", "children": [
                {"id": "help.about", "label": menus.about.text(), "enabled": menus.about.is_enabled()},
                {"id": "app.preferences", "label": menus.preferences.text(), "enabled": menus.preferences.is_enabled()},
                {"id": craft_quit::QUIT_MENU_ID, "label": menus.quit.text(), "enabled": true}
            ]});
            serde_json::json!({"installed": true, "menus": std::iter::once(app).chain(menus.nodes.iter().map(Node::inspect)).collect::<Vec<_>>() })
        }
        None => serde_json::json!({"installed": false, "menus": []}),
    })
}
