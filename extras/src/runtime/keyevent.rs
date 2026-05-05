use crossterm::event::{
    KeyCode as CKey, KeyEvent as CKeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    MediaKeyCode, ModifierKeyCode,
};
use wezterm_input_types::{KeyCode as WKey, KeyEvent as WKeyEvent, KeyboardLedStatus, Modifiers};

/// 将 crossterm 的 `KeyEvent` 转换为 wezterm 对应的 `KeyEvent`。
/// 对于无法识别的按键（如某些 RawCode、Void 等），返回 `None`。
pub fn convert_key_event(ev: CKeyEvent) -> Option<WKeyEvent> {
    // 先转换基础修饰符
    let mut modifiers = convert_modifiers(ev.modifiers);

    // 特殊处理 BackTab（Shift+Tab）
    if ev.code == CKey::BackTab {
        modifiers.insert(Modifiers::SHIFT);
    }

    // 转换按键码（BackTab 已经映射为 Tab）
    let key = convert_keycode(ev.code)?;

    // 转换 LED 状态（仅保留 CapsLock / NumLock）
    let leds = convert_leds(ev.state);

    // 按下/释放处理，重复事件视为按下
    let key_is_down = matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat);

    // 重复次数：crossterm 不提供具体数值，固定为 1
    let repeat_count = 1;

    let raw = None; // crossterm 不暴露原始事件

    Some(WKeyEvent {
        key,
        modifiers,
        leds,
        repeat_count,
        key_is_down,
        raw,
    })
}

fn convert_modifiers(mods: KeyModifiers) -> Modifiers {
    let mut out = Modifiers::empty();
    // Crossterm 使用紧凑位，wezterm 使用扩展位
    if mods.contains(KeyModifiers::SHIFT) {
        out.insert(Modifiers::SHIFT);
    }
    if mods.contains(KeyModifiers::CONTROL) {
        out.insert(Modifiers::CTRL);
    }
    if mods.contains(KeyModifiers::ALT) {
        out.insert(Modifiers::ALT);
    }
    if mods.contains(KeyModifiers::SUPER) {
        out.insert(Modifiers::SUPER);
    }
    // wezterm 无 Hyper/Meta，忽略
    out
}

fn convert_leds(state: KeyEventState) -> KeyboardLedStatus {
    let mut leds = KeyboardLedStatus::empty();
    if state.contains(KeyEventState::CAPS_LOCK) {
        leds.insert(KeyboardLedStatus::CAPS_LOCK);
    }
    if state.contains(KeyEventState::NUM_LOCK) {
        leds.insert(KeyboardLedStatus::NUM_LOCK);
    }
    leds
}

fn convert_keycode(code: CKey) -> Option<WKey> {
    use CKey::*;
    Some(match code {
        Backspace => WKey::Char('\x08'), // wezterm 约定：Backspace = Char(0x08)
        Enter => WKey::Char('\r'),
        Tab => WKey::Char('\t'),
        Esc => WKey::Char('\x1b'),
        Delete => WKey::Char('\x7f'),
        BackTab => WKey::Char('\t'), // 外部会加 SHIFT 修饰符
        Left => WKey::LeftArrow,
        Right => WKey::RightArrow,
        Up => WKey::UpArrow,
        Down => WKey::DownArrow,
        Home => WKey::Home,
        End => WKey::End,
        PageUp => WKey::PageUp,
        PageDown => WKey::PageDown,
        Insert => WKey::Insert,
        F(n) => WKey::Function(n),
        Char(c) => WKey::Char(c),
        Null => return None,
        CapsLock => WKey::CapsLock,
        ScrollLock => WKey::ScrollLock,
        NumLock => WKey::NumLock,
        PrintScreen => WKey::PrintScreen,
        Pause => WKey::Pause,
        Menu => WKey::Char('\x1b'), // 无法直接映射，回退忽略 (见下文)
        KeypadBegin => WKey::KeyPadBegin,
        Media(media) => match media {
            MediaKeyCode::Play | MediaKeyCode::Pause | MediaKeyCode::PlayPause => {
                WKey::MediaPlayPause
            }
            MediaKeyCode::Stop => WKey::MediaStop,
            MediaKeyCode::TrackNext => WKey::MediaNextTrack,
            MediaKeyCode::TrackPrevious => WKey::MediaPrevTrack,
            MediaKeyCode::LowerVolume => WKey::VolumeDown,
            MediaKeyCode::RaiseVolume => WKey::VolumeUp,
            MediaKeyCode::MuteVolume => WKey::VolumeMute,
            _ => return None,
        },
        Modifier(modk) => match modk {
            ModifierKeyCode::LeftShift => WKey::LeftShift,
            ModifierKeyCode::RightShift => WKey::RightShift,
            ModifierKeyCode::LeftControl => WKey::LeftControl,
            ModifierKeyCode::RightControl => WKey::RightControl,
            ModifierKeyCode::LeftAlt => WKey::LeftAlt,
            ModifierKeyCode::RightAlt => WKey::RightAlt,
            ModifierKeyCode::LeftSuper => WKey::LeftWindows,
            ModifierKeyCode::RightSuper => WKey::RightWindows,
            ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper => WKey::Hyper,
            ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta => WKey::Meta,
            _ => return None,
        },
    })
}
