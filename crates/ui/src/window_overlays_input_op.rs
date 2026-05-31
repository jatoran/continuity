//! Text-input edit ops shared by every text-bearing overlay (find
//! bar, palette, slash palette, …). Split out of
//! [`crate::window_overlays`] so that file stays under the 600-line
//! cap once H5 / H6 routing landed.

#[derive(Copy, Clone)]
pub(crate) enum InputOp {
    DeleteBack,
    DeleteForward,
    Caret(i32),
    CaretHome,
    CaretEnd,
}

#[derive(Default)]
pub(crate) struct InputOpEffect {
    pub(crate) matches: bool,
    pub(crate) find_in_all: bool,
}

pub(crate) fn apply_input_op(input: &mut crate::text_input::TextInput, op: InputOp) -> bool {
    match op {
        InputOp::DeleteBack => input.delete_back(),
        InputOp::DeleteForward => input.delete_forward(),
        InputOp::Caret(d) => {
            if d < 0 {
                input.move_left()
            } else {
                input.move_right()
            }
        }
        InputOp::CaretHome => {
            input.move_home();
            false
        }
        InputOp::CaretEnd => {
            input.move_end();
            false
        }
    }
}
