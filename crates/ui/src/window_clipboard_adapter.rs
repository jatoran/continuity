//! Native host adapter for editor-surface clipboard mediation requests.

use continuity_command::Error;
use continuity_host::HostRequest;

use crate::Window;

enum NativeHostResponse {
    ClipboardText(Option<String>),
    Complete,
}

impl Window {
    fn dispatch_native_host_request(
        &self,
        request: HostRequest,
    ) -> Result<NativeHostResponse, Error> {
        match request {
            HostRequest::ReadClipboard => continuity_win::clipboard::read_text(self.hwnd)
                .map(NativeHostResponse::ClipboardText)
                .map_err(|error| {
                    eprintln!("continuity-ui: clipboard read failed: {error}");
                    Error::UnsupportedContext("clipboard read failed")
                }),
            HostRequest::WriteClipboard(text) => {
                continuity_win::clipboard::write_text(self.hwnd, &text)
                    .map(|()| NativeHostResponse::Complete)
                    .map_err(|error| {
                        eprintln!("continuity-ui: clipboard write failed: {error}");
                        Error::UnsupportedContext("clipboard write failed")
                    })
            }
            HostRequest::ContextMenu { .. }
            | HostRequest::ActivateLink(_)
            | HostRequest::DroppedFiles(_) => {
                Err(Error::UnsupportedContext("unsupported native host request"))
            }
        }
    }

    /// Ask the native embedding host for plain clipboard text.
    pub(crate) fn request_host_clipboard_read(&self) -> Result<Option<String>, Error> {
        match self.dispatch_native_host_request(HostRequest::ReadClipboard)? {
            NativeHostResponse::ClipboardText(text) => Ok(text),
            NativeHostResponse::Complete => {
                Err(Error::UnsupportedContext("invalid clipboard host response"))
            }
        }
    }

    /// Ask the native embedding host to replace plain clipboard text.
    pub(crate) fn request_host_clipboard_write(&self, text: &str) -> Result<(), Error> {
        match self.dispatch_native_host_request(HostRequest::WriteClipboard(text.to_owned()))? {
            NativeHostResponse::Complete => Ok(()),
            NativeHostResponse::ClipboardText(_) => {
                Err(Error::UnsupportedContext("invalid clipboard host response"))
            }
        }
    }
}
