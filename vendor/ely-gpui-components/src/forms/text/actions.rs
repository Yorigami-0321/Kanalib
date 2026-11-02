use gpui::{
    App, ClipboardItem, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Render, Role, StatefulInteractiveElement,
    Styled, Window, actions, div, prelude::*,
};

use super::{
    InputEvent, TextInput,
    edit::{self, next_grapheme, prev_grapheme},
    element::TextElement,
};
use crate::forms::structure::FORM_CONTEXT;

actions!(
    ely_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Enter,
        Newline,
        Submit,
        ShowCharacterPalette,
    ]
);

const CONTEXT: &str = "ElyInput";

/// Binds the text keys. `init` calls it.
pub(crate) fn bind_keys(cx: &mut App) {
    let word = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    let mut bindings = vec![
        crate::key_binding("backspace", Backspace, Some(CONTEXT)),
        crate::key_binding("delete", Delete, Some(CONTEXT)),
        crate::key_binding(&format!("{word}-backspace"), DeleteWordLeft, Some(CONTEXT)),
        crate::key_binding("left", Left, Some(CONTEXT)),
        crate::key_binding("right", Right, Some(CONTEXT)),
        crate::key_binding("up", Up, Some(CONTEXT)),
        crate::key_binding("down", Down, Some(CONTEXT)),
        crate::key_binding("shift-left", SelectLeft, Some(CONTEXT)),
        crate::key_binding("shift-right", SelectRight, Some(CONTEXT)),
        crate::key_binding("shift-up", SelectUp, Some(CONTEXT)),
        crate::key_binding("shift-down", SelectDown, Some(CONTEXT)),
        crate::key_binding(&format!("{word}-left"), WordLeft, Some(CONTEXT)),
        crate::key_binding(&format!("{word}-right"), WordRight, Some(CONTEXT)),
        crate::key_binding(&format!("{word}-shift-left"), SelectWordLeft, Some(CONTEXT)),
        crate::key_binding(
            &format!("{word}-shift-right"),
            SelectWordRight,
            Some(CONTEXT),
        ),
        crate::key_binding("home", Home, Some(CONTEXT)),
        crate::key_binding("end", End, Some(CONTEXT)),
        crate::key_binding("shift-home", SelectHome, Some(CONTEXT)),
        crate::key_binding("shift-end", SelectEnd, Some(CONTEXT)),
        crate::key_binding("secondary-a", SelectAll, Some(CONTEXT)),
        crate::key_binding("secondary-c", Copy, Some(CONTEXT)),
        crate::key_binding("secondary-x", Cut, Some(CONTEXT)),
        crate::key_binding("secondary-z", Undo, Some(CONTEXT)),
        crate::key_binding("secondary-shift-z", Redo, Some(CONTEXT)),
        crate::key_binding("enter", Enter, Some(CONTEXT)),
        crate::key_binding("shift-enter", Newline, Some(CONTEXT)),
        crate::key_binding("secondary-enter", Submit, Some(CONTEXT)),
        crate::key_binding("secondary-enter", Submit, Some(FORM_CONTEXT)),
    ];
    // The browser's paste event reaches the input handler.
    if !cfg!(target_family = "wasm") {
        bindings.push(crate::key_binding("secondary-v", Paste, Some(CONTEXT)));
    }
    if cfg!(target_os = "macos") {
        bindings.extend([
            crate::key_binding("cmd-left", Home, Some(CONTEXT)),
            crate::key_binding("cmd-right", End, Some(CONTEXT)),
            crate::key_binding("cmd-shift-left", SelectHome, Some(CONTEXT)),
            crate::key_binding("cmd-shift-right", SelectEnd, Some(CONTEXT)),
            crate::key_binding("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        ]);
    } else {
        bindings.push(crate::key_binding("ctrl-y", Redo, Some(CONTEXT)));
    }
    cx.bind_keys(bindings);
}

impl TextInput {
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            prev_grapheme(&self.text, self.cursor())..self.cursor()
        } else {
            self.selection.clone()
        };
        self.replace(range, "", false, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.cursor()..next_grapheme(&self.text, self.cursor())
        } else {
            self.selection.clone()
        };
        self.replace(range, "", false, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            edit::prev_word(&self.text, self.cursor())..self.cursor()
        } else {
            self.selection.clone()
        };
        self.replace(range, "", false, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selection.is_empty() {
            prev_grapheme(&self.text, self.cursor())
        } else {
            self.selection.start
        };
        self.move_to(to, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selection.is_empty() {
            next_grapheme(&self.text, self.cursor())
        } else {
            self.selection.end
        };
        self.move_to(to, cx);
    }

    /// The offset one visual line above or below the caret.
    fn vertical(&self, down: bool) -> usize {
        let caret = self.cursor();
        let (Some(layout), Some(at)) = (
            self.layout.as_ref(),
            self.position_for(self.display_offset(caret)),
        ) else {
            return caret;
        };
        let step = if down {
            layout.line_height
        } else {
            -layout.line_height
        };
        let target = gpui::point(at.x, at.y + step + layout.line_height / 2.0);
        if target.y < gpui::Pixels::ZERO {
            return 0;
        }
        self.offset_for_point(layout.bounds.origin + target - self.scroll)
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(false);
        self.move_to(to, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.rows.is_some() {
            self.vertical(true)
        } else {
            self.text.len()
        };
        self.move_to(to, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(prev_grapheme(&self.text, self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_grapheme(&self.text, self.cursor()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(false);
        self.select_to(to, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(true);
        self.select_to(to, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(edit::prev_word(&self.text, self.cursor()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(edit::next_word(&self.text, self.cursor()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(edit::prev_word(&self.text, self.cursor()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(edit::next_word(&self.text, self.cursor()), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(edit::line_start(&self.text, self.cursor()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(edit::line_end(&self.text, self.cursor()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(edit::line_start(&self.text, self.cursor()), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(edit::line_end(&self.text, self.cursor()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = 0..self.text.len();
        self.reversed = false;
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.masked || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[self.selection.clone()].to_string(),
        ));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.masked || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[self.selection.clone()].to_string(),
        ));
        self.replace(self.selection.clone(), "", false, cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.replace(self.selection.clone(), &text, false, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(previous) = self.history.undo(self.snapshot()) {
            self.restore(previous, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(next) = self.history.redo(self.snapshot()) {
            self.restore(next, cx);
        }
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.rows.is_some() {
            self.replace(self.selection.clone(), "\n", false, cx);
        } else {
            log::info!("text input: submitted");
            cx.emit(InputEvent::Submit);
        }
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.rows.is_some() {
            self.replace(self.selection.clone(), "\n", false, cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        log::info!("text input: submitted");
        cx.emit(InputEvent::Submit);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus, cx);
        let at = self.offset_for_point(event.position);
        match event.click_count {
            2 => {
                let word = edit::word_at(&self.text, at);
                self.selection = word;
                self.reversed = false;
                cx.notify();
            }
            3.. => {
                self.selection = edit::line_start(&self.text, at)..edit::line_end(&self.text, at);
                self.reversed = false;
                cx.notify();
            }
            _ if event.modifiers.shift => self.select_to(at, cx),
            _ => {
                self.selecting = true;
                self.move_to(at, cx);
            }
        }
    }

    /// A right press keeps a selection it lands in.
    fn secondary_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus, cx);
        let at = self.offset_for_point(event.position);
        let held =
            !self.selection.is_empty() && (self.selection.start..=self.selection.end).contains(&at);
        if !held {
            self.move_to(at, cx);
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            let at = self.offset_for_point(event.position);
            self.select_to(at, cx);
        }
    }

    fn mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = window.is_window_active();
        if active != self.window_active {
            self.window_active = active;
            if active {
                self.caret_on = true;
            } else {
                self._blink = None;
            }
        }
        div()
            .id("ely-text-input")
            .role(Role::TextInput)
            .when_some(
                [&self.label, &self.placeholder]
                    .into_iter()
                    .find(|name| !name.is_empty())
                    .cloned(),
                |el, name| el.aria_label(name),
            )
            .when(!self.masked, |el| el.aria_value(self.aria_text()))
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(if self.disabled {
                CursorStyle::OperationNotAllowed
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::secondary_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(TextElement::new(cx.entity()))
    }
}
