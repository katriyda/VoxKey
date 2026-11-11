//! Clipboard + Ctrl+V text injection.
//!
//! The previous clipboard is snapshotted before injection and restored
//! afterwards. Common memory-backed clipboard formats (Unicode/ANSI text,
//! DIB images, dropped file lists, locale, registered formats) are preserved.
//! GDI-handle formats such as CF_BITMAP cannot be duplicated reliably with the
//! plain Win32 clipboard API and are skipped; see README for the limitation.

use std::mem::size_of;
use std::ptr::copy_nonoverlapping;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_V,
};

const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_PALETTE: u32 = 9;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_OWNERDISPLAY: u32 = 0x0080;
const CF_DSPBITMAP: u32 = 0x0082;
const CF_DSPMETAFILEPICT: u32 = 0x0083;
const CF_DSPENHMETAFILE: u32 = 0x00E0;
const CF_PRIVATEFIRST: u32 = 0x0200;
const CF_PRIVATELAST: u32 = 0x02FF;

#[derive(Clone, Debug)]
struct ClipboardItem {
    format: u32,
    bytes: Vec<u8>,
}

/// Inject text into the focused control using the clipboard + Ctrl+V.
///
/// Returns `Ok(None)` on full success. `Ok(Some(message))` means the text was
/// pasted but the original clipboard could not be fully restored. `Err` means
/// the text was not pasted.
pub fn inject_text(text: &str) -> Result<Option<String>, String> {
    if text.is_empty() {
        return Err("转录为空，没有可插入的文本".to_string());
    }

    let snapshot = capture_clipboard()
        .map_err(|e| format!("读取原剪贴板失败（为保护原内容，已取消插入）: {e}"))?;

    if let Err(err) = set_clipboard_text(text) {
        // EmptyClipboard may already have run; try to put the original back.
        let restore = restore_clipboard(&snapshot);
        let suffix = match restore {
            Ok(()) => String::new(),
            Err(restore_err) => format!("；恢复原剪贴板也失败: {restore_err}"),
        };
        return Err(format!("写入剪贴板失败: {err}{suffix}"));
    }

    // Let clipboard consumers receive WM_CLIPBOARDUPDATE before simulating the
    // paste. Many editors read the clipboard synchronously on Ctrl+V.
    sleep(Duration::from_millis(70));
    let paste_result = send_ctrl_v();

    // 350 ms is enough for normal Win32/WPF/UWP editors to consume the data.
    sleep(Duration::from_millis(350));

    let restore_result = restore_clipboard(&snapshot);
    match (paste_result, restore_result) {
        (Err(err), _) => Err(format!("模拟 Ctrl+V 失败: {err}")),
        (Ok(()), Err(err)) => Ok(Some(format!("原剪贴板恢复失败: {err}"))),
        (Ok(()), Ok(())) => Ok(None),
    }
}

fn send_ctrl_v() -> Result<(), String> {
    fn key(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let inputs = [
        key(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key(VK_V, KEYBD_EVENT_FLAGS(0)),
        key(VK_V, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];

    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(format!("SendInput 只发送了 {sent}/{} 个事件", inputs.len()));
    }
    Ok(())
}

fn capture_clipboard() -> Result<Vec<ClipboardItem>, String> {
    open_clipboard_retry()?;
    let mut items = Vec::new();
    let mut format = 0u32;

    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if should_skip_format(format) {
            continue;
        }

        let handle = match unsafe { GetClipboardData(format) } {
            Ok(handle) => handle,
            Err(_) => continue,
        };
        let global = HGLOBAL(handle.0);
        let size = unsafe { GlobalSize(global) };
        if size == 0 {
            continue;
        }
        let pointer = unsafe { GlobalLock(global) };
        if pointer.is_null() {
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, size).to_vec() };
        unsafe {
            let _ = GlobalUnlock(global);
        }
        items.push(ClipboardItem { format, bytes });
    }

    let close_result = unsafe { CloseClipboard() };
    close_result.map_err(|e| format!("关闭剪贴板失败: {e}"))?;
    Ok(items)
}

fn restore_clipboard(items: &[ClipboardItem]) -> Result<(), String> {
    open_clipboard_retry()?;
    let empty_result = unsafe { EmptyClipboard() };
    if let Err(e) = empty_result {
        let _ = unsafe { CloseClipboard() };
        return Err(format!("清空剪贴板失败: {e}"));
    }

    let mut warnings = Vec::new();
    for item in items {
        match write_hglobal(item.format, &item.bytes) {
            Ok(()) => {}
            Err(err) => warnings.push(err),
        }
    }

    let close_result = unsafe { CloseClipboard() };
    close_result.map_err(|e| format!("关闭剪贴板失败: {e}"))?;

    if warnings.is_empty() {
        Ok(())
    } else {
        Err(warnings.join("；"))
    }
}

fn set_clipboard_text(text: &str) -> Result<(), String> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes_len = utf16.len() * 2;

    open_clipboard_retry()?;
    let result = (|| -> Result<(), String> {
        unsafe {
            EmptyClipboard().map_err(|e| format!("清空剪贴板失败: {e}"))?;
        }

        let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes_len) }
            .map_err(|_| "无法分配剪贴板内存".to_string())?;
        let pointer = unsafe { GlobalLock(global) };
        if pointer.is_null() {
            unsafe {
                let _ = windows::Win32::Foundation::GlobalFree(Some(global));
            }
            return Err("无法锁定剪贴板内存".to_string());
        }
        unsafe {
            copy_nonoverlapping(utf16.as_ptr() as *const u8, pointer as *mut u8, bytes_len);
            let _ = GlobalUnlock(global);
        }

        let handle = HANDLE(global.0);
        match unsafe { SetClipboardData(CF_UNICODETEXT, Some(handle)) } {
            Ok(_) => Ok(()),
            Err(e) => {
                unsafe {
                    let _ = windows::Win32::Foundation::GlobalFree(Some(global));
                }
                Err(format!("设置剪贴板文本失败: {e}"))
            }
        }
    })();

    let close_result = unsafe { CloseClipboard() };
    result?;
    close_result.map_err(|e| format!("关闭剪贴板失败: {e}"))
}

fn write_hglobal(format: u32, bytes: &[u8]) -> Result<(), String> {
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
        .map_err(|_| format!("无法恢复剪贴板格式 {format}"))?;
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        unsafe {
            let _ = windows::Win32::Foundation::GlobalFree(Some(global));
        }
        return Err(format!("无法锁定剪贴板格式 {format} 的内存"));
    }
    unsafe {
        copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
        let _ = GlobalUnlock(global);
    }

    let handle = HANDLE(global.0);
    match unsafe { SetClipboardData(format, Some(handle)) } {
        Ok(_) => Ok(()),
        Err(e) => {
            unsafe {
                let _ = windows::Win32::Foundation::GlobalFree(Some(global));
            }
            Err(format!("恢复格式 {format} 失败: {e}"))
        }
    }
}

fn open_clipboard_retry() -> Result<(), String> {
    for attempt in 0..10 {
        match unsafe { OpenClipboard(None) } {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 9 => sleep(Duration::from_millis(12)),
            Err(_) => break,
        }
    }
    Err("剪贴板正被其他程序占用".to_string())
}

fn should_skip_format(format: u32) -> bool {
    matches!(
        format,
        CF_BITMAP
            | CF_PALETTE
            | CF_METAFILEPICT
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
    ) || (CF_PRIVATEFIRST..=CF_PRIVATELAST).contains(&format)
}
