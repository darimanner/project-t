//! Root view: host sidebar + terminal tabs, plus the add-host modal and the
//! Cmd+K palette. Owns the store (hosts DB) and routes overlay keyboard input.

use std::path::PathBuf;

use core_ssh::{Auth, SshConfig};
use core_store::{Host, Store, secrets};
use gpui::{
    App, AppContext, Context, Div, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, ParentElement, Render, Styled, Window,
    actions, div, px, rgb, rgba,
};

use crate::host_form::{Field, HostForm};
use crate::palette::{Palette, filter_hosts};
use crate::terminal_view::TerminalView;
use crate::text_input::render_input;

actions!(
    workspace,
    [NewTab, CloseTab, NextTab, PrevTab, TogglePalette]
);

const SIDEBAR_WIDTH: f32 = 200.0;

pub struct Workspace {
    tabs: Vec<Entity<TerminalView>>,
    active: usize,
    store: Option<Store>,
    store_error: Option<String>,
    hosts: Vec<Host>,
    form: Option<HostForm>,
    palette: Option<Palette>,
    /// Focus target while a modal overlay (form/palette) is open.
    overlay_focus: FocusHandle,
}

fn store_path() -> Option<PathBuf> {
    let dir = std::env::home_dir()?.join("Library/Application Support/project-t");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("store.db"))
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| TerminalView::new_local(window, cx));
        let (store, store_error) = match store_path().map(Store::open) {
            Some(Ok(store)) => (Some(store), None),
            Some(Err(err)) => (None, Some(format!("store: {err}"))),
            None => (None, Some("store: no home directory".to_owned())),
        };
        let mut ws = Self {
            tabs: vec![first],
            active: 0,
            store,
            store_error,
            hosts: Vec::new(),
            form: None,
            palette: None,
            overlay_focus: cx.focus_handle(),
        };
        ws.refresh_hosts();
        ws
    }

    fn refresh_hosts(&mut self) {
        if let Some(store) = &self.store {
            match store.list_hosts() {
                Ok(hosts) => self.hosts = hosts,
                Err(err) => self.store_error = Some(format!("store: {err}")),
            }
        }
    }

    // ---- tabs ----------------------------------------------------------

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let handle = self.tabs[self.active].read(cx).focus_handle(cx);
        window.focus(&handle);
    }

    fn push_tab(&mut self, tab: Entity<TerminalView>, cx: &mut Context<Self>) {
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        cx.notify();
    }

    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = cx.new(|cx| TerminalView::new_local(window, cx));
        self.push_tab(tab, cx);
    }

    fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep at least one tab open; closing the last one is a no-op.
        if self.tabs.len() == 1 {
            return;
        }
        let closed = self.tabs.remove(self.active);
        closed.read(cx).kill();
        self.active = self.active.min(self.tabs.len() - 1);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.active = index;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.active + 1) % self.tabs.len();
        self.activate(next, window, cx);
    }

    fn prev_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
        self.activate(prev, window, cx);
    }

    // ---- SSH -----------------------------------------------------------

    fn open_ssh_tab(&mut self, host_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.hosts.get(host_index).cloned() else {
            return;
        };
        let auth = match host.auth {
            core_store::AuthKind::Key => Auth::Key {
                path: host.key_path.clone().unwrap_or_default().into(),
                passphrase: None,
            },
            core_store::AuthKind::Password => match secrets::get_password(host.id) {
                Ok(Some(pw)) => Auth::Password(pw),
                Ok(None) => {
                    let tab = cx.new(|cx| {
                        TerminalView::new_failed(
                            format!("no password stored for {}", host.label),
                            window,
                            cx,
                        )
                    });
                    self.push_tab(tab, cx);
                    return;
                }
                Err(err) => {
                    let tab = cx
                        .new(|cx| TerminalView::new_failed(format!("keychain: {err}"), window, cx));
                    self.push_tab(tab, cx);
                    return;
                }
            },
        };
        let config = SshConfig {
            host: host.host.clone(),
            port: host.port,
            user: host.username.clone(),
            auth,
            size: core_ssh::Size {
                rows: 30,
                cols: 100,
            },
        };
        let label = host.label.clone();
        let tab = cx.new(|cx| TerminalView::new_ssh(label, config, window, cx));
        self.push_tab(tab, cx);
    }

    fn delete_host(&mut self, host_index: usize, cx: &mut Context<Self>) {
        let Some(host) = self.hosts.get(host_index) else {
            return;
        };
        let id = host.id;
        if let Some(store) = &self.store {
            if let Err(err) = store.delete_host(id) {
                self.store_error = Some(format!("store: {err}"));
                return;
            }
            if let Err(err) = secrets::delete_password(id) {
                eprintln!("keychain delete failed: {err}");
            }
            self.refresh_hosts();
            cx.notify();
        }
    }

    // ---- overlays ------------------------------------------------------

    fn open_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = Some(HostForm::default());
        self.palette = None;
        window.focus(&self.overlay_focus);
        cx.notify();
    }

    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self.palette = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_overlays(window, cx);
        } else {
            self.palette = Some(Palette::default());
            self.form = None;
            window.focus(&self.overlay_focus);
            cx.notify();
        }
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        match form.validate() {
            Ok(validated) => {
                let Some(store) = &self.store else {
                    form.error = Some("host store unavailable".to_owned());
                    cx.notify();
                    return;
                };
                match store.add_host(&validated.host) {
                    Ok(id) => {
                        if let Some(pw) = &validated.password
                            && let Err(err) = secrets::set_password(id, pw)
                        {
                            form.error = Some(format!("keychain: {err}"));
                            cx.notify();
                            return;
                        }
                        self.refresh_hosts();
                        self.close_overlays(window, cx);
                    }
                    Err(err) => {
                        form.error = Some(format!("store: {err}"));
                        cx.notify();
                    }
                }
            }
            Err(msg) => {
                form.error = Some(msg);
                cx.notify();
            }
        }
    }

    /// Keyboard input while an overlay is focused.
    fn on_overlay_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keystroke.key == "escape" {
            self.close_overlays(window, cx);
            return;
        }
        if self.form.is_some() {
            self.on_form_key(keystroke, window, cx);
        } else if self.palette.is_some() {
            self.on_palette_key(keystroke, window, cx);
        }
    }

    fn on_form_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        match keystroke.key.as_str() {
            "enter" => {
                self.submit_form(window, cx);
                return;
            }
            "tab" => {
                if keystroke.modifiers.shift {
                    form.focus_prev();
                } else {
                    form.focus_next();
                }
            }
            "space" | "left" | "right" if form.focus == Field::AuthToggle => form.toggle_auth(),
            _ => {
                if let Some(input) = form.focused_input() {
                    input.on_key(keystroke);
                }
            }
        }
        cx.notify();
    }

    fn on_palette_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = &mut self.palette else {
            return;
        };
        let filtered = filter_hosts(&self.hosts, palette.input.text());
        match keystroke.key.as_str() {
            "up" => palette.move_selection(-1, filtered.len()),
            "down" => palette.move_selection(1, filtered.len()),
            "enter" => {
                if let Some(&host_index) = filtered.get(palette.selected) {
                    self.close_overlays(window, cx);
                    self.open_ssh_tab(host_index, window, cx);
                    return;
                }
            }
            _ => {
                if palette.input.on_key(keystroke) {
                    palette.clamp_selection(filter_hosts(&self.hosts, palette.input.text()).len());
                }
            }
        }
        cx.notify();
    }

    // ---- rendering -----------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let mut rows = Vec::with_capacity(self.hosts.len());
        for (i, host) in self.hosts.iter().enumerate() {
            rows.push(
                div()
                    .id(("host", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(0xb8b8b8))
                    .hover(|s| s.bg(rgb(0x1c1c22)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.open_ssh_tab(i, window, cx);
                        }),
                    )
                    .child(
                        div().flex().flex_col().child(host.label.clone()).child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x666666))
                                .child(format!("{}@{}", host.username, host.host)),
                        ),
                    )
                    .child(
                        div()
                            .id(("host-del", i))
                            .px_1()
                            .rounded_sm()
                            .text_color(rgb(0x666666))
                            .hover(|s| s.bg(rgb(0x3a2a2a)).text_color(rgb(0xcc6666)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.delete_host(i, cx);
                                }),
                            )
                            .child("×"),
                    ),
            );
        }

        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .bg(rgb(0x0d0d10))
            .px_1()
            .py_2()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(0x666666))
                    .child("HOSTS"),
            )
            .children(rows);

        if let Some(err) = &self.store_error {
            sidebar = sidebar.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(0xcc6666))
                    .child(err.clone()),
            );
        }

        sidebar.child(
            div()
                .id("add-host")
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(0x888888))
                .hover(|s| s.bg(rgb(0x1c1c22)).text_color(rgb(0xd8d8d8)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.open_form(window, cx);
                    }),
                )
                .child("+ add host"),
        )
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tab_items = Vec::with_capacity(self.tabs.len());
        for i in 0..self.tabs.len() {
            let is_active = i == self.active;
            tab_items.push(
                div()
                    .id(("tab", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h_full()
                    .cursor_pointer()
                    .bg(if is_active {
                        rgb(0x1c1c22)
                    } else {
                        rgb(0x121216)
                    })
                    .text_color(if is_active {
                        rgb(0xd8d8d8)
                    } else {
                        rgb(0x888888)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.activate(i, window, cx);
                        }),
                    )
                    .child(format!("Terminal {}", i + 1))
                    .child(
                        div()
                            .id(("close", i))
                            .px_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgb(0x3a3a44)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    // Do not also activate the tab underneath.
                                    cx.stop_propagation();
                                    this.activate(i, window, cx);
                                    this.close_tab(window, cx);
                                }),
                            )
                            .child("×"),
                    ),
            );
        }

        div()
            .flex()
            .flex_row()
            .h(px(32.))
            .bg(rgb(0x0d0d10))
            .children(tab_items)
            .child(
                div()
                    .id("new-tab")
                    .flex()
                    .items_center()
                    .px_3()
                    .h_full()
                    .cursor_pointer()
                    .text_color(rgb(0x888888))
                    .hover(|s| s.text_color(rgb(0xd8d8d8)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.new_tab(window, cx);
                        }),
                    )
                    .child("+"),
            )
    }

    fn render_form(&self, form: &HostForm, cx: &mut Context<Self>) -> Div {
        let field_row = |title: &str, input: Div| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child(title.to_owned()),
                )
                .child(input)
        };

        // Click focuses the field (Tab still cycles). Propagation stops so the
        // backdrop's close-on-click never sees clicks inside the panel.
        fn click_focus(
            field: Field,
            cx: &mut Context<Workspace>,
        ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
            cx.listener(
                move |this: &mut Workspace, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    if let Some(form) = &mut this.form {
                        form.focus = field;
                        cx.notify();
                    }
                },
            )
        }
        let input_field = |state, placeholder, field: Field, cx: &mut Context<Self>| -> Div {
            render_input(state, placeholder, form.focus == field)
                .on_mouse_down(MouseButton::Left, click_focus(field, cx))
        };

        let auth_chip =
            |name: &str, active: bool, auth: core_store::AuthKind, cx: &mut Context<Self>| {
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if active { rgb(0x2a3a55) } else { rgb(0x1a1a20) })
                    .text_color(if active { rgb(0xd8d8d8) } else { rgb(0x777777) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this: &mut Self, _: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            if let Some(form) = &mut this.form {
                                form.focus = Field::AuthToggle;
                                form.set_auth(auth);
                                cx.notify();
                            }
                        }),
                    )
                    .child(name.to_owned())
            };
        let auth_focused = form.focus == Field::AuthToggle;
        let is_password = form.auth == core_store::AuthKind::Password;
        let auth_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(if auth_focused {
                rgb(0x5588cc)
            } else {
                rgb(0x33333c)
            })
            .on_mouse_down(MouseButton::Left, click_focus(Field::AuthToggle, cx))
            .child(auth_chip(
                "password",
                is_password,
                core_store::AuthKind::Password,
                cx,
            ))
            .child(auth_chip(
                "key",
                !is_password,
                core_store::AuthKind::Key,
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child("space to switch"),
            );

        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(360.))
            .p_4()
            .rounded_md()
            .bg(rgb(0x16161c))
            .border_1()
            .border_color(rgb(0x33333c))
            // Swallow clicks that land on the panel but not on any field.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| cx.stop_propagation()),
            )
            .child(div().text_color(rgb(0xd8d8d8)).child("Add host"))
            .child(field_row(
                "label",
                input_field(&form.label, "prod", Field::Label, cx),
            ))
            .child(field_row(
                "host",
                input_field(&form.host, "1.2.3.4", Field::Host, cx),
            ))
            .child(field_row(
                "port",
                input_field(&form.port, "22", Field::Port, cx),
            ))
            .child(field_row(
                "user",
                input_field(&form.user, "root", Field::User, cx),
            ))
            .child(field_row("auth", auth_row))
            .child(field_row(
                if is_password { "password" } else { "key path" },
                input_field(
                    &form.secret,
                    if is_password {
                        "••••"
                    } else {
                        "~/.ssh/id_ed25519"
                    },
                    Field::Secret,
                    cx,
                ),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x555555))
                    .child("tab: next field · enter: save · esc: cancel"),
            );

        if let Some(err) = &form.error {
            panel = panel.child(div().text_xs().text_color(rgb(0xcc6666)).child(err.clone()));
        }
        panel
    }

    fn render_palette(&self, palette: &Palette, cx: &mut Context<Self>) -> Div {
        let filtered = filter_hosts(&self.hosts, palette.input.text());
        let mut list = div().flex().flex_col().gap_1();
        if filtered.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(rgb(0x666666))
                    .child("no matching hosts"),
            );
        }
        for (pos, &host_index) in filtered.iter().enumerate() {
            let host = &self.hosts[host_index];
            let selected = pos == palette.selected;
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if selected {
                        rgb(0x2a3a55)
                    } else {
                        rgb(0x16161c)
                    })
                    .text_color(if selected {
                        rgb(0xd8d8d8)
                    } else {
                        rgb(0x999999)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.close_overlays(window, cx);
                            this.open_ssh_tab(host_index, window, cx);
                        }),
                    )
                    .child(host.label.clone())
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x666666))
                            .child(format!("{}@{}:{}", host.username, host.host, host.port)),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(420.))
            .p_3()
            .rounded_md()
            .bg(rgb(0x16161c))
            .border_1()
            .border_color(rgb(0x33333c))
            // Swallow clicks inside the panel so the backdrop doesn't close it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _window, cx| cx.stop_propagation()),
            )
            .child(render_input(&palette.input, "connect to host…", true))
            .child(list)
    }

    /// Dimmed full-window layer that centers a modal panel and owns focus.
    fn render_overlay(&self, panel: Div, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .track_focus(&self.overlay_focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.on_overlay_key(&ev.keystroke, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.close_overlays(window, cx);
                }),
            )
            .child(panel)
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_tab(window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.next_tab(window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.prev_tab(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePalette, window, cx| this.toggle_palette(window, cx)),
            )
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0x121216))
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(self.render_tab_bar(cx))
                    .child(div().flex_1().child(self.tabs[self.active].clone())),
            );

        if let Some(form) = &self.form {
            let panel = self.render_form(form, cx);
            root = root.child(self.render_overlay(panel, cx));
        } else if let Some(palette) = &self.palette {
            let panel = self.render_palette(palette, cx);
            root = root.child(self.render_overlay(panel, cx));
        }
        root
    }
}
