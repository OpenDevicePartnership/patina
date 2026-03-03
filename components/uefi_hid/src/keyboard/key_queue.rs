//! Key queue support for HID driver.
//!
//! Manages pending keystrokes, keyboard state, and translates between HID
//! usages and EFI keyboard primitives using the active HII keyboard layout.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::{
    collections::{BTreeSet, VecDeque},
    vec::Vec,
};
use core::ops::Deref;
use hidparser::report_data_types::Usage;

use crate::keyboard::layout::{EfiKey, HiiKey, HiiKeyboardLayout, HiiNsKeyDescriptor};

use r_efi::protocols::{self, hii_database::*, simple_text_input::InputKey, simple_text_input_ex::*};

// The set of HID usages that represent modifier keys this driver is interested in.
#[rustfmt::skip]
const KEYBOARD_MODIFIERS: &[u16] = &[
  LEFT_CONTROL_MODIFIER, RIGHT_CONTROL_MODIFIER, LEFT_SHIFT_MODIFIER, RIGHT_SHIFT_MODIFIER, LEFT_ALT_MODIFIER,
  RIGHT_ALT_MODIFIER, LEFT_LOGO_MODIFIER, RIGHT_LOGO_MODIFIER, MENU_MODIFIER, PRINT_MODIFIER, SYS_REQUEST_MODIFIER,
  ALT_GR_MODIFIER,
];

// The set of HID usages that represent modifier keys that toggle state (as opposed to remain active while pressed).
const TOGGLE_MODIFIERS: &[u16] = &[NUM_LOCK_MODIFIER, CAPS_LOCK_MODIFIER, SCROLL_LOCK_MODIFIER];

// Shift modifiers.
const SHIFT_MODIFIERS: &[u16] = &[LEFT_SHIFT_MODIFIER, RIGHT_SHIFT_MODIFIER];

// Mapping from HID modifier to the corresponding key_shift_state flag.
#[rustfmt::skip]
const SHIFT_STATE_MAP: &[(u16, u32)] = &[
    (LEFT_CONTROL_MODIFIER,  LEFT_CONTROL_PRESSED),
    (RIGHT_CONTROL_MODIFIER, RIGHT_CONTROL_PRESSED),
    (LEFT_ALT_MODIFIER,      LEFT_ALT_PRESSED),
    (RIGHT_ALT_MODIFIER,     RIGHT_ALT_PRESSED),
    (LEFT_SHIFT_MODIFIER,    LEFT_SHIFT_PRESSED),
    (RIGHT_SHIFT_MODIFIER,   RIGHT_SHIFT_PRESSED),
    (LEFT_LOGO_MODIFIER,     LEFT_LOGO_PRESSED),
    (RIGHT_LOGO_MODIFIER,    RIGHT_LOGO_PRESSED),
    (MENU_MODIFIER,          MENU_KEY_PRESSED),
    (SYS_REQUEST_MODIFIER,   SYS_REQ_PRESSED),
    (PRINT_MODIFIER,         SYS_REQ_PRESSED),
];

// Mapping from key_toggle_state flag to the corresponding HID modifier.
#[rustfmt::skip]
const TOGGLE_STATE_MAP: &[(KeyToggleState, u16)] = &[
    (SCROLL_LOCK_ACTIVE, SCROLL_LOCK_MODIFIER),
    (NUM_LOCK_ACTIVE,    NUM_LOCK_MODIFIER),
    (CAPS_LOCK_ACTIVE,   CAPS_LOCK_MODIFIER),
];

/// Defines whether a key stroke represents a key being pressed (KeyDown) or released (KeyUp)
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// Key is being pressed
    KeyDown,
    /// Key is being released
    KeyUp,
}

/// A wrapper for the KeyData type that allows definition of the Ord trait and additional registration matching logic.
#[derive(Debug, Clone)]
pub(crate) struct OrdKeyData(pub protocols::simple_text_input_ex::KeyData);

impl Deref for OrdKeyData {
    type Target = protocols::simple_text_input_ex::KeyData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Ord for OrdKeyData {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let e = self.key.unicode_char.cmp(&other.key.unicode_char);
        if !e.is_eq() {
            return e;
        }
        let e = self.key.scan_code.cmp(&other.key.scan_code);
        if !e.is_eq() {
            return e;
        }
        let e = self.key_state.key_shift_state.cmp(&other.key_state.key_shift_state);
        if !e.is_eq() {
            return e;
        }
        self.key_state.key_toggle_state.cmp(&other.key_state.key_toggle_state)
    }
}

impl PartialOrd for OrdKeyData {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for OrdKeyData {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for OrdKeyData {}

impl OrdKeyData {
    // Returns whether this key matches the given registration. Note that this is not a straight compare - UEFI spec
    // allows for some degree of wildcard matching. Refer to UEFI spec 2.10 section 12.2.5.
    pub(crate) fn matches_registered_key(&self, registration: &Self) -> bool {
        // char and scan must match (per the reference implementation in the EDK2 C code).
        self.key.unicode_char == registration.key.unicode_char
            && self.key.scan_code == registration.key.scan_code
            // shift state must be zero (wildcard) or must match.
            && (registration.key_state.key_shift_state == 0
                || registration.key_state.key_shift_state == self.key_state.key_shift_state)
            // toggle state must be zero (wildcard) or must match.
            && (registration.key_state.key_toggle_state == 0
                || registration.key_state.key_toggle_state == self.key_state.key_toggle_state)
    }
}

/// Manages the queue of pending keystrokes.
#[derive(Debug, Default)]
pub(crate) struct KeyQueue {
    layout: Option<HiiKeyboardLayout>,
    active_modifiers: BTreeSet<u16>,
    active_ns_key: Option<HiiNsKeyDescriptor>,
    partial_key_support_active: bool,
    key_queue: VecDeque<KeyData>,
    registered_keys: BTreeSet<OrdKeyData>,
    notified_key_queue: VecDeque<KeyData>,
}

impl KeyQueue {
    /// Resets the key queue to its initial state.
    pub(crate) fn reset(&mut self, extended_reset: bool) {
        if extended_reset {
            self.active_modifiers.clear();
        } else {
            self.active_modifiers.retain(|x| modifier_to_led_usage(*x).is_some());
        }
        self.active_ns_key = None;
        self.partial_key_support_active = false;
        self.key_queue.clear();
    }

    /// Processes a keystroke and updates the key queue.
    pub(crate) fn keystroke(&mut self, key: Usage, action: KeyAction) {
        log::trace!("keystroke: usage 0x{:08X}, action {:?}", u32::from(key), action);
        let Some(ref active_layout) = self.layout else {
            //nothing to do if no layout. This is unexpected: layout should be initialized with default if not present.
            log::warn!("key_queue::keystroke: Received keystroke without layout.");
            return;
        };

        let Some(efi_key) = usage_to_efi_key(key) else {
            //unsupported key usage, nothing to do.
            return;
        };

        // Check if it is a dependent key of a currently active "non-spacing" (ns) key.
        // Non-spacing key handling is described in UEFI spec 2.10 section 33.2.4.3.
        let mut current_descriptor = None;
        if let Some(ref ns_key) = self.active_ns_key {
            for descriptor in &ns_key.dependent_keys {
                if descriptor.key == efi_key {
                    // found a dependent key for a previously active ns key.
                    // de-activate the ns key and process the dependent descriptor.
                    current_descriptor = Some(*descriptor);
                    self.active_ns_key = None;
                    break;
                }
            }
        }

        // If it is not a dependent key of a currently active ns key, then check if it is a regular or ns key.
        if current_descriptor.is_none() {
            for key in &active_layout.keys {
                match key {
                    HiiKey::Key(descriptor) if descriptor.key == efi_key => {
                        current_descriptor = Some(*descriptor);
                        break;
                    }
                    HiiKey::NsKey(ns_descriptor) if ns_descriptor.descriptor.key == efi_key => {
                        // if it is an ns_key, set it as the active ns key, and no further processing is needed.
                        self.active_ns_key = Some(ns_descriptor.clone());
                        return;
                    }
                    _ => continue,
                }
            }
        }

        let Some(current_descriptor) = current_descriptor else {
            return; //could not find descriptor, nothing to do.
        };

        //handle modifiers that are active as long as they are pressed
        if KEYBOARD_MODIFIERS.contains(&current_descriptor.modifier) {
            match action {
                KeyAction::KeyDown => {
                    self.active_modifiers.insert(current_descriptor.modifier);
                    log::trace!("keystroke: modifier 0x{:04X} pressed", current_descriptor.modifier);
                }
                KeyAction::KeyUp => {
                    self.active_modifiers.remove(&current_descriptor.modifier);
                    log::trace!("keystroke: modifier 0x{:04X} released", current_descriptor.modifier);
                }
            }
        }

        //handle modifiers that toggle each time the key is pressed.
        if TOGGLE_MODIFIERS.contains(&current_descriptor.modifier) && action == KeyAction::KeyDown {
            if self.active_modifiers.contains(&current_descriptor.modifier) {
                self.active_modifiers.remove(&current_descriptor.modifier);
                log::trace!("keystroke: toggle modifier 0x{:04X} deactivated", current_descriptor.modifier);
            } else {
                self.active_modifiers.insert(current_descriptor.modifier);
                log::trace!("keystroke: toggle modifier 0x{:04X} activated", current_descriptor.modifier);
            }
        }

        if action == KeyAction::KeyUp {
            //nothing else to do.
            return;
        }

        // process the keystroke to construct a KeyData item to add to the queue.
        let mut key_data = protocols::simple_text_input_ex::KeyData {
            key: InputKey {
                unicode_char: current_descriptor.unicode,
                scan_code: modifier_to_scan(current_descriptor.modifier),
            },
            ..Default::default()
        };

        // retrieve relevant modifier state that may need to be applied to the key data.
        let shift_active = SHIFT_MODIFIERS.iter().any(|x| self.active_modifiers.contains(x));
        let alt_gr_active = self.active_modifiers.contains(&ALT_GR_MODIFIER);
        let caps_lock_active = self.active_modifiers.contains(&CAPS_LOCK_MODIFIER);
        let num_lock_active = self.active_modifiers.contains(&NUM_LOCK_MODIFIER);

        // Apply the shift modifier if needed. shift_applied tracks whether shift was consumed so it can be removed from
        // the key state later (see UEFI spec 2.10 section 12.2.3).
        let affected_by_shift = (current_descriptor.affected_attribute & AFFECTED_BY_STANDARD_SHIFT) != 0;
        let shift_applied = affected_by_shift && shift_active;

        if affected_by_shift {
            match (shift_active, alt_gr_active) {
                (true, true) => key_data.key.unicode_char = current_descriptor.shifted_alt_gr_unicode,
                (true, false) => key_data.key.unicode_char = current_descriptor.shifted_unicode,
                (false, true) => key_data.key.unicode_char = current_descriptor.alt_gr_unicode,
                (false, false) => {} // unicode_char already set to default
            }
        }

        // if capslock is active, then invert the shift state of the key.
        if (current_descriptor.affected_attribute & AFFECTED_BY_CAPS_LOCK) != 0 && caps_lock_active {
            //Note: reference EDK2 implementation does not apply capslock to alt_gr.
            if key_data.key.unicode_char == current_descriptor.unicode {
                key_data.key.unicode_char = current_descriptor.shifted_unicode;
            } else if key_data.key.unicode_char == current_descriptor.shifted_unicode {
                key_data.key.unicode_char = current_descriptor.unicode;
            }
        }

        // for the num pad, numlock (and shift state) controls whether a number key or a control key (e.g. arrow) is queued.
        if (current_descriptor.affected_attribute & AFFECTED_BY_NUM_LOCK) != 0 {
            if num_lock_active && !shift_active {
                key_data.key.scan_code = SCAN_NULL;
            } else {
                key_data.key.unicode_char = 0x0000;
            }
        }

        //special handling for unicode ESC (0x1B).
        const ESC_UNICODE: u16 = 0x1B;
        if key_data.key.unicode_char == ESC_UNICODE && key_data.key.scan_code == SCAN_NULL {
            key_data.key.scan_code = SCAN_ESC;
            key_data.key.unicode_char = 0x0000;
        }

        if !self.partial_key_support_active && key_data.key.unicode_char == 0 && key_data.key.scan_code == SCAN_NULL {
            return; // no further processing required if there is no key or scancode and partial support is not active.
        }

        //initialize key state from active modifiers
        key_data.key_state = self.init_key_state();

        // if shift was applied above, then remove shift from key state. See UEFI spec 2.10 section 12.2.3.
        if shift_applied {
            key_data.key_state.key_shift_state &= !(LEFT_SHIFT_PRESSED | RIGHT_SHIFT_PRESSED);
        }

        // if a callback has been registered matching this key, enqueue it in the callback queue.
        if self.is_registered_key(key_data) {
            self.notified_key_queue.push_back(key_data);
        }

        // enqueue the key data.
        log::trace!(
            "keystroke: enqueuing key unicode=0x{:04X} scan=0x{:04X}",
            key_data.key.unicode_char,
            key_data.key.scan_code,
        );
        self.key_queue.push_back(key_data);
    }

    // Returns true if the key matches any registered notification key.
    fn is_registered_key(&self, current_key: KeyData) -> bool {
        for registered_key in &self.registered_keys {
            if OrdKeyData(current_key).matches_registered_key(registered_key) {
                return true;
            }
        }
        false
    }

    /// Returns a new KeyState reflecting the current modifier state.
    pub(crate) fn init_key_state(&self) -> KeyState {
        let key_shift_state = SHIFT_STATE_MAP
            .iter()
            .filter(|(modifier, _)| self.active_modifiers.contains(modifier))
            .fold(SHIFT_STATE_VALID, |state, (_, pressed)| state | pressed);

        let key_toggle_state = TOGGLE_STATE_MAP
            .iter()
            .filter(|(_, modifier)| self.active_modifiers.contains(modifier))
            .fold(TOGGLE_STATE_VALID, |state, (flag, _)| state | flag)
            | if self.partial_key_support_active { KEY_STATE_EXPOSED } else { 0 };
        KeyState { key_shift_state, key_toggle_state }
    }

    /// Removes and returns the next pending keystroke.
    pub(crate) fn pop_key(&mut self) -> Option<KeyData> {
        self.key_queue.pop_front()
    }

    /// Returns the next pending keystroke without removing it.
    pub(crate) fn peek_key(&self) -> Option<KeyData> {
        self.key_queue.front().cloned()
    }

    /// Removes and returns the next pending notify keystroke.
    pub(crate) fn pop_notify_key(&mut self) -> Option<KeyData> {
        self.notified_key_queue.pop_front()
    }

    /// Returns the next pending notify keystroke without removing it.
    pub(crate) fn peek_notify_key(&self) -> Option<KeyData> {
        self.notified_key_queue.front().cloned()
    }

    /// Sets the toggle state for scroll/caps/num lock and partial key exposure.
    pub(crate) fn set_key_toggle_state(&mut self, toggle_state: KeyToggleState) {
        log::trace!("set_key_toggle_state: 0x{:02X}", toggle_state);
        for &(flag, modifier) in TOGGLE_STATE_MAP {
            if (toggle_state & flag) != 0 {
                self.active_modifiers.insert(modifier);
            } else {
                self.active_modifiers.remove(&modifier);
            }
        }

        self.partial_key_support_active = (toggle_state & KEY_STATE_EXPOSED) != 0;
    }

    /// Returns the HID LED usages corresponding to active toggle modifiers.
    pub(crate) fn active_leds(&self) -> Vec<Usage> {
        self.active_modifiers.iter().copied().filter_map(modifier_to_led_usage).collect()
    }

    /// Returns the current keyboard layout.
    pub(crate) fn layout(&self) -> Option<HiiKeyboardLayout> {
        self.layout.clone()
    }

    /// Sets the keyboard layout used for keystroke translation.
    pub(crate) fn set_layout(&mut self, new_layout: Option<HiiKeyboardLayout>) {
        self.layout = new_layout;
    }

    /// Registers a key for notification matching.
    pub(crate) fn add_notify_key(&mut self, key_data: OrdKeyData) {
        self.registered_keys.insert(key_data);
    }

    /// Unregisters a notification key.
    pub(crate) fn remove_notify_key(&mut self, key_data: &OrdKeyData) {
        self.registered_keys.remove(key_data);
    }
}

// Helper routine that converts a HID Usage to the corresponding EfiKey.
fn usage_to_efi_key(usage: Usage) -> Option<EfiKey> {
    //Refer to UEFI spec version 2.10 figure 34.3
    match usage.into() {
        0x00070001..=0x00070003 => None, //Keyboard error codes.
        0x00070004 => Some(EfiKey::C1),
        0x00070005 => Some(EfiKey::B5),
        0x00070006 => Some(EfiKey::B3),
        0x00070007 => Some(EfiKey::C3),
        0x00070008 => Some(EfiKey::D3),
        0x00070009 => Some(EfiKey::C4),
        0x0007000A => Some(EfiKey::C5),
        0x0007000B => Some(EfiKey::C6),
        0x0007000C => Some(EfiKey::D8),
        0x0007000D => Some(EfiKey::C7),
        0x0007000E => Some(EfiKey::C8),
        0x0007000F => Some(EfiKey::C9),
        0x00070010 => Some(EfiKey::B7),
        0x00070011 => Some(EfiKey::B6),
        0x00070012 => Some(EfiKey::D9),
        0x00070013 => Some(EfiKey::D10),
        0x00070014 => Some(EfiKey::D1),
        0x00070015 => Some(EfiKey::D4),
        0x00070016 => Some(EfiKey::C2),
        0x00070017 => Some(EfiKey::D5),
        0x00070018 => Some(EfiKey::D7),
        0x00070019 => Some(EfiKey::B4),
        0x0007001A => Some(EfiKey::D2),
        0x0007001B => Some(EfiKey::B2),
        0x0007001C => Some(EfiKey::D6),
        0x0007001D => Some(EfiKey::B1),
        0x0007001E => Some(EfiKey::E1),
        0x0007001F => Some(EfiKey::E2),
        0x00070020 => Some(EfiKey::E3),
        0x00070021 => Some(EfiKey::E4),
        0x00070022 => Some(EfiKey::E5),
        0x00070023 => Some(EfiKey::E6),
        0x00070024 => Some(EfiKey::E7),
        0x00070025 => Some(EfiKey::E8),
        0x00070026 => Some(EfiKey::E9),
        0x00070027 => Some(EfiKey::E10),
        0x00070028 => Some(EfiKey::Enter),
        0x00070029 => Some(EfiKey::Esc),
        0x0007002A => Some(EfiKey::BackSpace),
        0x0007002B => Some(EfiKey::Tab),
        0x0007002C => Some(EfiKey::SpaceBar),
        0x0007002D => Some(EfiKey::E11),
        0x0007002E => Some(EfiKey::E12),
        0x0007002F => Some(EfiKey::D11),
        0x00070030 => Some(EfiKey::D12),
        0x00070031 => Some(EfiKey::D13),
        0x00070032 => Some(EfiKey::C12),
        0x00070033 => Some(EfiKey::C10),
        0x00070034 => Some(EfiKey::C11),
        0x00070035 => Some(EfiKey::E0),
        0x00070036 => Some(EfiKey::B8),
        0x00070037 => Some(EfiKey::B9),
        0x00070038 => Some(EfiKey::B10),
        0x00070039 => Some(EfiKey::CapsLock),
        0x0007003A => Some(EfiKey::F1),
        0x0007003B => Some(EfiKey::F2),
        0x0007003C => Some(EfiKey::F3),
        0x0007003D => Some(EfiKey::F4),
        0x0007003E => Some(EfiKey::F5),
        0x0007003F => Some(EfiKey::F6),
        0x00070040 => Some(EfiKey::F7),
        0x00070041 => Some(EfiKey::F8),
        0x00070042 => Some(EfiKey::F9),
        0x00070043 => Some(EfiKey::F10),
        0x00070044 => Some(EfiKey::F11),
        0x00070045 => Some(EfiKey::F12),
        0x00070046 => Some(EfiKey::Print),
        0x00070047 => Some(EfiKey::SLck),
        0x00070048 => Some(EfiKey::Pause),
        0x00070049 => Some(EfiKey::Ins),
        0x0007004A => Some(EfiKey::Home),
        0x0007004B => Some(EfiKey::PgUp),
        0x0007004C => Some(EfiKey::Del),
        0x0007004D => Some(EfiKey::End),
        0x0007004E => Some(EfiKey::PgDn),
        0x0007004F => Some(EfiKey::RightArrow),
        0x00070050 => Some(EfiKey::LeftArrow),
        0x00070051 => Some(EfiKey::DownArrow),
        0x00070052 => Some(EfiKey::UpArrow),
        0x00070053 => Some(EfiKey::NLck),
        0x00070054 => Some(EfiKey::Slash),
        0x00070055 => Some(EfiKey::Asterisk),
        0x00070056 => Some(EfiKey::Minus),
        0x00070057 => Some(EfiKey::Plus),
        0x00070058 => Some(EfiKey::Enter),
        0x00070059 => Some(EfiKey::One),
        0x0007005A => Some(EfiKey::Two),
        0x0007005B => Some(EfiKey::Three),
        0x0007005C => Some(EfiKey::Four),
        0x0007005D => Some(EfiKey::Five),
        0x0007005E => Some(EfiKey::Six),
        0x0007005F => Some(EfiKey::Seven),
        0x00070060 => Some(EfiKey::Eight),
        0x00070061 => Some(EfiKey::Nine),
        0x00070062 => Some(EfiKey::Zero),
        0x00070063 => Some(EfiKey::Period),
        0x00070064 => Some(EfiKey::B0),
        0x00070065 => Some(EfiKey::A4),
        0x00070066..=0x000700DF => None, // not used by EFI keyboard layout.
        0x000700E0 => Some(EfiKey::LCtrl),
        0x000700E1 => Some(EfiKey::LShift),
        0x000700E2 => Some(EfiKey::LAlt),
        0x000700E3 => Some(EfiKey::A0),
        0x000700E4 => Some(EfiKey::RCtrl),
        0x000700E5 => Some(EfiKey::RShift),
        0x000700E6 => Some(EfiKey::A2),
        0x000700E7 => Some(EfiKey::A3),
        _ => None, // all other usages not used by EFI keyboard layout.
    }
}

//These should be defined in r_efi::protocols::simple_text_input
pub const SCAN_NULL: u16 = 0x0000;
pub const SCAN_UP: u16 = 0x0001;
pub const SCAN_DOWN: u16 = 0x0002;
pub const SCAN_RIGHT: u16 = 0x0003;
pub const SCAN_LEFT: u16 = 0x0004;
pub const SCAN_HOME: u16 = 0x0005;
pub const SCAN_END: u16 = 0x0006;
pub const SCAN_INSERT: u16 = 0x0007;
pub const SCAN_DELETE: u16 = 0x0008;
pub const SCAN_PAGE_UP: u16 = 0x0009;
pub const SCAN_PAGE_DOWN: u16 = 0x000A;
pub const SCAN_F1: u16 = 0x000B;
pub const SCAN_F2: u16 = 0x000C;
pub const SCAN_F3: u16 = 0x000D;
pub const SCAN_F4: u16 = 0x000E;
pub const SCAN_F5: u16 = 0x000F;
pub const SCAN_F6: u16 = 0x0010;
pub const SCAN_F7: u16 = 0x0011;
pub const SCAN_F8: u16 = 0x0012;
pub const SCAN_F9: u16 = 0x0013;
pub const SCAN_F10: u16 = 0x0014;
pub const SCAN_F11: u16 = 0x0015;
pub const SCAN_F12: u16 = 0x0016;
pub const SCAN_ESC: u16 = 0x0017;
pub const SCAN_PAUSE: u16 = 0x0048;

// helper routine that converts the given modifier to the corresponding SCAN code
fn modifier_to_scan(modifier: u16) -> u16 {
    match modifier {
        INSERT_MODIFIER => SCAN_INSERT,
        DELETE_MODIFIER => SCAN_DELETE,
        PAGE_DOWN_MODIFIER => SCAN_PAGE_DOWN,
        PAGE_UP_MODIFIER => SCAN_PAGE_UP,
        HOME_MODIFIER => SCAN_HOME,
        END_MODIFIER => SCAN_END,
        LEFT_ARROW_MODIFIER => SCAN_LEFT,
        RIGHT_ARROW_MODIFIER => SCAN_RIGHT,
        DOWN_ARROW_MODIFIER => SCAN_DOWN,
        UP_ARROW_MODIFIER => SCAN_UP,
        FUNCTION_KEY_ONE_MODIFIER => SCAN_F1,
        FUNCTION_KEY_TWO_MODIFIER => SCAN_F2,
        FUNCTION_KEY_THREE_MODIFIER => SCAN_F3,
        FUNCTION_KEY_FOUR_MODIFIER => SCAN_F4,
        FUNCTION_KEY_FIVE_MODIFIER => SCAN_F5,
        FUNCTION_KEY_SIX_MODIFIER => SCAN_F6,
        FUNCTION_KEY_SEVEN_MODIFIER => SCAN_F7,
        FUNCTION_KEY_EIGHT_MODIFIER => SCAN_F8,
        FUNCTION_KEY_NINE_MODIFIER => SCAN_F9,
        FUNCTION_KEY_TEN_MODIFIER => SCAN_F10,
        FUNCTION_KEY_ELEVEN_MODIFIER => SCAN_F11,
        FUNCTION_KEY_TWELVE_MODIFIER => SCAN_F12,
        PAUSE_MODIFIER => SCAN_PAUSE,
        _ => SCAN_NULL,
    }
}

// helper routine that converts the given modifier to the corresponding HID Usage.
fn modifier_to_led_usage(modifier: u16) -> Option<Usage> {
    match modifier {
        NUM_LOCK_MODIFIER => Some(Usage::from(0x00080001)),
        CAPS_LOCK_MODIFIER => Some(Usage::from(0x00080002)),
        SCROLL_LOCK_MODIFIER => Some(Usage::from(0x00080003)),
        _ => None,
    }
}

#[cfg(test)]
mod test {

    use hidparser::report_data_types::Usage;
    use r_efi::protocols::{
        self,
        hii_database::{
            AFFECTED_BY_CAPS_LOCK, AFFECTED_BY_STANDARD_SHIFT, NS_KEY_DEPENDENCY_MODIFIER, NS_KEY_MODIFIER,
        },
    };

    use crate::keyboard::layout::{EfiKey, HiiKey, HiiKeyDescriptor, HiiNsKeyDescriptor};

    use crate::keyboard::key_queue::{OrdKeyData, SCAN_DOWN, SCAN_END, SCAN_ESC, SCAN_NULL};

    use super::KeyQueue;

    // HID usages for keys used in tests.
    fn usage_a() -> Usage {
        Usage::from(0x00070004u32)
    } // EfiKey::C1 = 'a'/'A'
    fn usage_1() -> Usage {
        Usage::from(0x0007001Eu32)
    } // EfiKey::E1 = '1'/'!'
    fn usage_esc() -> Usage {
        Usage::from(0x00070029u32)
    } // EfiKey::Esc
    fn usage_lshift() -> Usage {
        Usage::from(0x000700E1u32)
    } // EfiKey::LShift
    fn usage_lctrl() -> Usage {
        Usage::from(0x000700E0u32)
    } // EfiKey::LCtrl
    fn usage_capslock() -> Usage {
        Usage::from(0x00070039u32)
    } // EfiKey::CapsLock
    fn usage_numlock() -> Usage {
        Usage::from(0x00070053u32)
    } // EfiKey::NLck
    fn usage_scrolllock() -> Usage {
        Usage::from(0x00070047u32)
    } // EfiKey::SLck
    fn usage_numpad1() -> Usage {
        Usage::from(0x00070059u32)
    } // EfiKey::One (numpad)

    fn key_queue_with_default_layout() -> KeyQueue {
        let mut kq = KeyQueue::default();
        kq.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        kq
    }

    fn press_key(kq: &mut KeyQueue, usage: Usage) {
        kq.keystroke(usage, super::KeyAction::KeyDown);
    }

    fn release_key(kq: &mut KeyQueue, usage: Usage) {
        kq.keystroke(usage, super::KeyAction::KeyUp);
    }

    fn tap_key(kq: &mut KeyQueue, usage: Usage) {
        press_key(kq, usage);
        release_key(kq, usage);
    }

    // convenience macro for defining HiiKeyDescriptor structures.
    // note: for unicode characters, these are encoded as u16 for compliance with UEFI spec. UEFI only supports UCS-2
    // encoding - so unicode characters that require more than two bytes under UTF-16 are not supported (and will panic).
    macro_rules! key_descriptor {
        ($key:expr, $unicode:literal, $shifted:literal, $alt_gr:literal, $shifted_alt_gr:literal, $modifier:expr, $affected:expr ) => {
            HiiKeyDescriptor {
                key: $key,
                unicode: $unicode.encode_utf16(&mut [0u16; 1])[0],
                shifted_unicode: $shifted.encode_utf16(&mut [0u16; 1])[0],
                alt_gr_unicode: $alt_gr.encode_utf16(&mut [0u16; 1])[0],
                shifted_alt_gr_unicode: $shifted_alt_gr.encode_utf16(&mut [0u16; 1])[0],
                modifier: $modifier,
                affected_attribute: $affected,
            }
        };
    }

    #[test]
    fn test_ord_key_comparisons() {
        let mut key_data1: protocols::simple_text_input_ex::KeyData = Default::default();
        let mut key_data2: protocols::simple_text_input_ex::KeyData = Default::default();

        assert_eq!(OrdKeyData(key_data1), OrdKeyData(key_data2));
        key_data1.key.unicode_char = 'a' as u16;
        assert_ne!(OrdKeyData(key_data1), OrdKeyData(key_data2));
        key_data2.key.unicode_char = 'a' as u16;
        assert_eq!(OrdKeyData(key_data1), OrdKeyData(key_data2));

        key_data1.key.scan_code = SCAN_DOWN;
        assert_ne!(OrdKeyData(key_data1), OrdKeyData(key_data2));
        key_data2.key.scan_code = SCAN_DOWN;
        assert_eq!(OrdKeyData(key_data1), OrdKeyData(key_data2));

        key_data1.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED;
        assert_ne!(OrdKeyData(key_data1), OrdKeyData(key_data2));
        key_data2.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED;
        assert_eq!(OrdKeyData(key_data1), OrdKeyData(key_data2));

        key_data1.key_state.key_toggle_state =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE;
        assert_ne!(OrdKeyData(key_data1), OrdKeyData(key_data2));
        key_data2.key_state.key_toggle_state =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE;
        assert_eq!(OrdKeyData(key_data1), OrdKeyData(key_data2));

        assert_eq!(OrdKeyData(key_data1).partial_cmp(&OrdKeyData(key_data2)), Some(core::cmp::Ordering::Equal));
    }

    #[test]
    fn test_ns_keystroke() {
        let mut key_queue = KeyQueue::default();

        let mut ns_key_layout = crate::keyboard::layout::get_default_keyboard_layout();

        let keys = &mut ns_key_layout.keys;

        let (index, _) = keys
            .iter()
            .enumerate()
            .find(|(_, element)| if let HiiKey::Key(key) = element { key.key == EfiKey::E0 } else { false })
            .unwrap();

        #[rustfmt::skip]
    let ns_key = HiiKey::NsKey(HiiNsKeyDescriptor {
      descriptor:
        key_descriptor!(EfiKey::E0,  '\0',        '\0',       '\0', '\0', NS_KEY_MODIFIER, 0),
      dependent_keys: vec![
        key_descriptor!(EfiKey::C1,  '\u{00E2}',  '\u{00C2}', '\0', '\0', NS_KEY_DEPENDENCY_MODIFIER, AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
        key_descriptor!(EfiKey::D3,  '\u{00EA}',  '\u{00CA}', '\0', '\0', NS_KEY_DEPENDENCY_MODIFIER, AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
        key_descriptor!(EfiKey::D8,  '\u{00EC}',  '\u{00CC}', '\0', '\0', NS_KEY_DEPENDENCY_MODIFIER, AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
        key_descriptor!(EfiKey::D9,  '\u{00F4}',  '\u{00D4}', '\0', '\0', NS_KEY_DEPENDENCY_MODIFIER, AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
        key_descriptor!(EfiKey::D7,  '\u{00FB}',  '\u{00CB}', '\0', '\0', NS_KEY_DEPENDENCY_MODIFIER, AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK)
      ]});

        keys[index] = ns_key.clone();

        key_queue.set_layout(Some(ns_key_layout));

        let key = Usage::from(0x00070035); //E0

        key_queue.keystroke(key, super::KeyAction::KeyDown);
        key_queue.keystroke(key, super::KeyAction::KeyUp);

        let HiiKey::NsKey(expected_key) = ns_key else { panic!() };
        assert_eq!(key_queue.active_ns_key, Some(expected_key));

        assert!(key_queue.peek_key().is_none());

        let key = Usage::from(0x00070004); //C1
        key_queue.keystroke(key, super::KeyAction::KeyDown);
        key_queue.keystroke(key, super::KeyAction::KeyUp);

        let stroke = key_queue.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, '\u{00E2}' as u16);
    }

    // --- keystroke queuing tests ---

    #[test]
    fn basic_key_press_produces_correct_unicode() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_a());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 'a' as u16);
        assert_eq!(stroke.key.scan_code, SCAN_NULL);
    }

    #[test]
    fn key_up_does_not_enqueue() {
        let mut kq = key_queue_with_default_layout();
        release_key(&mut kq, usage_a());
        assert!(kq.pop_key().is_none());
    }

    #[test]
    fn shifted_key_produces_shifted_unicode() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, usage_a());
        // skip the shift key entry if any, find the 'A' key
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 'A' as u16);
    }

    #[test]
    fn shift_removed_from_key_state_when_applied() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, usage_a());
        let stroke = kq.pop_key().unwrap();
        // shift was consumed to produce 'A', so shift should not appear in key_state
        assert_eq!(
            stroke.key_state.key_shift_state
                & (protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED
                    | protocols::simple_text_input_ex::RIGHT_SHIFT_PRESSED),
            0
        );
    }

    #[test]
    fn shift_not_removed_when_not_applied() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, usage_esc()); // ESC is not AFFECTED_BY_STANDARD_SHIFT
        let stroke = kq.pop_key().unwrap();
        // shift was NOT consumed, so it should remain in key_state
        assert_ne!(stroke.key_state.key_shift_state & protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED, 0);
    }

    #[test]
    fn caps_lock_inverts_to_shifted() {
        let mut kq = key_queue_with_default_layout();
        // toggle caps lock on
        tap_key(&mut kq, usage_capslock());
        press_key(&mut kq, usage_a());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 'A' as u16);
    }

    #[test]
    fn caps_lock_plus_shift_inverts_to_unshifted() {
        let mut kq = key_queue_with_default_layout();
        // toggle caps lock on
        tap_key(&mut kq, usage_capslock());
        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, usage_a());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 'a' as u16);
    }

    #[test]
    fn caps_lock_does_not_affect_number_keys() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_capslock());
        press_key(&mut kq, usage_1());
        let stroke = kq.pop_key().unwrap();
        // '1' is AFFECTED_BY_STANDARD_SHIFT but not AFFECTED_BY_CAPS_LOCK
        assert_eq!(stroke.key.unicode_char, '1' as u16);
    }

    #[test]
    fn num_lock_on_numpad_produces_number() {
        let mut kq = key_queue_with_default_layout();
        // toggle num lock on
        tap_key(&mut kq, usage_numlock());
        press_key(&mut kq, usage_numpad1());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, '1' as u16);
        assert_eq!(stroke.key.scan_code, SCAN_NULL);
    }

    #[test]
    fn num_lock_off_numpad_produces_scan_code() {
        let mut kq = key_queue_with_default_layout();
        // num lock is off by default
        press_key(&mut kq, usage_numpad1());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 0);
        assert_eq!(stroke.key.scan_code, SCAN_END);
    }

    #[test]
    fn esc_produces_scan_code() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_esc());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.scan_code, SCAN_ESC);
        assert_eq!(stroke.key.unicode_char, 0);
    }

    #[test]
    fn keystroke_without_layout_does_not_enqueue() {
        let mut kq = KeyQueue::default(); // no layout set
        press_key(&mut kq, usage_a());
        assert!(kq.pop_key().is_none());
    }

    // --- init_key_state tests ---

    #[test]
    fn init_key_state_reflects_active_shift_modifiers() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_lctrl());
        let state = kq.init_key_state();
        assert_ne!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED, 0);
    }

    #[test]
    fn init_key_state_reflects_toggle_modifiers() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_capslock());
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);
    }

    // --- set_key_toggle_state tests ---

    #[test]
    fn set_key_toggle_state_scroll_lock() {
        let mut kq = KeyQueue::default();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::SCROLL_LOCK_ACTIVE,
        );
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::SCROLL_LOCK_ACTIVE, 0);
        assert_eq!(state.key_toggle_state & protocols::simple_text_input_ex::NUM_LOCK_ACTIVE, 0);
    }

    #[test]
    fn set_key_toggle_state_num_lock() {
        let mut kq = KeyQueue::default();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::NUM_LOCK_ACTIVE,
        );
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::NUM_LOCK_ACTIVE, 0);
    }

    #[test]
    fn set_key_toggle_state_key_state_exposed() {
        let mut kq = KeyQueue::default();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        );
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::KEY_STATE_EXPOSED, 0);
    }

    #[test]
    fn set_key_toggle_state_clears_previously_set_toggles() {
        let mut kq = KeyQueue::default();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID
                | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE
                | protocols::simple_text_input_ex::NUM_LOCK_ACTIVE,
        );
        // now clear num lock
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE,
        );
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);
        assert_eq!(state.key_toggle_state & protocols::simple_text_input_ex::NUM_LOCK_ACTIVE, 0);
    }

    // --- reset tests ---

    #[test]
    fn reset_non_extended_retains_toggle_modifiers() {
        let mut kq = key_queue_with_default_layout();
        // activate caps lock (toggle) and left shift (non-toggle)
        tap_key(&mut kq, usage_capslock());
        press_key(&mut kq, usage_lshift());
        // drain any queued keys
        while kq.pop_key().is_some() {}

        kq.reset(false);
        let state = kq.init_key_state();
        // caps lock (toggle/LED modifier) should be retained
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);
        // left shift (non-toggle) should be cleared
        assert_eq!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED, 0);
    }

    #[test]
    fn reset_extended_clears_all_modifiers() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_capslock());
        press_key(&mut kq, usage_lshift());
        while kq.pop_key().is_some() {}

        kq.reset(true);
        let state = kq.init_key_state();
        assert_eq!(state.key_shift_state, protocols::simple_text_input_ex::SHIFT_STATE_VALID);
        assert_eq!(state.key_toggle_state, protocols::simple_text_input_ex::TOGGLE_STATE_VALID);
    }

    // --- matches_registered_key tests ---

    #[test]
    fn matches_registered_key_with_wildcard_shift_state() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        key_data.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;

        // registration with zero shift state should match any shift state
        let mut registration: protocols::simple_text_input_ex::KeyData = Default::default();
        registration.key.unicode_char = 'a' as u16;
        registration.key_state.key_shift_state = 0;

        assert!(OrdKeyData(key_data).matches_registered_key(&OrdKeyData(registration)));
    }

    #[test]
    fn matches_registered_key_with_mismatched_shift_state() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        key_data.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;

        let mut registration: protocols::simple_text_input_ex::KeyData = Default::default();
        registration.key.unicode_char = 'a' as u16;
        registration.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::RIGHT_CONTROL_PRESSED;

        assert!(!OrdKeyData(key_data).matches_registered_key(&OrdKeyData(registration)));
    }

    #[test]
    fn matches_registered_key_with_wildcard_toggle_state() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        key_data.key_state.key_toggle_state =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE;

        let mut registration: protocols::simple_text_input_ex::KeyData = Default::default();
        registration.key.unicode_char = 'a' as u16;
        registration.key_state.key_toggle_state = 0;

        assert!(OrdKeyData(key_data).matches_registered_key(&OrdKeyData(registration)));
    }

    // --- active_leds tests ---

    #[test]
    fn active_leds_reflects_toggle_modifiers() {
        let mut kq = key_queue_with_default_layout();
        assert!(kq.active_leds().is_empty());

        tap_key(&mut kq, usage_capslock());
        let leds = kq.active_leds();
        assert_eq!(leds.len(), 1);
        assert_eq!(leds[0], Usage::from(0x00080002)); // caps lock LED usage
    }

    #[test]
    fn active_leds_multiple_toggles() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_capslock());
        tap_key(&mut kq, usage_scrolllock());
        let leds = kq.active_leds();
        assert_eq!(leds.len(), 2);
    }

    // --- registered key notification queuing ---

    #[test]
    fn registered_key_enqueues_to_notify_queue() {
        let mut kq = key_queue_with_default_layout();
        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key.unicode_char = 'a' as u16;
        kq.add_notify_key(OrdKeyData(reg_key));

        press_key(&mut kq, usage_a());
        assert!(kq.peek_notify_key().is_some());
        let notify = kq.pop_notify_key().unwrap();
        assert_eq!(notify.key.unicode_char, 'a' as u16);
    }

    #[test]
    fn unregistered_key_does_not_enqueue_to_notify_queue() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_a());
        assert!(kq.peek_notify_key().is_none());
    }

    #[test]
    fn num_lock_plus_shift_numpad_produces_scan_code() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_numlock());
        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, usage_numpad1());
        let stroke = kq.pop_key().unwrap();
        assert_eq!(stroke.key.unicode_char, 0);
        assert_eq!(stroke.key.scan_code, SCAN_END);
    }

    #[test]
    fn toggle_modifier_toggles_off_on_second_press() {
        let mut kq = key_queue_with_default_layout();
        tap_key(&mut kq, usage_capslock());
        let state = kq.init_key_state();
        assert_ne!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);

        tap_key(&mut kq, usage_capslock());
        let state = kq.init_key_state();
        assert_eq!(state.key_toggle_state & protocols::simple_text_input_ex::CAPS_LOCK_ACTIVE, 0);
    }

    #[test]
    fn partial_key_support_enqueues_empty_key() {
        let mut kq = key_queue_with_default_layout();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        );
        // left ctrl produces no unicode or scan code, but should still be enqueued with partial support
        press_key(&mut kq, usage_lctrl());
        assert!(kq.pop_key().is_some());
    }

    #[test]
    fn keys_dequeue_in_fifo_order() {
        let mut kq = key_queue_with_default_layout();
        press_key(&mut kq, usage_a());
        press_key(&mut kq, usage_1());
        let first = kq.pop_key().unwrap();
        let second = kq.pop_key().unwrap();
        assert_eq!(first.key.unicode_char, 'a' as u16);
        assert_eq!(second.key.unicode_char, '1' as u16);
        assert!(kq.pop_key().is_none());
    }

    // --- usage_to_efi_key coverage ---

    #[test]
    fn usage_to_efi_key_covers_all_letter_keys() {
        // Test letter keys 'a'..'z' (usages 0x04..0x1D)
        for usage_val in 0x00070004u32..=0x0007001Du32 {
            assert!(
                super::usage_to_efi_key(Usage::from(usage_val)).is_some(),
                "usage_to_efi_key should return Some for usage 0x{:08X}",
                usage_val,
            );
        }
    }

    #[test]
    fn usage_to_efi_key_covers_digit_and_symbol_keys() {
        // 0x1E..0x38 covers digits '1'..'0', enter, esc, backspace, tab, space, and symbols
        for usage_val in 0x0007001Eu32..=0x00070038u32 {
            assert!(
                super::usage_to_efi_key(Usage::from(usage_val)).is_some(),
                "usage_to_efi_key should return Some for usage 0x{:08X}",
                usage_val,
            );
        }
    }

    #[test]
    fn usage_to_efi_key_covers_f_keys_and_navigation() {
        // 0x39..0x65 covers capslock, F1..F12, print, scroll lock, pause, ins, home, pgup,
        // del, end, pgdn, arrows, numlock, numpad keys, B0, A4
        for usage_val in 0x00070039u32..=0x00070065u32 {
            assert!(
                super::usage_to_efi_key(Usage::from(usage_val)).is_some(),
                "usage_to_efi_key should return Some for usage 0x{:08X}",
                usage_val,
            );
        }
    }

    #[test]
    fn usage_to_efi_key_covers_modifier_keys() {
        // 0xE0..0xE7 covers LCtrl, LShift, LAlt, A0, RCtrl, RShift, A2, A3
        for usage_val in 0x000700E0u32..=0x000700E7u32 {
            assert!(
                super::usage_to_efi_key(Usage::from(usage_val)).is_some(),
                "usage_to_efi_key should return Some for usage 0x{:08X}",
                usage_val,
            );
        }
    }

    #[test]
    fn usage_to_efi_key_returns_none_for_error_codes() {
        for usage_val in 0x00070001u32..=0x00070003u32 {
            assert!(super::usage_to_efi_key(Usage::from(usage_val)).is_none());
        }
    }

    #[test]
    fn usage_to_efi_key_returns_none_for_unused_range() {
        assert!(super::usage_to_efi_key(Usage::from(0x00070066u32)).is_none());
        assert!(super::usage_to_efi_key(Usage::from(0x000700DFu32)).is_none());
    }

    #[test]
    fn usage_to_efi_key_returns_none_for_out_of_range() {
        assert!(super::usage_to_efi_key(Usage::from(0x000700F0u32)).is_none());
        assert!(super::usage_to_efi_key(Usage::from(0x00000000u32)).is_none());
    }

    // --- modifier_to_scan coverage ---

    #[test]
    fn modifier_to_scan_covers_all_function_keys() {
        use r_efi::protocols::hii_database::*;
        let cases = [
            (INSERT_MODIFIER, super::SCAN_INSERT),
            (DELETE_MODIFIER, super::SCAN_DELETE),
            (PAGE_DOWN_MODIFIER, super::SCAN_PAGE_DOWN),
            (PAGE_UP_MODIFIER, super::SCAN_PAGE_UP),
            (HOME_MODIFIER, super::SCAN_HOME),
            (END_MODIFIER, super::SCAN_END),
            (LEFT_ARROW_MODIFIER, super::SCAN_LEFT),
            (RIGHT_ARROW_MODIFIER, super::SCAN_RIGHT),
            (DOWN_ARROW_MODIFIER, super::SCAN_DOWN),
            (UP_ARROW_MODIFIER, super::SCAN_UP),
            (FUNCTION_KEY_ONE_MODIFIER, super::SCAN_F1),
            (FUNCTION_KEY_TWO_MODIFIER, super::SCAN_F2),
            (FUNCTION_KEY_THREE_MODIFIER, super::SCAN_F3),
            (FUNCTION_KEY_FOUR_MODIFIER, super::SCAN_F4),
            (FUNCTION_KEY_FIVE_MODIFIER, super::SCAN_F5),
            (FUNCTION_KEY_SIX_MODIFIER, super::SCAN_F6),
            (FUNCTION_KEY_SEVEN_MODIFIER, super::SCAN_F7),
            (FUNCTION_KEY_EIGHT_MODIFIER, super::SCAN_F8),
            (FUNCTION_KEY_NINE_MODIFIER, super::SCAN_F9),
            (FUNCTION_KEY_TEN_MODIFIER, super::SCAN_F10),
            (FUNCTION_KEY_ELEVEN_MODIFIER, super::SCAN_F11),
            (FUNCTION_KEY_TWELVE_MODIFIER, super::SCAN_F12),
            (PAUSE_MODIFIER, super::SCAN_PAUSE),
        ];
        for (modifier, expected_scan) in cases {
            assert_eq!(
                super::modifier_to_scan(modifier),
                expected_scan,
                "modifier_to_scan(0x{:04X}) should be 0x{:04X}",
                modifier,
                expected_scan
            );
        }
    }

    #[test]
    fn modifier_to_scan_returns_null_for_unknown() {
        assert_eq!(super::modifier_to_scan(0xFFFF), SCAN_NULL);
    }

    // --- alt_gr key coverage ---

    #[test]
    fn alt_gr_without_shift_produces_alt_gr_unicode() {
        use r_efi::protocols::hii_database::ALT_GR_MODIFIER;
        let mut kq = KeyQueue::default();
        // Layout with both the alt_gr modifier key and a key that has alt_gr mapping
        let layout = crate::keyboard::layout::HiiKeyboardLayout {
            keys: alloc::vec![
                HiiKey::Key(key_descriptor!(EfiKey::A2, '\0', '\0', '\0', '\0', ALT_GR_MODIFIER, 0)),
                HiiKey::Key(key_descriptor!(EfiKey::C1, 'a', 'A', 'ä', 'Ä', 0, AFFECTED_BY_STANDARD_SHIFT)),
            ],
            ..crate::keyboard::layout::get_default_keyboard_layout()
        };
        kq.set_layout(Some(layout));

        // Press right alt (alt_gr), then 'a'
        press_key(&mut kq, Usage::from(0x000700E6u32)); // A2 = right alt
        press_key(&mut kq, usage_a());

        let key_data = kq.pop_key().unwrap();
        assert_eq!(key_data.key.unicode_char, 'ä' as u16);
    }

    #[test]
    fn shift_plus_alt_gr_produces_shifted_alt_gr_unicode() {
        use r_efi::protocols::hii_database::ALT_GR_MODIFIER;
        let mut kq = KeyQueue::default();
        let layout = crate::keyboard::layout::HiiKeyboardLayout {
            keys: alloc::vec![
                HiiKey::Key(key_descriptor!(EfiKey::A2, '\0', '\0', '\0', '\0', ALT_GR_MODIFIER, 0)),
                HiiKey::Key(key_descriptor!(
                    EfiKey::LShift,
                    '\0',
                    '\0',
                    '\0',
                    '\0',
                    r_efi::protocols::hii_database::LEFT_SHIFT_MODIFIER,
                    0
                )),
                HiiKey::Key(key_descriptor!(EfiKey::C1, 'a', 'A', 'ä', 'Ä', 0, AFFECTED_BY_STANDARD_SHIFT)),
            ],
            ..crate::keyboard::layout::get_default_keyboard_layout()
        };
        kq.set_layout(Some(layout));

        press_key(&mut kq, usage_lshift());
        press_key(&mut kq, Usage::from(0x000700E6u32)); // right alt
        press_key(&mut kq, usage_a());

        let key_data = kq.pop_key().unwrap();
        assert_eq!(key_data.key.unicode_char, 'Ä' as u16);
    }

    // --- modifier key up removes from active state ---

    #[test]
    fn modifier_key_up_clears_shift_state() {
        let mut kq = key_queue_with_default_layout();
        kq.set_key_toggle_state(
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        );

        press_key(&mut kq, usage_lshift());
        let state = kq.init_key_state();
        assert_ne!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED, 0);

        release_key(&mut kq, usage_lshift());
        let state = kq.init_key_state();
        assert_eq!(state.key_shift_state & protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED, 0);
    }

    // --- keystroke without layout does nothing ---

    #[test]
    fn keystroke_without_layout_does_nothing() {
        let mut kq = KeyQueue::default();
        // No layout set
        press_key(&mut kq, usage_a());
        assert!(kq.pop_key().is_none());
    }

    // --- unsupported usage ---

    #[test]
    fn unsupported_usage_is_ignored() {
        let mut kq = key_queue_with_default_layout();
        // Usage 0x00070066 is in the "not used" range
        press_key(&mut kq, Usage::from(0x00070066u32));
        assert!(kq.pop_key().is_none());
    }

    // --- key not found in layout ---

    #[test]
    fn key_not_in_layout_is_ignored() {
        let mut kq = KeyQueue::default();
        // Set a layout with only one key
        let layout = crate::keyboard::layout::HiiKeyboardLayout {
            keys: alloc::vec![HiiKey::Key(key_descriptor!(
                EfiKey::C1,
                'a',
                'A',
                '\0',
                '\0',
                0,
                AFFECTED_BY_STANDARD_SHIFT
            ))],
            ..crate::keyboard::layout::get_default_keyboard_layout()
        };
        kq.set_layout(Some(layout));
        // Press a key that maps to EfiKey::E1 which is not in this minimal layout
        press_key(&mut kq, usage_1());
        assert!(kq.pop_key().is_none());
    }

    // --- is_registered_key returns false when char mismatch ---

    #[test]
    fn matches_registered_key_returns_false_on_char_mismatch() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key.unicode_char = 'b' as u16;
        assert!(!OrdKeyData(key_data).matches_registered_key(&OrdKeyData(reg_key)));
    }

    #[test]
    fn matches_registered_key_returns_false_on_shift_mismatch() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        key_data.key_state.key_shift_state = protocols::simple_text_input_ex::SHIFT_STATE_VALID;
        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key.unicode_char = 'a' as u16;
        reg_key.key_state.key_shift_state =
            protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_SHIFT_PRESSED;
        assert!(!OrdKeyData(key_data).matches_registered_key(&OrdKeyData(reg_key)));
    }

    #[test]
    fn matches_registered_key_returns_false_on_toggle_mismatch() {
        let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();
        key_data.key.unicode_char = 'a' as u16;
        key_data.key_state.key_toggle_state = protocols::simple_text_input_ex::TOGGLE_STATE_VALID;
        let mut reg_key: protocols::simple_text_input_ex::KeyData = Default::default();
        reg_key.key.unicode_char = 'a' as u16;
        reg_key.key_state.key_toggle_state =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::NUM_LOCK_ACTIVE;
        assert!(!OrdKeyData(key_data).matches_registered_key(&OrdKeyData(reg_key)));
    }

    // --- layout getter/setter ---

    #[test]
    fn layout_getter_returns_set_layout() {
        let mut kq = KeyQueue::default();
        assert!(kq.layout().is_none());
        let layout = crate::keyboard::layout::get_default_keyboard_layout();
        kq.set_layout(Some(layout.clone()));
        assert!(kq.layout().is_some());
    }
}
