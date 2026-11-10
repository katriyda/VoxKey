//! Parsing and display for the hold-to-talk shortcut. The actual global hook
//! lives in `platform`.

pub const DEFAULT_HOTKEY: &str = "right_ctrl";

pub const MOD_CTRL: u8 = 1 << 0;
pub const MOD_SHIFT: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_WIN: u8 = 1 << 3;

// Win32 virtual-key codes used by the hook. Keeping numbers here avoids a
// dependency on the keyboard module for a pure configuration type.
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_SPACE: u32 = 0x20;
const VK_TAB: u32 = 0x09;
const VK_CAPITAL: u32 = 0x14;
const VK_ESCAPE: u32 = 0x1B;
const VK_OEM_3: u32 = 0xC0;
const VK_F1: u32 = 0x70;
const VK_F2: u32 = 0x71;
const VK_F3: u32 = 0x72;
const VK_F4: u32 = 0x73;
const VK_F5: u32 = 0x74;
const VK_F6: u32 = 0x75;
const VK_F7: u32 = 0x76;
const VK_F8: u32 = 0x77;
const VK_F9: u32 = 0x78;
const VK_F10: u32 = 0x79;
const VK_F11: u32 = 0x7A;
const VK_F12: u32 = 0x7B;
const VK_F13: u32 = 0x7C;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeySpec {
    pub trigger: u32,
    pub modifiers: u8,
    pub fn_alias: bool,
    pub label: String,
}

impl HotkeySpec {
    /// Some devices expose Fn as F13 (or an unusual scan code). This is best
    /// effort: most Fn keys are handled entirely by keyboard firmware and never
    /// reach Windows.
    pub fn matches_trigger(&self, vk: u32, scan_code: u32) -> bool {
        vk == self.trigger || (self.fn_alias && matches!(scan_code, 0x64 | 0x65))
    }

    pub fn family_mask(&self) -> u8 {
        modifier_family(self.trigger)
    }

    pub fn modifiers_match(&self, active_modifiers: u8) -> bool {
        // Ignore the modifier family of the trigger key itself. Otherwise
        // holding Right Ctrl would never satisfy a shortcut whose trigger is
        // Right Ctrl.
        let relevant = active_modifiers & !self.family_mask();
        relevant == self.modifiers
    }
}

pub fn parse_hotkey(input: &str) -> Result<HotkeySpec, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("快捷键不能为空".to_string());
    }

    let mut modifiers = 0u8;
    let mut pending: Option<HotkeySpec> = None;

    for token in trimmed.split('+').map(|s| s.trim()) {
        if token.is_empty() {
            return Err("快捷键格式无效".to_string());
        }
        let lower = token.to_ascii_lowercase();

        if pending.is_some() {
            return Err("快捷键只能包含一个主键".to_string());
        }

        if let Some(bit) = modifier_bit(&lower) {
            if modifiers & bit != 0 {
                return Err(format!("修饰键重复: {token}"));
            }
            modifiers |= bit;
            continue;
        }

        let (trigger, fn_alias) = trigger_code(&lower)
            .ok_or_else(|| format!("不支持的按键: {token}"))?;
        pending = Some(HotkeySpec {
            trigger,
            modifiers,
            fn_alias,
            label: String::new(),
        });
    }

    let mut spec = pending.ok_or_else(|| "快捷键缺少主键".to_string())?;
    if spec.trigger == 0 {
        return Err("快捷键缺少主键".to_string());
    }
    if spec.modifiers & spec.family_mask() != 0 {
        return Err("这个主键已经属于该修饰键，请换一个组合".to_string());
    }
    if spec.modifiers == 0
        && ((0x41..=0x5A).contains(&spec.trigger) || (0x30..=0x39).contains(&spec.trigger))
    {
        return Err("字母或数字作为主键时必须搭配修饰键".to_string());
    }
    spec.label = format_label(spec.trigger, spec.modifiers, spec.fn_alias);
    Ok(spec)
}

fn modifier_bit(name: &str) -> Option<u8> {
    match name {
        "ctrl" | "control" => Some(MOD_CTRL),
        "shift" => Some(MOD_SHIFT),
        "alt" => Some(MOD_ALT),
        "win" | "meta" | "super" => Some(MOD_WIN),
        _ => None,
    }
}

fn trigger_code(name: &str) -> Option<(u32, bool)> {
    let code = match name {
        "rightctrl" | "right_ctrl" | "rctrl" | "rc" => VK_RCONTROL,
        "leftctrl" | "left_ctrl" | "lctrl" => VK_LCONTROL,
        "rightalt" | "right_alt" | "ralt" => VK_RMENU,
        "leftalt" | "left_alt" | "lalt" => VK_LMENU,
        "rightshift" | "right_shift" | "rshift" => VK_RSHIFT,
        "leftshift" | "left_shift" | "lshift" => VK_LSHIFT,
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "capslock" | "caps_lock" => VK_CAPITAL,
        "escape" | "esc" => VK_ESCAPE,
        "backtick" | "grave" => VK_OEM_3,
        "fn" | "f13" => VK_F13,
        _ => {
            if let Some(number) = name.strip_prefix('f') {
                if let Ok(n) = number.parse::<u32>() {
                    if (1..=12).contains(&n) {
                        return Some((VK_F1 + n - 1, false));
                    }
                }
            }
            if name.len() == 1 {
                let ch = name.chars().next().unwrap();
                if ch.is_ascii_alphanumeric() {
                    return Some((ch.to_ascii_uppercase() as u32, false));
                }
            }
            return None;
        }
    };
    Some((code, name == "fn"))
}

pub fn format_label(trigger: u32, modifiers: u8, fn_alias: bool) -> String {
    let mut label = String::new();
    let mut pushed = false;
    for (bit, name) in [
        (MOD_CTRL, "Ctrl"),
        (MOD_SHIFT, "Shift"),
        (MOD_ALT, "Alt"),
        (MOD_WIN, "Win"),
    ] {
        if modifiers & bit != 0 {
            if pushed {
                label.push_str(" + ");
            }
            label.push_str(name);
            pushed = true;
        }
    }
    let key = match trigger {
        VK_RCONTROL => Some("右 Ctrl"),
        VK_LCONTROL => Some("左 Ctrl"),
        VK_RMENU => Some("右 Alt"),
        VK_LMENU => Some("左 Alt"),
        VK_RSHIFT => Some("右 Shift"),
        VK_LSHIFT => Some("左 Shift"),
        VK_SPACE => Some("空格"),
        VK_TAB => Some("Tab"),
        VK_CAPITAL => Some("Caps Lock"),
        VK_ESCAPE => Some("Esc"),
        VK_OEM_3 => Some("`"),
        VK_F1 => Some("F1"),
        VK_F2 => Some("F2"),
        VK_F3 => Some("F3"),
        VK_F4 => Some("F4"),
        VK_F5 => Some("F5"),
        VK_F6 => Some("F6"),
        VK_F7 => Some("F7"),
        VK_F8 => Some("F8"),
        VK_F9 => Some("F9"),
        VK_F10 => Some("F10"),
        VK_F11 => Some("F11"),
        VK_F12 => Some("F12"),
        VK_F13 if fn_alias => Some("Fn / F13"),
        VK_F13 => Some("F13"),
       0x41..=0x5A => None,
       0x30..=0x39 => None,
        _ => None,
    };
    let key_label = key.unwrap_or_else(|| {
        if (0x41..=0x5A).contains(&trigger) || (0x30..=0x39).contains(&trigger) {
            char::from_u32(trigger).map(|c| c.to_string()).unwrap_or_default()
        } else {
            format!("VK 0x{trigger:02X}")
        }
    });
    if pushed {
        label.push_str(" + ");
    }
    label.push_str(&key_label);
    label
}

pub fn modifier_family(vk: u32) -> u8 {
    match vk {
        VK_LSHIFT | VK_RSHIFT | 0x10 => MOD_SHIFT,
        VK_LCONTROL | VK_RCONTROL | 0x11 => MOD_CTRL,
        VK_LMENU | VK_RMENU | 0x12 => MOD_ALT,
        VK_LWIN | VK_RWIN => MOD_WIN,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default() {
        let spec = parse_hotkey(DEFAULT_HOTKEY).unwrap();
        assert_eq!(spec.trigger, VK_RCONTROL);
        assert_eq!(spec.label, "右 Ctrl");
        assert!(spec.modifiers_match(MOD_CTRL));
        assert!(!spec.modifiers_match(MOD_CTRL | MOD_SHIFT));
    }

    #[test]
    fn parses_chord() {
        let spec = parse_hotkey("ctrl+shift+space").unwrap();
        assert_eq!(spec.trigger, VK_SPACE);
        assert_eq!(spec.modifiers, MOD_CTRL | MOD_SHIFT);
        assert!(spec.modifiers_match(MOD_CTRL | MOD_SHIFT));
        assert_eq!(spec.label, "Ctrl + Shift + 空格");
    }

    #[test]
    fn fn_alias() {
        let spec = parse_hotkey("fn").unwrap();
        assert!(spec.fn_alias);
        assert!(spec.matches_trigger(VK_F13, 0));
        assert!(spec.matches_trigger(0, 0x64));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("ctrl").is_err());
        assert!(parse_hotkey("ctrl+ctrl+space").is_err());
        assert!(parse_hotkey("space+a").is_err());
    }
}
