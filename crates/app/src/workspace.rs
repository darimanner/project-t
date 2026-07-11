//! Root view: owns terminal tabs, renders the tab bar plus the active session.

use gpui::{
    App, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, Styled, Window, actions, div, px, rgb,
};

use crate::terminal_view::TerminalView;

actions!(workspace, [NewTab, CloseTab, NextTab, PrevTab]);

pub struct Workspace {
    tabs: Vec<Entity<TerminalView>>,
    active: usize,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| TerminalView::new(window, cx));
        Self {
            tabs: vec![first],
            active: 0,
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let handle = self.tabs[self.active].read(cx).focus_handle(cx);
        window.focus(&handle);
    }

    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = cx.new(|cx| TerminalView::new(window, cx));
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        cx.notify();
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
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_tab(window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.next_tab(window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.prev_tab(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x121216))
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().child(self.tabs[self.active].clone()))
    }
}
