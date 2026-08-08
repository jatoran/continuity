//! Native UI Automation adapter for the reusable editor surface.

#[cfg(test)]
mod tests;
mod text_range;

use std::sync::{Arc, Mutex};

use windows::core::{implement, IUnknown, Interface, Ref};
use windows::Win32::Foundation::{BOOL, E_NOINTERFACE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, IRawElementProviderSimple_Impl, ITextProvider2, ITextProvider2_Impl,
    ITextProvider_Impl, ITextRangeProvider, ProviderOptions, ProviderOptions_ServerSideProvider,
    ProviderOptions_UseComThreading, SupportedTextSelection, SupportedTextSelection_Multiple,
    UIA_AutomationFocusChangedEventId, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
    UIA_HasKeyboardFocusPropertyId, UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId,
    UIA_TextPattern2Id, UIA_TextPatternId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId, UiaHostProviderFromHwnd, UiaPoint,
    UiaRaiseAutomationEvent, UiaReturnRawElementProvider, UiaRootObjectId, UIA_PATTERN_ID,
    UIA_PROPERTY_ID,
};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_APP};

use crate::editor_surface::accessibility::{AccessibilityChange, AccessibilitySnapshot};
use crate::Window;
use text_range::{
    create_provider_array, document_length_utf16, position_to_utf16, utf16_to_position, TextRange,
};

pub(crate) const ACCESSIBILITY_SELECTION_MESSAGE: u32 = WM_APP + 1;

#[derive(Clone, Copy)]
pub(crate) enum AccessibilitySelectionAction {
    Replace,
    Add,
    Remove,
}

pub(crate) struct AccessibilitySelectionRequest {
    pub(crate) start_utf16: usize,
    pub(crate) end_utf16: usize,
    pub(crate) action: AccessibilitySelectionAction,
}

pub(super) fn send_selection_request(
    hwnd: HWND,
    start_utf16: usize,
    end_utf16: usize,
    action: AccessibilitySelectionAction,
) -> windows::core::Result<()> {
    let request = AccessibilitySelectionRequest {
        start_utf16,
        end_utf16,
        action,
    };
    let result = unsafe {
        SendMessageW(
            hwnd,
            ACCESSIBILITY_SELECTION_MESSAGE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (&request as *const AccessibilitySelectionRequest) as isize,
            )),
        )
    };
    if result.0 == 1 {
        Ok(())
    } else {
        Err(text_range::not_supported())
    }
}

#[implement(IRawElementProviderSimple, ITextProvider2)]
struct NativeEditorProvider {
    hwnd: HWND,
    shared: Arc<Mutex<AccessibilitySnapshot>>,
}

pub(crate) fn create_native_editor_provider(
    hwnd: HWND,
    shared: Arc<Mutex<AccessibilitySnapshot>>,
) -> IRawElementProviderSimple {
    NativeEditorProvider { hwnd, shared }.into()
}

pub(crate) fn return_raw_element_provider(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    provider: &IRawElementProviderSimple,
) -> LRESULT {
    unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, provider) }
}

pub(crate) fn raise_accessibility_events(
    provider: Option<&IRawElementProviderSimple>,
    change: AccessibilityChange,
) {
    let Some(provider) = provider else {
        return;
    };
    unsafe {
        if change.was_text_changed {
            let _ = UiaRaiseAutomationEvent(provider, UIA_Text_TextChangedEventId);
        }
        if change.were_selections_changed {
            let _ = UiaRaiseAutomationEvent(provider, UIA_Text_TextSelectionChangedEventId);
        }
        if change.was_focus_changed {
            let _ = UiaRaiseAutomationEvent(provider, UIA_AutomationFocusChangedEventId);
        }
    }
}

pub(crate) fn selections_from_accessibility_request(
    rope: &continuity_buffer::RopeSnapshot,
    current: &[continuity_text::Selection],
    lparam: LPARAM,
) -> Option<Vec<continuity_text::Selection>> {
    let request = lparam.0 as *const AccessibilitySelectionRequest;
    let request = unsafe { request.as_ref() }?;
    let start = utf16_to_position(rope, request.start_utf16);
    let end = utf16_to_position(rope, request.end_utf16);
    let selection =
        continuity_text::Selection::new(start, end, continuity_text::SelectionKind::Caret);
    let mut selections = current.to_vec();
    match request.action {
        AccessibilitySelectionAction::Replace => selections = vec![selection],
        AccessibilitySelectionAction::Add => selections.push(selection),
        AccessibilitySelectionAction::Remove => {
            selections.retain(|candidate| candidate.ordered_range() != selection.ordered_range());
            if selections.is_empty() {
                selections.push(continuity_text::Selection::caret_at(start));
            }
        }
    }
    Some(selections)
}

impl NativeEditorProvider {
    fn snapshot(&self) -> AccessibilitySnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl NativeEditorProvider_Impl {
    fn create_range(&self, start: usize, end: usize) -> windows::core::Result<ITextRangeProvider> {
        let enclosing: IRawElementProviderSimple = unsafe { self.cast()? };
        Ok(TextRange::new(self.hwnd, Arc::clone(&self.shared), enclosing, start, end).into())
    }

    fn create_document_range(&self) -> windows::core::Result<ITextRangeProvider> {
        let snapshot = self.snapshot();
        let end = snapshot
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        self.create_range(0, end)
    }
}

impl IRawElementProviderSimple_Impl for NativeEditorProvider_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, patternid: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        if patternid == UIA_TextPatternId || patternid == UIA_TextPattern2Id {
            let provider: ITextProvider2 = unsafe { self.cast()? };
            provider.cast()
        } else {
            Err(windows::core::Error::from_hresult(E_NOINTERFACE))
        }
    }

    fn GetPropertyValue(&self, propertyid: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        let snapshot = self.snapshot();
        let value = if propertyid == UIA_ControlTypePropertyId {
            VARIANT::from(UIA_DocumentControlTypeId.0)
        } else if propertyid == UIA_NamePropertyId {
            VARIANT::from("Continuity editor")
        } else if propertyid == UIA_HasKeyboardFocusPropertyId {
            VARIANT::from(snapshot.has_keyboard_focus)
        } else if propertyid == UIA_IsKeyboardFocusablePropertyId
            || propertyid == UIA_IsEnabledPropertyId
        {
            VARIANT::from(snapshot.is_enabled)
        } else if propertyid == UIA_IsControlElementPropertyId
            || propertyid == UIA_IsContentElementPropertyId
        {
            VARIANT::from(true)
        } else {
            VARIANT::default()
        };
        Ok(value)
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        unsafe { UiaHostProviderFromHwnd(self.hwnd) }
    }
}

impl ITextProvider_Impl for NativeEditorProvider_Impl {
    fn GetSelection(&self) -> windows::core::Result<*mut SAFEARRAY> {
        let snapshot = self.snapshot();
        let mut ranges = Vec::new();
        if let Some(document) = snapshot.document {
            for selection in &document.selections {
                let start = position_to_utf16(&document.rope, selection.anchor);
                let end = position_to_utf16(&document.rope, selection.head);
                ranges.push(self.create_range(start.min(end), start.max(end))?);
            }
        }
        create_provider_array(&ranges)
    }

    fn GetVisibleRanges(&self) -> windows::core::Result<*mut SAFEARRAY> {
        create_provider_array(&[self.create_document_range()?])
    }

    fn RangeFromChild(
        &self,
        _childelement: Ref<'_, IRawElementProviderSimple>,
    ) -> windows::core::Result<ITextRangeProvider> {
        Err(text_range::not_supported())
    }

    fn RangeFromPoint(&self, _point: &UiaPoint) -> windows::core::Result<ITextRangeProvider> {
        let snapshot = self.snapshot();
        let caret = snapshot
            .document
            .as_ref()
            .and_then(|document| {
                document
                    .selections
                    .first()
                    .map(|selection| (document, selection))
            })
            .map_or(0, |(document, selection)| {
                position_to_utf16(&document.rope, selection.head)
            });
        self.create_range(caret, caret)
    }

    fn DocumentRange(&self) -> windows::core::Result<ITextRangeProvider> {
        self.create_document_range()
    }

    fn SupportedTextSelection(&self) -> windows::core::Result<SupportedTextSelection> {
        Ok(SupportedTextSelection_Multiple)
    }
}

impl ITextProvider2_Impl for NativeEditorProvider_Impl {
    fn RangeFromAnnotation(
        &self,
        _annotationelement: Ref<'_, IRawElementProviderSimple>,
    ) -> windows::core::Result<ITextRangeProvider> {
        Err(text_range::not_supported())
    }

    fn GetCaretRange(&self, isactive: *mut BOOL) -> windows::core::Result<ITextRangeProvider> {
        let snapshot = self.snapshot();
        if !isactive.is_null() {
            unsafe {
                isactive.write(BOOL::from(
                    snapshot.has_keyboard_focus && snapshot.is_enabled,
                ))
            };
        }
        let caret = snapshot
            .document
            .as_ref()
            .and_then(|document| {
                document
                    .selections
                    .first()
                    .map(|selection| (document, selection))
            })
            .map_or(0, |(document, selection)| {
                position_to_utf16(&document.rope, selection.head)
            });
        self.create_range(caret, caret)
    }
}

impl Window {
    pub(crate) fn publish_accessibility_snapshot(
        &mut self,
        snapshot: &continuity_core::EditorSnapshot,
    ) {
        if self.accessibility_provider.is_none() {
            return;
        }
        let change = self.surface.accessibility.publish(
            snapshot,
            unsafe { IsWindowEnabled(self.hwnd).as_bool() },
            self.surface.focus.has_keyboard_focus,
        );
        self.raise_accessibility_events(change);
    }

    pub(crate) fn publish_accessibility_from_core(&mut self) {
        if let Some(snapshot) = self.current_snapshot() {
            self.publish_accessibility_snapshot(&snapshot);
        }
    }

    pub(crate) fn handle_get_object(
        &mut self,
        hwnd: HWND,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if lparam.0 as i32 != UiaRootObjectId {
            return None;
        }
        self.accessibility_provider.get_or_insert_with(|| {
            create_native_editor_provider(hwnd, self.surface.accessibility.shared())
        });
        self.publish_accessibility_from_core();
        let provider = self
            .accessibility_provider
            .as_ref()
            .expect("invariant: accessibility provider was initialized");
        Some(return_raw_element_provider(hwnd, wparam, lparam, provider))
    }

    fn raise_accessibility_events(&self, change: AccessibilityChange) {
        raise_accessibility_events(self.accessibility_provider.as_ref(), change);
    }

    pub(crate) fn handle_accessibility_selection(&mut self, lparam: LPARAM) -> LRESULT {
        let Some(snapshot) = self.current_snapshot() else {
            return LRESULT(0);
        };
        let Some(selections) = selections_from_accessibility_request(
            snapshot.rope_snapshot(),
            &snapshot.selections,
            lparam,
        ) else {
            return LRESULT(0);
        };
        if self
            .editor
            .set_selections(self.buffer_id, selections)
            .is_err()
        {
            return LRESULT(0);
        }
        self.publish_accessibility_from_core();
        self.invalidate(self.hwnd);
        LRESULT(1)
    }
}
