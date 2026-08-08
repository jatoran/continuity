//! Native input translation for the embeddable child surface.

use continuity_engine::SelectionEdit;
use continuity_host::{
    CompositionIntent, EditorIntent, EditorOperation, FocusIntent, HostRequest, OperationRequest,
    PointerButton, PointerIntent, PointerPhase, ScrollIntent, SelectionIntent,
};
use continuity_text::{Position, Selection, SelectionKind};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_A, VK_BACK, VK_C, VK_CONTROL, VK_DELETE,
    VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_MENU, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP, VK_V, VK_Y,
    VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::{GetNextDlgTabItem, WHEEL_DELTA};

use super::{ControlClipboardMode, TabBehavior};
use crate::editor_control::state::EditorControlState;
use crate::Error;

impl EditorControlState {
    pub(super) fn insert_text(&mut self, text: String) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        self.dispatch_operation(EditorOperation::ApplySelectionEdit(
            SelectionEdit::InsertText(text),
        ))
    }

    pub(super) fn handle_char(&mut self, value: u16) -> Result<(), Error> {
        if self.surface.ime.composing || matches!(value, 8 | 9 | 10 | 13 | 27) {
            return Ok(());
        }
        if (0xD800..=0xDBFF).contains(&value) {
            self.pending_high_surrogate = Some(value);
            return Ok(());
        }
        let units = self
            .pending_high_surrogate
            .take()
            .map_or_else(|| vec![value], |high| vec![high, value]);
        let text = char::decode_utf16(units)
            .filter_map(Result::ok)
            .collect::<String>();
        self.insert_text(text)
    }

    pub(super) fn handle_key_down(&mut self, key: u16) -> Result<bool, Error> {
        let control = is_key_down(VK_CONTROL.0);
        let shift = is_key_down(VK_SHIFT.0);
        let handled = if control {
            match key {
                value if value == VK_Z.0 && shift => {
                    self.dispatch_operation(EditorOperation::Redo)?;
                    true
                }
                value if value == VK_Z.0 => {
                    self.dispatch_operation(EditorOperation::Undo)?;
                    true
                }
                value if value == VK_Y.0 => {
                    self.dispatch_operation(EditorOperation::Redo)?;
                    true
                }
                value if value == VK_A.0 => {
                    self.select_all()?;
                    true
                }
                value if value == VK_C.0 => {
                    self.copy_selection()?;
                    true
                }
                value if value == VK_V.0 => {
                    self.paste()?;
                    true
                }
                _ => false,
            }
        } else {
            match key {
                value if value == VK_BACK.0 => {
                    self.dispatch_selection_edit(SelectionEdit::DeleteBack)?;
                    true
                }
                value if value == VK_DELETE.0 => {
                    self.dispatch_selection_edit(SelectionEdit::DeleteForward)?;
                    true
                }
                0x0D => {
                    self.dispatch_selection_edit(SelectionEdit::InsertNewlineSmart)?;
                    true
                }
                value if value == VK_TAB.0 => {
                    self.handle_tab(shift)?;
                    true
                }
                value if value == VK_LEFT.0 => {
                    self.move_selections(MoveDirection::Left, shift)?;
                    true
                }
                value if value == VK_RIGHT.0 => {
                    self.move_selections(MoveDirection::Right, shift)?;
                    true
                }
                value if value == VK_UP.0 => {
                    self.move_selections(MoveDirection::Up, shift)?;
                    true
                }
                value if value == VK_DOWN.0 => {
                    self.move_selections(MoveDirection::Down, shift)?;
                    true
                }
                value if value == VK_HOME.0 => {
                    self.move_selections(MoveDirection::Home, shift)?;
                    true
                }
                value if value == VK_END.0 => {
                    self.move_selections(MoveDirection::End, shift)?;
                    true
                }
                _ => false,
            }
        };
        Ok(handled)
    }

    fn dispatch_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
        self.dispatch_operation(EditorOperation::ApplySelectionEdit(edit))
    }

    pub(super) fn dispatch_operation(&mut self, operation: EditorOperation) -> Result<(), Error> {
        let revision = self.runtime.revision(self.buffer_id)?;
        self.dispatch(EditorIntent::Operation(OperationRequest {
            buffer_id: self.buffer_id,
            expected_revision: Some(revision),
            timestamp_ms: wall_clock_ms(),
            operation,
        }))
    }

    fn handle_tab(&mut self, backwards: bool) -> Result<(), Error> {
        match self.options.tab_behavior {
            TabBehavior::InsertIndent => self.insert_text("\t".to_owned()),
            TabBehavior::TraverseHost => {
                let next = unsafe { GetNextDlgTabItem(self.parent, Some(self.hwnd), backwards) }?;
                if !next.is_invalid() && next != self.hwnd {
                    unsafe {
                        let _ = SetFocus(Some(next));
                    }
                }
                Ok(())
            }
        }
    }

    fn select_all(&mut self) -> Result<(), Error> {
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let end =
            Position::from_byte_offset(snapshot.rope.rope(), snapshot.rope.rope().len_bytes())
                .map_err(|error| continuity_host::Error::Engine(error.into()))?;
        self.dispatch(EditorIntent::Select(SelectionIntent {
            buffer_id: self.buffer_id,
            selections: vec![Selection::new(Position::ZERO, end, SelectionKind::Caret)],
        }))
    }

    fn copy_selection(&mut self) -> Result<(), Error> {
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let rope = snapshot.rope.rope();
        let text = snapshot
            .selections
            .iter()
            .filter_map(|selection| {
                let range = selection.ordered_range();
                let start = position_to_byte(rope, range.start)?;
                let end = position_to_byte(rope, range.end)?;
                (start != end).then(|| rope.byte_slice(start..end).to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            return Ok(());
        }
        match self.options.clipboard {
            ControlClipboardMode::Native => {
                continuity_win::clipboard::write_text(self.hwnd, &text)?;
                Ok(())
            }
            ControlClipboardMode::HostMediated => {
                self.dispatch(EditorIntent::Request(HostRequest::WriteClipboard(text)))
            }
        }
    }

    fn paste(&mut self) -> Result<(), Error> {
        match self.options.clipboard {
            ControlClipboardMode::Native => {
                if let Some(text) = continuity_win::clipboard::read_text(self.hwnd)? {
                    self.insert_text(text)?;
                }
                Ok(())
            }
            ControlClipboardMode::HostMediated => {
                self.dispatch(EditorIntent::Request(HostRequest::ReadClipboard))
            }
        }
    }

    fn move_selections(&mut self, direction: MoveDirection, extend: bool) -> Result<(), Error> {
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let rope = snapshot.rope.rope();
        let selections = snapshot
            .selections
            .into_iter()
            .map(|selection| move_selection(rope, selection, direction, extend))
            .collect();
        self.dispatch(EditorIntent::Select(SelectionIntent {
            buffer_id: self.buffer_id,
            selections,
        }))
    }

    pub(super) fn handle_focus(&mut self, has_focus: bool) -> Result<(), Error> {
        self.surface.focus.has_keyboard_focus = has_focus;
        self.dispatch(EditorIntent::Focus(if has_focus {
            FocusIntent::Gained
        } else {
            FocusIntent::Lost
        }))
    }

    pub(super) fn handle_wheel(&mut self, wparam: WPARAM) -> Result<(), Error> {
        let delta = ((wparam.0 >> 16) as u16) as i16 as f32;
        let line_height = self.options.font_size_dip * 1.45;
        let delta_y = -(delta / WHEEL_DELTA as f32) * line_height * 3.0;
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let content_height = snapshot.rope.rope().len_lines() as f32 * line_height;
        self.surface
            .view
            .scroll_instant(delta_y, content_height.max(line_height));
        self.dispatch(EditorIntent::Scroll(ScrollIntent {
            delta_x_dip: 0.0,
            delta_y_dip: delta_y,
            is_inertial: false,
        }))
    }

    pub(super) fn handle_pointer(
        &mut self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<(), Error> {
        let (x, y) = packed_point(lparam);
        let scale = self.scale();
        let x_dip = x as f32 / scale;
        let y_dip = y as f32 / scale;
        let (phase, button, clicks) = match message {
            windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN => {
                (PointerPhase::Down, PointerButton::Primary, 1)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDBLCLK => {
                (PointerPhase::Down, PointerButton::Primary, 2)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                (PointerPhase::Up, PointerButton::Primary, 1)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE => {
                (PointerPhase::Move, PointerButton::None, 0)
            }
            _ => (PointerPhase::Leave, PointerButton::None, 0),
        };
        let intent = PointerIntent {
            x_dip,
            y_dip,
            button,
            phase,
            click_count: clicks,
            is_primary_down: wparam.0 & 0x0001 != 0,
            is_secondary_down: wparam.0 & 0x0002 != 0,
            is_middle_down: wparam.0 & 0x0010 != 0,
            is_shift_down: wparam.0 & 0x0004 != 0,
            is_control_down: wparam.0 & 0x0008 != 0,
            is_alt_down: is_key_down(VK_MENU.0),
        };
        self.dispatch(EditorIntent::Pointer(intent))?;
        match phase {
            PointerPhase::Down if button == PointerButton::Primary => {
                let position = self.hit_test_position(x_dip, y_dip)?;
                self.drag_anchor = Some(position);
                self.dispatch(EditorIntent::Select(SelectionIntent {
                    buffer_id: self.buffer_id,
                    selections: vec![Selection::caret_at(position)],
                }))?;
                unsafe {
                    let _ = SetCapture(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            PointerPhase::Move if intent.is_primary_down => {
                if let Some(anchor) = self.drag_anchor {
                    let head = self.hit_test_position(x_dip, y_dip)?;
                    self.dispatch(EditorIntent::Select(SelectionIntent {
                        buffer_id: self.buffer_id,
                        selections: vec![Selection::new(anchor, head, SelectionKind::Caret)],
                    }))?;
                }
            }
            PointerPhase::Up => {
                self.drag_anchor = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn hit_test_position(&self, x_dip: f32, y_dip: f32) -> Result<Position, Error> {
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let rope = snapshot.rope.rope();
        let line_height = self.options.font_size_dip * 1.45;
        let display_row = ((y_dip + self.surface.view.scroll_y_dip) / line_height)
            .floor()
            .max(0.0) as usize;
        if let Some((_, frame)) = self.surface.projection.last_painted_frame_display.as_ref() {
            if let Some(spec) = frame.display_line_by_index(display_row as u32) {
                let approximate = (x_dip / (self.options.font_size_dip * 0.55)).round() as usize;
                let display_byte = spec
                    .display_text()
                    .char_indices()
                    .nth(approximate)
                    .map_or(spec.display_text().len(), |(byte, _)| byte);
                if let Some(source_byte) = spec.display_to_source(
                    continuity_display_map::DisplayByte::from_usize(display_byte),
                ) {
                    let line = spec.source_line.as_usize();
                    let line_start = rope.line_to_byte(line);
                    return Ok(Position {
                        line: line as u32,
                        byte_in_line: source_byte.raw().saturating_sub(line_start as u32),
                    });
                }
            }
        }
        let line = display_row.min(rope.len_lines().saturating_sub(1));
        let slice = rope.line(line);
        let content = slice.to_string();
        let approximate = (x_dip / (self.options.font_size_dip * 0.55)).round() as usize;
        let mut byte = content
            .char_indices()
            .nth(approximate)
            .map_or(content.len(), |(byte, _)| byte);
        byte = byte.min(content.trim_end_matches(['\r', '\n']).len());
        Ok(Position {
            line: line as u32,
            byte_in_line: byte as u32,
        })
    }

    pub(super) fn handle_ime_start(&mut self) -> Result<(), Error> {
        self.surface.ime.composing = true;
        self.surface.ime.comp.clear();
        self.surface.ime.caret_byte = 0;
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let position = snapshot
            .selections
            .first()
            .map_or(Position::ZERO, |selection| selection.head);
        self.dispatch(EditorIntent::Composition(CompositionIntent::Start {
            position,
        }))
    }

    pub(super) fn handle_ime_composition(
        &mut self,
        hwnd: HWND,
        lparam: LPARAM,
    ) -> Result<(), Error> {
        let Some(composition) = continuity_win::ime::read_composition(hwnd, lparam.0) else {
            return Ok(());
        };
        self.surface.ime.comp = composition.comp.clone();
        self.surface.ime.caret_byte = composition.caret_byte;
        let caret_utf16 = composition.comp[..composition.caret_byte.min(composition.comp.len())]
            .encode_utf16()
            .count() as u32;
        self.dispatch(EditorIntent::Composition(CompositionIntent::Update {
            text: composition.comp,
            selection_utf16: caret_utf16..caret_utf16,
        }))?;
        if !composition.result.is_empty() {
            self.dispatch(EditorIntent::Composition(CompositionIntent::Commit(
                composition.result.clone(),
            )))?;
            self.insert_text(composition.result)?;
        }
        self.update_ime_position(hwnd)
    }

    pub(super) fn handle_ime_end(&mut self) -> Result<(), Error> {
        self.surface.ime.clear();
        self.dispatch(EditorIntent::Composition(CompositionIntent::Cancel))
    }

    fn update_ime_position(&self, hwnd: HWND) -> Result<(), Error> {
        let snapshot = self.runtime.snapshot(self.buffer_id)?;
        let position = snapshot
            .selections
            .first()
            .map_or(Position::ZERO, |selection| selection.head);
        let scale = self.scale();
        let x = position.byte_in_line as f32 * self.options.font_size_dip * 0.55;
        let y = (position.line as f32 + 1.0) * self.options.font_size_dip * 1.45
            - self.surface.view.scroll_y_dip;
        continuity_win::ime::set_composition_position(
            hwnd,
            (x * scale).round() as i32,
            (y * scale).round() as i32,
        );
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum MoveDirection {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

fn move_selection(
    rope: &ropey::Rope,
    selection: Selection,
    direction: MoveDirection,
    extend: bool,
) -> Selection {
    let ordered = selection.ordered_range();
    let head = if !extend && !selection.is_caret() {
        match direction {
            MoveDirection::Left | MoveDirection::Up | MoveDirection::Home => ordered.start,
            _ => ordered.end,
        }
    } else {
        move_position(rope, selection.head, direction)
    };
    if extend {
        Selection::new(selection.anchor, head, selection.kind)
    } else {
        Selection::caret_at(head)
    }
}

fn move_position(rope: &ropey::Rope, position: Position, direction: MoveDirection) -> Position {
    let line = (position.line as usize).min(rope.len_lines().saturating_sub(1));
    let line_text = rope.line(line).to_string();
    let line_content = line_text.trim_end_matches(['\r', '\n']);
    match direction {
        MoveDirection::Home => Position {
            line: line as u32,
            byte_in_line: 0,
        },
        MoveDirection::End => Position {
            line: line as u32,
            byte_in_line: line_content.len() as u32,
        },
        MoveDirection::Up | MoveDirection::Down => {
            let target_line = if matches!(direction, MoveDirection::Up) {
                line.saturating_sub(1)
            } else {
                (line + 1).min(rope.len_lines().saturating_sub(1))
            };
            let target = rope.line(target_line).to_string();
            let target = target.trim_end_matches(['\r', '\n']);
            Position {
                line: target_line as u32,
                byte_in_line: clamp_boundary(target, position.byte_in_line as usize) as u32,
            }
        }
        MoveDirection::Left | MoveDirection::Right => {
            let absolute = position_to_byte(rope, position).unwrap_or(0);
            let target = if matches!(direction, MoveDirection::Left) {
                previous_boundary(rope, absolute)
            } else {
                next_boundary(rope, absolute)
            };
            Position::from_byte_offset(rope, target).unwrap_or(position)
        }
    }
}

fn previous_boundary(rope: &ropey::Rope, byte: usize) -> usize {
    if byte == 0 {
        return 0;
    }
    let text = rope.to_string();
    text[..byte.min(text.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(rope: &ropey::Rope, byte: usize) -> usize {
    let text = rope.to_string();
    let byte = byte.min(text.len());
    text[byte..]
        .chars()
        .next()
        .map_or(byte, |character| byte + character.len_utf8())
}

fn clamp_boundary(text: &str, wanted: usize) -> usize {
    let mut byte = wanted.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn position_to_byte(rope: &ropey::Rope, position: Position) -> Option<usize> {
    let line = position.line as usize;
    (line < rope.len_lines()).then(|| rope.line_to_byte(line) + position.byte_in_line as usize)
}

fn packed_point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 as i16) as i32;
    let y = ((lparam.0 >> 16) as i16) as i32;
    (x, y)
}

fn is_key_down(key: u16) -> bool {
    unsafe { GetKeyState(key as i32) < 0 }
}

fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
