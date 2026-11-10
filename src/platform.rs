//! Win32 integration: global hold-to-talk hook, tray icon/menu, overlay window
//! flags and the primary work area.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetWindowLongPtrW, LoadCursorW, LoadIconW, PostMessageW,
    PostQuitMessage, PostThreadMessageW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowsHookExW, SystemParametersInfoW, TrackPopupMenu, TranslateMessage,
    UnhookWindowsHookEx, CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, HICON, HMENU, IDC_ARROW,
    IDI_APPLICATION, IMAGE_FLAGS, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MENU_ITEM_FLAGS, MSG,
    SPI_GETWORKAREA, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, TRACK_POPUP_MENU_FLAGS, WH_KEYBOARD_LL, WM_COMMAND,
    WM_CONTEXTMENU, WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_NULL, WM_QUIT,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

use crate::config::Language;
use crate::events::{TrayAction, UiEvent};
use crate::hotkey::{modifier_family, HotkeySpec};

#[derive(Clone, Copy, Debug, Default)]
pub struct TrayState {
    pub language: Language,
    pub llm_enabled: bool,
}

pub struct Platform {
    hotkey: Arc<Mutex<HotkeySpec>>,
    tray_state: Arc<Mutex<TrayState>>,
    hook_thread: Option<JoinHandle<()>>,
    tray_thread: Option<JoinHandle<()>>,
    hook_thread_id: Arc<AtomicU32>,
    tray_thread_id: Arc<AtomicU32>,
}

impl Platform {
    pub fn start(
        ctx: egui::Context,
        tx: std::sync::mpsc::Sender<UiEvent>,
        hotkey: HotkeySpec,
        tray_state: TrayState,
        icon_bytes: &'static [u8],
    ) -> Self {
        let hotkey = Arc::new(Mutex::new(hotkey));
        let tray_state = Arc::new(Mutex::new(tray_state));
        let hook_thread_id = Arc::new(AtomicU32::new(0));
        let tray_thread_id = Arc::new(AtomicU32::new(0));

        let hook_runtime = Arc::new(HookRuntime {
            ctx: ctx.clone(),
            tx: tx.clone(),
            hotkey: Arc::clone(&hotkey),
            recording: AtomicBool::new(false),
            modifiers: AtomicU8::new(0),
        });
        let _ = HOOK_RUNTIME.set(hook_runtime);

        let hook_thread = {
            let tx = tx.clone();
            let ctx = ctx.clone();
            let thread_id = Arc::clone(&hook_thread_id);
            let spawn_tx = tx.clone();
            let error_tx = tx.clone();
            std::thread::Builder::new()
                .name("voxkey-hotkey".to_string())
                .spawn(move || run_hook(thread_id, ctx, spawn_tx))
                .map_err(move |e| {
                    let _ =
                        error_tx.send(UiEvent::PlatformError(format!("无法启动快捷键线程: {e}")));
                })
                .ok()
        };

        let tray_thread = {
            let tx = tx.clone();
            let ctx = ctx.clone();
            let state = Arc::clone(&tray_state);
            let thread_id = Arc::clone(&tray_thread_id);
            let spawn_tx = tx.clone();
            let error_tx = tx.clone();
            std::thread::Builder::new()
                .name("voxkey-tray".to_string())
                .spawn(move || run_tray(thread_id, ctx, spawn_tx, state, icon_bytes))
                .map_err(move |e| {
                    let _ = error_tx.send(UiEvent::PlatformError(format!("无法启动托盘线程: {e}")));
                })
                .ok()
        };

        Self {
            hotkey,
            tray_state,
            hook_thread,
            tray_thread,
            hook_thread_id,
            tray_thread_id,
        }
    }

    pub fn set_hotkey(&self, spec: HotkeySpec) {
        if let Ok(mut guard) = self.hotkey.lock() {
            *guard = spec;
        }
    }

    pub fn set_tray_state(&self, state: TrayState) {
        if let Ok(mut guard) = self.tray_state.lock() {
            *guard = state;
        }
    }

    pub fn shutdown(&mut self) {
        stop_thread(&self.hook_thread_id, &mut self.hook_thread);
        stop_thread(&self.tray_thread_id, &mut self.tray_thread);
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn stop_thread(thread_id: &AtomicU32, handle: &mut Option<JoinHandle<()>>) {
    let id = thread_id.load(Ordering::Acquire);
    if let Some(join) = handle.take() {
        if id != 0 {
            let posted = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            if posted.is_ok() {
                let _ = join.join();
                return;
            }
        }
        // The worker did not have a message queue (or it already exited).
        // Dropping the handle is safer than blocking app shutdown forever.
        drop(join);
    }
}

// -------------------------------------------------------------------------
// Global keyboard hook
// -------------------------------------------------------------------------

static HOOK_RUNTIME: OnceLock<Arc<HookRuntime>> = OnceLock::new();

struct HookRuntime {
    ctx: egui::Context,
    tx: std::sync::mpsc::Sender<UiEvent>,
    hotkey: Arc<Mutex<HotkeySpec>>,
    recording: AtomicBool,
    modifiers: AtomicU8,
}

fn send_ui(runtime_tx: &std::sync::mpsc::Sender<UiEvent>, ctx: &egui::Context, event: UiEvent) {
    let _ = runtime_tx.send(event);
    ctx.request_repaint();
}

impl HookRuntime {
    fn handle_key(&self, msg: u32, info: KBDLLHOOKSTRUCT) {
        if info.flags.contains(LLKHF_INJECTED) {
            return;
        }
        let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
        if !down && !up {
            return;
        }

        let family = modifier_family(info.vkCode);
        if family != 0 {
            if down {
                self.modifiers.fetch_or(family, Ordering::SeqCst);
            } else {
                self.modifiers.fetch_and(!family, Ordering::SeqCst);
            }
        }

        let Ok(spec) = self.hotkey.lock().map(|guard| guard.clone()) else {
            return;
        };
        // Fn keys usually never reach Windows. On devices that do expose Fn as
        // F13 (or one of the related scan codes), accept it as an extra trigger
        // even when the configured reliable fallback is a different key.
        const VK_F13: u32 = 0x7C;
        let fn_probe = info.vkCode == VK_F13 || matches!(info.scanCode, 0x64 | 0x65);
        let is_trigger = spec.matches_trigger(info.vkCode, info.scanCode) || fn_probe;

        if down && is_trigger && !self.recording.load(Ordering::SeqCst) {
            let active = self.modifiers.load(Ordering::SeqCst);
            if spec.modifiers_match(active) {
                self.recording.store(true, Ordering::SeqCst);
                send_ui(&self.tx, &self.ctx, UiEvent::HotkeyDown);
            }
        } else if up
            && self.recording.load(Ordering::SeqCst)
            && (is_trigger || (family != 0 && (family & spec.modifiers) != 0))
        {
            self.recording.store(false, Ordering::SeqCst);
            send_ui(&self.tx, &self.ctx, UiEvent::HotkeyUp);
        }
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(runtime) = HOOK_RUNTIME.get() {
            let info = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
            runtime.handle_key(wparam.0 as u32, info);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn run_hook(thread_id: Arc<AtomicU32>, ctx: egui::Context, tx: std::sync::mpsc::Sender<UiEvent>) {
    unsafe {
        thread_id.store(GetCurrentThreadId(), Ordering::Release);
    }

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(e) => {
            send_ui(
                &tx,
                &ctx,
                UiEvent::PlatformError(format!("无法获取模块句柄: {e}")),
            );
            return;
        }
    };
    let hinstance = HINSTANCE::from(module);

    let hook = match unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(hinstance), 0)
    } {
        Ok(hook) => hook,
        Err(e) => {
            send_ui(
                &tx,
                &ctx,
                UiEvent::PlatformError(format!(
                    "无法安装全局快捷键钩子: {e}. 可尝试以普通用户权限重新启动应用"
                )),
            );
            return;
        }
    };

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0).0 };
        if result <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    if !hook.is_invalid() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
    let _ = (ctx, tx);
}

// -------------------------------------------------------------------------
// Tray icon and menu
// -------------------------------------------------------------------------

const TRAY_CLASS: &str = "VoxKeyTrayWindow";
const TRAY_WINDOW_TITLE: &str = "VoxKey Tray";
const TRAY_UID: u32 = 1;
const TRAY_CALLBACK: u32 = 0x8001; // WM_APP + 1

const ID_LANG_BASE: u32 = 1;
const ID_TOGGLE_LLM: u32 = 20;
const ID_SETTINGS: u32 = 30;
const ID_EXIT: u32 = 40;

struct TrayRuntime {
    raw_hwnd: isize,
    ctx: egui::Context,
    tx: std::sync::mpsc::Sender<UiEvent>,
    state: Arc<Mutex<TrayState>>,
}

impl TrayRuntime {
    fn hwnd(&self) -> HWND {
        HWND(self.raw_hwnd as *mut c_void)
    }
}

static TRAY_RUNTIME: OnceLock<Arc<TrayRuntime>> = OnceLock::new();

fn run_tray(
    thread_id: Arc<AtomicU32>,
    ctx: egui::Context,
    tx: std::sync::mpsc::Sender<UiEvent>,
    state: Arc<Mutex<TrayState>>,
    icon_bytes: &'static [u8],
) {
    unsafe {
        thread_id.store(GetCurrentThreadId(), Ordering::Release);
    }

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(e) => {
            send_ui(
                &tx,
                &ctx,
                UiEvent::PlatformError(format!("无法获取模块句柄: {e}")),
            );
            return;
        }
    };
    let hinstance = HINSTANCE::from(module);

    let (mut icon, mut owns_icon) = create_tray_icon(icon_bytes);
    let class_name = wide(TRAY_CLASS);
    let title = wide(TRAY_WINDOW_TITLE);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };

    let wndclass = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(tray_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: icon,
        hCursor: cursor,
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
    };

    let atom = unsafe { RegisterClassW(&wndclass) };
    if atom == 0 {
        send_ui(
            &tx,
            &ctx,
            UiEvent::PlatformError("无法注册托盘窗口类".to_string()),
        );
        if owns_icon {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
        return;
    }

    let hwnd = match unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            Default::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(e) => {
            send_ui(
                &tx,
                &ctx,
                UiEvent::PlatformError(format!("无法创建托盘窗口: {e}")),
            );
            if owns_icon {
                unsafe {
                    let _ = DestroyIcon(icon);
                }
            }
            return;
        }
    };

    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    fill_wide_buffer(&mut nid.szTip, "VoxKey · 按住快捷键说话");

    if !unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool() {
        let first_error = unsafe { GetLastError() }.0;
        // The embedded .ico is a nice-to-have; never let icon parsing prevent
        // the tray from working. Retry once with the stock application icon.
        if owns_icon {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
        icon = unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() };
        owns_icon = false;
        nid.hIcon = icon;
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool() {
            let second_error = unsafe { GetLastError() }.0;
            send_ui(
                &tx,
                &ctx,
                UiEvent::PlatformError(format!(
                    "无法添加系统托盘图标（Win32 {first_error}/{second_error}）"
                )),
            );
            cleanup_tray(hwnd, icon, owns_icon, &nid);
            return;
        }
    }

    let runtime = Arc::new(TrayRuntime {
        raw_hwnd: hwnd.0 as isize,
        ctx: ctx.clone(),
        tx: tx.clone(),
        state,
    });
    let _ = TRAY_RUNTIME.set(Arc::clone(&runtime));

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0).0 };
        if result <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    cleanup_tray(hwnd, icon, owns_icon, &nid);
    let _ = &runtime;
}

fn cleanup_tray(hwnd: HWND, icon: HICON, owns_icon: bool, nid: &NOTIFYICONDATAW) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, nid);
        let _ = DestroyWindow(hwnd);
        if owns_icon && !icon.is_invalid() {
            let _ = DestroyIcon(icon);
        }
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == TRAY_CALLBACK {
        let mouse_message = lparam.0 as u32;
        match mouse_message {
            WM_RBUTTONUP | WM_CONTEXTMENU => handle_tray_event(hwnd, TrayEvent::ShowMenu),
            WM_LBUTTONDBLCLK => handle_tray_event(hwnd, TrayEvent::OpenSettings),
            _ => {}
        }
        return LRESULT(0);
    }

    if msg == WM_COMMAND {
        let id = (wparam.0 & 0xFFFF) as u32;
        handle_menu_command(id);
        return LRESULT(0);
    }

    if msg == WM_DESTROY {
        unsafe {
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }

    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

enum TrayEvent {
    ShowMenu,
    OpenSettings,
}

fn handle_tray_event(hwnd: HWND, event: TrayEvent) {
    let Some(runtime) = TRAY_RUNTIME.get() else {
        return;
    };
    match event {
        TrayEvent::OpenSettings => {
            send_ui(
                &runtime.tx,
                &runtime.ctx,
                UiEvent::Tray(TrayAction::OpenSettings),
            );
        }
        TrayEvent::ShowMenu => {
            if runtime.hwnd() != hwnd {
                return;
            }
            show_tray_menu(runtime);
        }
    }
}

fn show_tray_menu(runtime: &TrayRuntime) {
    let menu = match unsafe { CreatePopupMenu() } {
        Ok(menu) => menu,
        Err(_) => return,
    };
    let state = runtime.state.lock().map(|guard| *guard).unwrap_or_default();

    append_menu(menu, 0x1, 0, "VoxKey", false); // grayed header
    append_menu(menu, 0x800, 0, "", false); // separator
    for (index, language) in Language::ALL.iter().enumerate() {
        append_menu(
            menu,
            0,
            (ID_LANG_BASE + index as u32) as usize,
            language.label(),
            *language == state.language,
        );
    }
    append_menu(menu, 0x800, 0, "", false);
    append_menu(
        menu,
        0,
        ID_TOGGLE_LLM as usize,
        "LLM 纠错",
        state.llm_enabled,
    );
    append_menu(menu, 0, ID_SETTINGS as usize, "设置…", false);
    append_menu(menu, 0x800, 0, "", false);
    append_menu(menu, 0, ID_EXIT as usize, "退出", false);

    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(runtime.hwnd());
        let _ = TrackPopupMenu(
            menu,
            TRACK_POPUP_MENU_FLAGS(0x0002 | 0x0020),
            point.x,
            point.y,
            None,
            runtime.hwnd(),
            None,
        );
        let _ = PostMessageW(Some(runtime.hwnd()), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn append_menu(menu: HMENU, base_flags: u32, id: usize, label: &str, checked: bool) {
    let flags = base_flags | if checked { 0x8 } else { 0 };
    let text = wide(label);
    unsafe {
        let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(flags), id, PCWSTR(text.as_ptr()));
    }
}

fn handle_menu_command(id: u32) {
    let Some(runtime) = TRAY_RUNTIME.get() else {
        return;
    };
    match id {
        ID_LANG_BASE..=5 => {
            let index = (id - ID_LANG_BASE) as usize;
            if let Some(language) = Language::ALL.get(index) {
                send_ui(
                    &runtime.tx,
                    &runtime.ctx,
                    UiEvent::Tray(TrayAction::SetLanguage(*language)),
                );
            }
        }
        ID_TOGGLE_LLM => {
            send_ui(
                &runtime.tx,
                &runtime.ctx,
                UiEvent::Tray(TrayAction::ToggleLlm),
            );
        }
        ID_SETTINGS => {
            send_ui(
                &runtime.tx,
                &runtime.ctx,
                UiEvent::Tray(TrayAction::OpenSettings),
            );
        }
        ID_EXIT => {
            send_ui(&runtime.tx, &runtime.ctx, UiEvent::Tray(TrayAction::Quit));
        }
        _ => {}
    }
}

fn create_tray_icon(bytes: &[u8]) -> (HICON, bool) {
    if let Some(icon) = icon_from_ico(bytes) {
        return (icon, true);
    }
    match unsafe { LoadIconW(None, IDI_APPLICATION) } {
        Ok(icon) => (icon, false),
        Err(_) => (HICON::default(), false),
    }
}

fn icon_from_ico(bytes: &[u8]) -> Option<HICON> {
    if bytes.len() < 6 || bytes[0..4] != [0, 0, 1, 0] {
        return None;
    }
    let count = read_u16(bytes, 4)? as usize;
    let mut best: Option<(u32, usize, usize)> = None;

    for index in 0..count {
        let entry = 6 + index * 16;
        if entry + 16 > bytes.len() {
            break;
        }
        let width = if bytes[entry] == 0 {
            256u32
        } else {
            bytes[entry] as u32
        };
        let size = read_u32(bytes, entry + 8)? as usize;
        let offset = read_u32(bytes, entry + 12)? as usize;
        if offset + size > bytes.len() {
            continue;
        }
        let score = if width == 32 {
            100
        } else if width <= 64 {
            50
        } else {
            10
        };
        if best.map(|(s, _, _)| score > s).unwrap_or(true) {
            best = Some((score, offset, size));
        }
    }

    let (_, offset, size) = best?;
    unsafe {
        CreateIconFromResourceEx(
            &bytes[offset..offset + size],
            true,
            0x0003_0000,
            32,
            32,
            IMAGE_FLAGS(0),
        )
        .ok()
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

// -------------------------------------------------------------------------
// Window helpers
// -------------------------------------------------------------------------

/// Marks the hidden-by-default overlay window as non-activating. Call it while
/// the overlay is still hidden, so showing it never steals focus from the app
/// receiving dictated text.
pub fn configure_overlay_window(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_style = (style | (WS_EX_NOACTIVATE.0 as isize) | (WS_EX_TOOLWINDOW.0 as isize))
            & !(WS_EX_APPWINDOW.0 as isize);
        if new_style != style {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn foreground_window() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        None
    } else {
        Some(hwnd)
    }
}

pub fn set_foreground_window(hwnd: HWND) {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Primary monitor work area in physical pixels: (left, top, width, height).
pub fn primary_work_area() -> Option<(i32, i32, i32, i32)> {
    let mut rect = RECT::default();
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if result.is_err() {
        return None;
    }
    Some((
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ))
}

fn fill_wide_buffer<const N: usize>(buffer: &mut [u16; N], text: &str) {
    for (slot, value) in buffer.iter_mut().zip(text.encode_utf16()) {
        *slot = value;
    }
    if let Some(last) = buffer.last_mut() {
        *last = 0;
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
