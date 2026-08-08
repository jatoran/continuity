//! Minimal non-Continuity Win32 host for [`continuity_ui::EditorControl`].
//!
//! This harness deliberately does not construct `continuity_ui::Window`, an
//! editor actor, SQLite, desktop registries, or application services. Its
//! worker thread owns a plain parent HWND and the ordinary Win32
//! `GetMessage`/`TranslateMessage`/`DispatchMessage` loop.

use std::sync::mpsc;
use std::thread::JoinHandle;

use continuity_host::HostEventBatch;
use continuity_ui::{
    ControlBounds, ControlEventSink, ControlOptions, ControlRuntime, EditorControl,
};
use continuity_win::{ComGuard, WindowClass};
use crossbeam_channel::Receiver;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, MoveWindow, PostMessageW, PostQuitMessage, SetWindowLongPtrW,
    TranslateMessage, CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA, HMENU, MSG, WINDOW_EX_STYLE,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WS_OVERLAPPEDWINDOW,
};

const QUERY_TEXT_MESSAGE: u32 = WM_APP + 41;
const DESTROY_CONTROL_MESSAGE: u32 = WM_APP + 42;
const RECREATE_CONTROL_MESSAGE: u32 = WM_APP + 43;
const REQUEST_CLIPBOARD_MESSAGE: u32 = WM_APP + 44;
const PROVIDE_CLIPBOARD_MESSAGE: u32 = WM_APP + 45;
const DISPATCH_COMMAND_MESSAGE: u32 = WM_APP + 46;

struct HarnessState {
    controls: Vec<Option<EditorControl>>,
    options: ControlOptions,
}

struct RecreateRequest {
    index: usize,
    text: String,
    hwnd: HWND,
    receiver: Option<Receiver<HostEventBatch>>,
}

struct TextRequest<'a> {
    index: usize,
    text: &'a str,
}

/// Test-side handle to a dedicated host-owned message pump and child controls.
pub struct EditorControlHarness {
    parent: HWND,
    children: Vec<HWND>,
    receivers: Vec<Receiver<HostEventBatch>>,
    worker: Option<JoinHandle<()>>,
}

impl EditorControlHarness {
    /// Spawn a plain Win32 host containing `count` independent child editors.
    ///
    /// # Panics
    ///
    /// Panics when the host thread cannot initialize or create its HWNDs.
    #[must_use]
    pub fn spawn(count: usize, initial_text: &str) -> Self {
        Self::spawn_with_options(count, initial_text, ControlOptions::default())
    }

    /// Spawn controls with an explicit native behavior configuration.
    ///
    /// # Panics
    ///
    /// Panics when the host thread cannot initialize or create its HWNDs.
    #[must_use]
    pub fn spawn_with_options(count: usize, initial_text: &str, options: ControlOptions) -> Self {
        assert!(count > 0, "host harness requires at least one control");
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let text = initial_text.to_owned();
        let worker = std::thread::spawn(move || {
            let result = run_host(count, &text, options, &ready_sender);
            if let Err(error) = result {
                let _ = ready_sender.send(Err(error));
            }
        });
        let (parent_raw, children_raw, receivers) = ready_receiver
            .recv()
            .expect("host thread should report startup")
            .expect("host harness should initialize");
        Self {
            parent: HWND(parent_raw as *mut core::ffi::c_void),
            children: children_raw
                .into_iter()
                .map(|raw| HWND(raw as *mut core::ffi::c_void))
                .collect(),
            receivers,
            worker: Some(worker),
        }
    }

    /// Parent HWND owned by the harness rather than Continuity desktop code.
    #[must_use]
    pub fn parent_hwnd(&self) -> HWND {
        self.parent
    }

    /// Current HWND for one child slot.
    #[must_use]
    pub fn child_hwnd(&self, index: usize) -> HWND {
        self.children[index]
    }

    /// Host-event receiver for one child slot.
    #[must_use]
    pub fn events(&self, index: usize) -> &Receiver<HostEventBatch> {
        &self.receivers[index]
    }

    /// Send a native message synchronously to one child HWND.
    pub fn send_child_message(&self, index: usize, message: u32, wparam: usize, lparam: isize) {
        let _ = self.send_child_message_result(index, message, wparam, lparam);
    }

    /// Send a native message and return its `LRESULT` value.
    #[must_use]
    pub fn send_child_message_result(
        &self,
        index: usize,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.children[index],
                message,
                Some(WPARAM(wparam)),
                Some(LPARAM(lparam)),
            )
            .0
        }
    }

    /// Query canonical text on the host UI thread.
    #[must_use]
    pub fn text(&self, index: usize) -> String {
        let mut output = String::new();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                QUERY_TEXT_MESSAGE,
                Some(WPARAM(index)),
                Some(LPARAM((&mut output as *mut String) as isize)),
            );
        }
        output
    }

    /// Resize a child through the same parent-layout operation used by hosts.
    pub fn resize(&self, index: usize, width: i32, height: i32) {
        unsafe {
            let _ = MoveWindow(
                self.children[index],
                0,
                0,
                width.max(1),
                height.max(1),
                true,
            );
        }
    }

    /// Destroy one child without stopping the host message pump.
    pub fn destroy_control(&mut self, index: usize) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                DESTROY_CONTROL_MESSAGE,
                Some(WPARAM(index)),
                None,
            );
        }
        self.children[index] = HWND::default();
    }

    /// Recreate one child slot with fresh ephemeral state.
    pub fn recreate_control(&mut self, index: usize, text: &str) {
        let mut request = RecreateRequest {
            index,
            text: text.to_owned(),
            hwnd: HWND::default(),
            receiver: None,
        };
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                RECREATE_CONTROL_MESSAGE,
                None,
                Some(LPARAM((&mut request as *mut RecreateRequest) as isize)),
            );
        }
        assert!(
            !request.hwnd.is_invalid(),
            "recreated child should be valid"
        );
        self.children[index] = request.hwnd;
        self.receivers[index] = request
            .receiver
            .expect("recreated child should return an event receiver");
    }

    /// Ask the host-mediated clipboard adapter to emit a read request.
    pub fn request_clipboard_read(&self, index: usize) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                REQUEST_CLIPBOARD_MESSAGE,
                Some(WPARAM(index)),
                None,
            );
        }
    }

    /// Return clipboard text to one child on its owner thread.
    pub fn provide_clipboard_text(&self, index: usize, text: &str) {
        let request = TextRequest { index, text };
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                PROVIDE_CLIPBOARD_MESSAGE,
                None,
                Some(LPARAM((&request as *const TextRequest<'_>) as isize)),
            );
        }
    }

    /// Dispatch a stable command through the child API on its owner thread.
    pub fn dispatch_command(&self, index: usize, command: &str) {
        let request = TextRequest {
            index,
            text: command,
        };
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.parent,
                DISPATCH_COMMAND_MESSAGE,
                None,
                Some(LPARAM((&request as *const TextRequest<'_>) as isize)),
            );
        }
    }
}

impl Drop for EditorControlHarness {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(Some(self.parent), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

type Startup = (isize, Vec<isize>, Vec<Receiver<HostEventBatch>>);

fn run_host(
    count: usize,
    text: &str,
    options: ControlOptions,
    ready_sender: &mpsc::SyncSender<Result<Startup, String>>,
) -> Result<(), String> {
    let _com = ComGuard::new().map_err(|error| error.to_string())?;
    let _ = continuity_win::set_per_monitor_dpi_v2();
    let class =
        WindowClass::register_unique_with_proc("ContinuityExternalHost", Some(host_wndproc))
            .map_err(|error| error.to_string())?;
    let mut state = Box::new(HarnessState {
        controls: Vec::new(),
        options,
    });
    let parent = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class.name().as_ptr()),
            &HSTRING::from("external editor-control host"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            700,
            None,
            Option::<HMENU>::None,
            Some(class.hinstance().into()),
            Some((&mut *state as *mut HarnessState).cast()),
        )
    }
    .map_err(|error| error.to_string())?;
    let mut children = Vec::with_capacity(count);
    let mut receivers = Vec::with_capacity(count);
    for index in 0..count {
        let (control, receiver) = create_control(parent, index, text, state.options.clone())?;
        children.push(control.hwnd());
        receivers.push(receiver);
        state.controls.push(Some(control));
    }
    ready_sender
        .send(Ok((
            parent.0 as isize,
            children.into_iter().map(|child| child.0 as isize).collect(),
            receivers,
        )))
        .map_err(|error| error.to_string())?;
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(state);
    unsafe {
        let _ = DestroyWindow(parent);
    }
    drop(class);
    Ok(())
}

fn create_control(
    parent: HWND,
    index: usize,
    text: &str,
    options: ControlOptions,
) -> Result<(EditorControl, Receiver<HostEventBatch>), String> {
    let (sink, receiver) = ControlEventSink::bounded(256);
    let control = EditorControl::new(
        parent,
        ControlBounds {
            x: (index as i32 % 2) * 440,
            y: (index as i32 / 2) * 330,
            width: 430,
            height: 320,
        },
        ControlRuntime::Ephemeral {
            initial_text: text.to_owned(),
        },
        options,
        sink,
    )
    .map_err(|error| error.to_string())?;
    Ok((control, receiver))
}

unsafe extern "system" fn host_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() {
            let state = unsafe { (*create).lpCreateParams } as isize;
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state) };
        }
    }
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut HarnessState;
    match message {
        QUERY_TEXT_MESSAGE if !state.is_null() => {
            let output = lparam.0 as *mut String;
            let index = wparam.0;
            if let (Some(control), Some(output)) = (
                unsafe { &mut *state }
                    .controls
                    .get(index)
                    .and_then(Option::as_ref),
                unsafe { output.as_mut() },
            ) {
                *output = control.text().unwrap_or_default();
            }
            LRESULT(1)
        }
        DESTROY_CONTROL_MESSAGE if !state.is_null() => {
            let index = wparam.0;
            if let Some(slot) = unsafe { &mut *state }.controls.get_mut(index) {
                *slot = None;
            }
            LRESULT(1)
        }
        RECREATE_CONTROL_MESSAGE if !state.is_null() => {
            let request = lparam.0 as *mut RecreateRequest;
            let Some(request) = (unsafe { request.as_mut() }) else {
                return LRESULT(0);
            };
            let state = unsafe { &mut *state };
            if request.index >= state.controls.len() {
                return LRESULT(0);
            }
            match create_control(hwnd, request.index, &request.text, state.options.clone()) {
                Ok((control, receiver)) => {
                    request.hwnd = control.hwnd();
                    request.receiver = Some(receiver);
                    state.controls[request.index] = Some(control);
                    LRESULT(1)
                }
                Err(_) => LRESULT(0),
            }
        }
        REQUEST_CLIPBOARD_MESSAGE if !state.is_null() => {
            let index = wparam.0;
            if let Some(control) = unsafe { &mut *state }
                .controls
                .get_mut(index)
                .and_then(Option::as_mut)
            {
                let _ = control.dispatch(continuity_host::EditorIntent::Request(
                    continuity_host::HostRequest::ReadClipboard,
                ));
            }
            LRESULT(1)
        }
        PROVIDE_CLIPBOARD_MESSAGE | DISPATCH_COMMAND_MESSAGE if !state.is_null() => {
            let request = lparam.0 as *const TextRequest<'_>;
            let Some(request) = (unsafe { request.as_ref() }) else {
                return LRESULT(0);
            };
            if let Some(control) = unsafe { &mut *state }
                .controls
                .get_mut(request.index)
                .and_then(Option::as_mut)
            {
                if message == PROVIDE_CLIPBOARD_MESSAGE {
                    let _ = control.provide_clipboard_text(request.text);
                } else {
                    let _ = control.dispatch_command(request.text);
                }
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
