//! UI Automation text ranges backed by the surface accessibility snapshot.

use std::sync::{Arc, Mutex};

use continuity_buffer::RopeSnapshot;
use continuity_text::Position;
use windows::core::{implement, AsImpl, Interface, Ref, BSTR, HRESULT};
use windows::Win32::Foundation::{BOOL, E_OUTOFMEMORY, HWND};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_R8, VT_UNKNOWN};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, ITextRangeProvider, ITextRangeProvider_Impl,
    TextPatternRangeEndpoint, TextPatternRangeEndpoint_Start, TextUnit, TextUnit_Character,
    TextUnit_Document, TextUnit_Line, TextUnit_Paragraph, TextUnit_Word, UIA_IsReadOnlyAttributeId,
    UiaGetReservedNotSupportedValue, UIA_E_NOTSUPPORTED, UIA_TEXTATTRIBUTE_ID,
};

use super::{send_selection_request, AccessibilitySelectionAction};
use crate::editor_surface::accessibility::{AccessibilityDocument, AccessibilitySnapshot};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Endpoints {
    start: usize,
    end: usize,
}

#[implement(ITextRangeProvider)]
pub(super) struct TextRange {
    hwnd: HWND,
    shared: Arc<Mutex<AccessibilitySnapshot>>,
    enclosing: IRawElementProviderSimple,
    /// UI Automation may mutate a range from a COM thread while another query
    /// observes it. The lock covers only two UTF-16 indices and is never held
    /// during document conversion or COM calls.
    endpoints: Mutex<Endpoints>,
}

impl TextRange {
    pub(super) fn new(
        hwnd: HWND,
        shared: Arc<Mutex<AccessibilitySnapshot>>,
        enclosing: IRawElementProviderSimple,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            hwnd,
            shared,
            enclosing,
            endpoints: Mutex::new(Endpoints {
                start: start.min(end),
                end: start.max(end),
            }),
        }
    }
}

impl TextRange_Impl {
    fn snapshot(&self) -> AccessibilitySnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn endpoints(&self, document_length: usize) -> Endpoints {
        let endpoints = *self
            .endpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Endpoints {
            start: endpoints.start.min(document_length),
            end: endpoints
                .end
                .min(document_length)
                .max(endpoints.start.min(document_length)),
        }
    }

    fn replace_endpoints(&self, endpoints: Endpoints) {
        *self
            .endpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = endpoints;
    }

    fn document_and_endpoints(&self) -> Option<(AccessibilityDocument, Endpoints)> {
        let document = self.snapshot().document?;
        let endpoints = self.endpoints(document_length_utf16(&document.rope));
        Some((document, endpoints))
    }

    fn clone_range(&self, endpoints: Endpoints) -> ITextRangeProvider {
        TextRange::new(
            self.hwnd,
            Arc::clone(&self.shared),
            self.enclosing.clone(),
            endpoints.start,
            endpoints.end,
        )
        .into()
    }

    fn target_endpoints(
        &self,
        target: Ref<'_, ITextRangeProvider>,
    ) -> windows::core::Result<Endpoints> {
        let target = target.as_ref().ok_or_else(not_supported)?;
        let enclosing = unsafe { target.GetEnclosingElement()? };
        if enclosing != self.enclosing {
            return Err(not_supported());
        }
        // The enclosing provider identity proves this range came from this
        // editor provider; every range it creates is a `TextRange`.
        let target_impl: &TextRange = unsafe { AsImpl::<TextRange>::as_impl(target) };
        let document_length = self
            .snapshot()
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        let endpoints = *target_impl
            .endpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(Endpoints {
            start: endpoints.start.min(document_length),
            end: endpoints.end.min(document_length),
        })
    }
}

impl ITextRangeProvider_Impl for TextRange_Impl {
    fn Clone(&self) -> windows::core::Result<ITextRangeProvider> {
        let length = self
            .snapshot()
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        Ok(self.clone_range(self.endpoints(length)))
    }

    fn Compare(&self, range: Ref<'_, ITextRangeProvider>) -> windows::core::Result<BOOL> {
        let length = self
            .snapshot()
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        Ok(BOOL::from(
            self.endpoints(length) == self.target_endpoints(range)?,
        ))
    }

    fn CompareEndpoints(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: Ref<'_, ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> windows::core::Result<i32> {
        let length = self
            .snapshot()
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        let own = self.endpoints(length);
        let target = self.target_endpoints(targetrange)?;
        let own_value = if endpoint == TextPatternRangeEndpoint_Start {
            own.start
        } else {
            own.end
        };
        let target_value = if targetendpoint == TextPatternRangeEndpoint_Start {
            target.start
        } else {
            target.end
        };
        Ok(own_value
            .saturating_sub(target_value)
            .min(i32::MAX as usize) as i32
            - target_value
                .saturating_sub(own_value)
                .min(i32::MAX as usize) as i32)
    }

    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> windows::core::Result<()> {
        let Some((document, endpoints)) = self.document_and_endpoints() else {
            return Ok(());
        };
        let text = document_text_utf16(&document.rope);
        self.replace_endpoints(expand_to_unit(&text, endpoints, unit));
        Ok(())
    }

    fn FindAttribute(
        &self,
        _attributeid: UIA_TEXTATTRIBUTE_ID,
        _val: &VARIANT,
        _backward: BOOL,
    ) -> windows::core::Result<ITextRangeProvider> {
        Err(not_supported())
    }

    fn FindText(
        &self,
        text: &BSTR,
        backward: BOOL,
        ignorecase: BOOL,
    ) -> windows::core::Result<ITextRangeProvider> {
        let Some((document, endpoints)) = self.document_and_endpoints() else {
            return Err(not_supported());
        };
        let haystack = utf16_range(&document.rope, endpoints);
        let needle = text.to_string();
        let haystack_search = if ignorecase.as_bool() {
            haystack.to_lowercase()
        } else {
            haystack.clone()
        };
        let needle_search = if ignorecase.as_bool() {
            needle.to_lowercase()
        } else {
            needle
        };
        let found = if backward.as_bool() {
            haystack_search.rfind(&needle_search)
        } else {
            haystack_search.find(&needle_search)
        };
        let Some(byte_start) = found else {
            return Err(not_supported());
        };
        let start_offset = haystack_search[..byte_start].encode_utf16().count();
        let match_length = needle_search.encode_utf16().count();
        Ok(self.clone_range(Endpoints {
            start: endpoints.start + start_offset,
            end: endpoints.start + start_offset + match_length,
        }))
    }

    fn GetAttributeValue(
        &self,
        attributeid: UIA_TEXTATTRIBUTE_ID,
    ) -> windows::core::Result<VARIANT> {
        if attributeid == UIA_IsReadOnlyAttributeId {
            let is_read_only = self
                .snapshot()
                .document
                .is_some_and(|document| document.is_read_only);
            Ok(VARIANT::from(is_read_only))
        } else {
            Ok(VARIANT::from(unsafe { UiaGetReservedNotSupportedValue()? }))
        }
    }

    fn GetBoundingRectangles(&self) -> windows::core::Result<*mut SAFEARRAY> {
        create_empty_array(VT_R8)
    }

    fn GetEnclosingElement(&self) -> windows::core::Result<IRawElementProviderSimple> {
        Ok(self.enclosing.clone())
    }

    fn GetText(&self, maxlength: i32) -> windows::core::Result<BSTR> {
        let Some((document, endpoints)) = self.document_and_endpoints() else {
            return Ok(BSTR::new());
        };
        let mut utf16: Vec<u16> = utf16_range(&document.rope, endpoints)
            .encode_utf16()
            .collect();
        if maxlength >= 0 {
            utf16.truncate(maxlength as usize);
        }
        Ok(BSTR::from_wide(&utf16))
    }

    fn Move(&self, unit: TextUnit, count: i32) -> windows::core::Result<i32> {
        let Some((document, endpoints)) = self.document_and_endpoints() else {
            return Ok(0);
        };
        let text = document_text_utf16(&document.rope);
        let (start, moved) = move_index(&text, endpoints.start, unit, count);
        let width = endpoints.end.saturating_sub(endpoints.start);
        self.replace_endpoints(Endpoints {
            start,
            end: start.saturating_add(width).min(text.len()),
        });
        Ok(moved)
    }

    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> windows::core::Result<i32> {
        let Some((document, mut endpoints)) = self.document_and_endpoints() else {
            return Ok(0);
        };
        let text = document_text_utf16(&document.rope);
        let current = if endpoint == TextPatternRangeEndpoint_Start {
            endpoints.start
        } else {
            endpoints.end
        };
        let (target, moved) = move_index(&text, current, unit, count);
        if endpoint == TextPatternRangeEndpoint_Start {
            endpoints.start = target;
            if endpoints.start > endpoints.end {
                endpoints.end = endpoints.start;
            }
        } else {
            endpoints.end = target;
            if endpoints.end < endpoints.start {
                endpoints.start = endpoints.end;
            }
        }
        self.replace_endpoints(endpoints);
        Ok(moved)
    }

    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: Ref<'_, ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> windows::core::Result<()> {
        let target = self.target_endpoints(targetrange)?;
        let value = if targetendpoint == TextPatternRangeEndpoint_Start {
            target.start
        } else {
            target.end
        };
        let length = self
            .snapshot()
            .document
            .as_ref()
            .map_or(0, |document| document_length_utf16(&document.rope));
        let mut own = self.endpoints(length);
        if endpoint == TextPatternRangeEndpoint_Start {
            own.start = value;
            if own.start > own.end {
                own.end = own.start;
            }
        } else {
            own.end = value;
            if own.end < own.start {
                own.start = own.end;
            }
        }
        self.replace_endpoints(own);
        Ok(())
    }

    fn Select(&self) -> windows::core::Result<()> {
        let endpoints = self
            .document_and_endpoints()
            .map_or(Endpoints::default(), |(_, value)| value);
        send_selection_request(
            self.hwnd,
            endpoints.start,
            endpoints.end,
            AccessibilitySelectionAction::Replace,
        )
    }

    fn AddToSelection(&self) -> windows::core::Result<()> {
        let endpoints = self
            .document_and_endpoints()
            .map_or(Endpoints::default(), |(_, value)| value);
        send_selection_request(
            self.hwnd,
            endpoints.start,
            endpoints.end,
            AccessibilitySelectionAction::Add,
        )
    }

    fn RemoveFromSelection(&self) -> windows::core::Result<()> {
        let endpoints = self
            .document_and_endpoints()
            .map_or(Endpoints::default(), |(_, value)| value);
        send_selection_request(
            self.hwnd,
            endpoints.start,
            endpoints.end,
            AccessibilitySelectionAction::Remove,
        )
    }

    fn ScrollIntoView(&self, _aligntotop: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetChildren(&self) -> windows::core::Result<*mut SAFEARRAY> {
        create_empty_array(VT_UNKNOWN)
    }
}

pub(super) fn not_supported() -> windows::core::Error {
    windows::core::Error::from_hresult(HRESULT(UIA_E_NOTSUPPORTED as i32))
}

pub(super) fn document_length_utf16(snapshot: &RopeSnapshot) -> usize {
    snapshot.rope().chars().map(char::len_utf16).sum()
}

pub(super) fn position_to_utf16(snapshot: &RopeSnapshot, position: Position) -> usize {
    let rope = snapshot.rope();
    let byte = position.to_byte_offset(rope).unwrap_or(rope.len_bytes());
    let character = rope.byte_to_char(byte.min(rope.len_bytes()));
    rope.slice(..character).chars().map(char::len_utf16).sum()
}

pub(super) fn utf16_to_position(snapshot: &RopeSnapshot, target: usize) -> Position {
    let rope = snapshot.rope();
    let mut utf16_offset = 0;
    let mut byte_offset = 0;
    for character in rope.chars() {
        let width = character.len_utf16();
        if utf16_offset + width > target {
            break;
        }
        utf16_offset += width;
        byte_offset += character.len_utf8();
    }
    Position::from_byte_offset(rope, byte_offset).unwrap_or(Position::ZERO)
}

pub(super) fn create_provider_array(
    providers: &[ITextRangeProvider],
) -> windows::core::Result<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, providers.len() as u32) };
    if array.is_null() {
        return Err(windows::core::Error::from_hresult(E_OUTOFMEMORY));
    }
    for (index, provider) in providers.iter().enumerate() {
        let raw = Interface::as_raw(provider);
        if let Err(error) = unsafe {
            SafeArrayPutElement(
                array,
                &(index as i32),
                (&raw as *const *mut core::ffi::c_void).cast(),
            )
        } {
            unsafe {
                let _ = SafeArrayDestroy(array);
            }
            return Err(error);
        }
    }
    Ok(array)
}

fn create_empty_array(
    value_type: windows::Win32::System::Variant::VARENUM,
) -> windows::core::Result<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(value_type, 0, 0) };
    if array.is_null() {
        Err(windows::core::Error::from_hresult(E_OUTOFMEMORY))
    } else {
        Ok(array)
    }
}

fn document_text_utf16(snapshot: &RopeSnapshot) -> Vec<u16> {
    snapshot.rope().to_string().encode_utf16().collect()
}

fn utf16_range(snapshot: &RopeSnapshot, endpoints: Endpoints) -> String {
    let text = document_text_utf16(snapshot);
    String::from_utf16_lossy(&text[endpoints.start.min(text.len())..endpoints.end.min(text.len())])
}

fn expand_to_unit(text: &[u16], endpoints: Endpoints, unit: TextUnit) -> Endpoints {
    if unit == TextUnit_Document {
        return Endpoints {
            start: 0,
            end: text.len(),
        };
    }
    let pivot = endpoints.start.min(text.len());
    if unit == TextUnit_Character {
        return Endpoints {
            start: pivot,
            end: (pivot + 1).min(text.len()),
        };
    }
    let boundary = |value: u16| {
        if unit == TextUnit_Line || unit == TextUnit_Paragraph {
            value == b'\n' as u16
        } else if unit == TextUnit_Word {
            char::from_u32(value as u32).is_some_and(char::is_whitespace)
        } else {
            false
        }
    };
    let start = text[..pivot]
        .iter()
        .rposition(|value| boundary(*value))
        .map_or(0, |index| index + 1);
    let end = text[pivot..]
        .iter()
        .position(|value| boundary(*value))
        .map_or(text.len(), |index| pivot + index + 1);
    Endpoints { start, end }
}

fn move_index(text: &[u16], current: usize, unit: TextUnit, count: i32) -> (usize, i32) {
    if count == 0 {
        return (current.min(text.len()), 0);
    }
    if unit == TextUnit_Document {
        return if count < 0 { (0, -1) } else { (text.len(), 1) };
    }
    let mut index = current.min(text.len());
    let mut moved = 0;
    let step = count.signum();
    while moved != count {
        let next = if unit == TextUnit_Character {
            if step < 0 {
                index.saturating_sub(1)
            } else {
                (index + 1).min(text.len())
            }
        } else {
            let probe = if step < 0 {
                index.saturating_sub(1)
            } else {
                index
            };
            let expanded = expand_to_unit(
                text,
                Endpoints {
                    start: probe,
                    end: probe,
                },
                unit,
            );
            if step < 0 {
                expanded.start
            } else {
                expanded.end
            }
        };
        if next == index {
            break;
        }
        index = next;
        moved += step;
    }
    (index, moved)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::{Revision, RopeSnapshot};
    use ropey::Rope;

    use super::*;

    #[test]
    fn position_conversion_counts_utf16_surrogate_pairs() {
        let rope = RopeSnapshot::new(Arc::new(Rope::from_str("a😀b")), Revision::INITIAL);
        assert_eq!(position_to_utf16(&rope, Position::new(0, 5)), 3);
        assert_eq!(document_length_utf16(&rope), 4);
    }

    #[test]
    fn line_expansion_includes_line_ending() {
        let text: Vec<u16> = "one\ntwo".encode_utf16().collect();
        assert_eq!(
            expand_to_unit(&text, Endpoints { start: 1, end: 1 }, TextUnit_Line),
            Endpoints { start: 0, end: 4 }
        );
    }
}
