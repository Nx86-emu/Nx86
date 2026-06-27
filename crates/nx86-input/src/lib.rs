use gilrs::{Button, EventType, Gilrs};

pub const CRATE_NAME: &str = "nx86-input";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAction {
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    A,
    B,
    X,
    Y,
    L,
    R,
    ZL,
    ZR,
    Plus,
    Minus,
}

impl InputAction {
    pub const ALL: [Self; 14] = [
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::L,
        Self::R,
        Self::ZL,
        Self::ZR,
        Self::Plus,
        Self::Minus,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DPadUp => "D-pad Up",
            Self::DPadDown => "D-pad Down",
            Self::DPadLeft => "D-pad Left",
            Self::DPadRight => "D-pad Right",
            Self::A => "A",
            Self::B => "B",
            Self::X => "X",
            Self::Y => "Y",
            Self::L => "L",
            Self::R => "R",
            Self::ZL => "ZL",
            Self::ZR => "ZR",
            Self::Plus => "Plus",
            Self::Minus => "Minus",
        }
    }

    #[must_use]
    pub const fn bit(self) -> u64 {
        match self {
            Self::DPadUp => ControllerButtons::D_PAD_UP,
            Self::DPadDown => ControllerButtons::D_PAD_DOWN,
            Self::DPadLeft => ControllerButtons::D_PAD_LEFT,
            Self::DPadRight => ControllerButtons::D_PAD_RIGHT,
            Self::A => ControllerButtons::A,
            Self::B => ControllerButtons::B,
            Self::X => ControllerButtons::X,
            Self::Y => ControllerButtons::Y,
            Self::L => ControllerButtons::L,
            Self::R => ControllerButtons::R,
            Self::ZL => ControllerButtons::ZL,
            Self::ZR => ControllerButtons::ZR,
            Self::Plus => ControllerButtons::PLUS,
            Self::Minus => ControllerButtons::MINUS,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControllerButtons(u64);

impl ControllerButtons {
    pub const D_PAD_UP: u64 = 1 << 0;
    pub const D_PAD_DOWN: u64 = 1 << 1;
    pub const D_PAD_LEFT: u64 = 1 << 2;
    pub const D_PAD_RIGHT: u64 = 1 << 3;
    pub const A: u64 = 1 << 4;
    pub const B: u64 = 1 << 5;
    pub const X: u64 = 1 << 6;
    pub const Y: u64 = 1 << 7;
    pub const L: u64 = 1 << 8;
    pub const R: u64 = 1 << 9;
    pub const ZL: u64 = 1 << 10;
    pub const ZR: u64 = 1 << 11;
    pub const PLUS: u64 = 1 << 12;
    pub const MINUS: u64 = 1 << 13;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn is_pressed(self, action: InputAction) -> bool {
        self.0 & action.bit() != 0
    }

    pub fn set(&mut self, action: InputAction, pressed: bool) {
        if pressed {
            self.0 |= action.bit();
        } else {
            self.0 &= !action.bit();
        }
    }

    #[must_use]
    pub fn with(mut self, action: InputAction) -> Self {
        self.set(action, true);
        self
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputSnapshot {
    pub controller_state: u64,
}

impl InputSnapshot {
    #[must_use]
    pub const fn neutral() -> Self {
        Self {
            controller_state: 0,
        }
    }

    #[must_use]
    pub const fn from_buttons(buttons: ControllerButtons) -> Self {
        Self {
            controller_state: buttons.bits(),
        }
    }

    #[must_use]
    pub const fn buttons(self) -> ControllerButtons {
        ControllerButtons::from_bits(self.controller_state)
    }

    #[must_use]
    pub const fn packed(self) -> u64 {
        self.controller_state
    }
}

#[must_use]
pub const fn neutral_controller_state() -> u64 {
    InputSnapshot::neutral().packed()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GamepadStatus {
    pub available: bool,
    pub connected_gamepads: usize,
    pub last_error: Option<String>,
}

impl GamepadStatus {
    #[must_use]
    pub fn label(&self) -> String {
        if self.available {
            format!("{} gamepad(s) connected", self.connected_gamepads)
        } else {
            self.last_error
                .clone()
                .unwrap_or_else(|| "gamepad backend unavailable".to_owned())
        }
    }
}

#[derive(Debug)]
pub struct GamepadRuntime {
    gilrs: Option<Gilrs>,
    buttons: ControllerButtons,
    status: GamepadStatus,
}

impl GamepadRuntime {
    #[must_use]
    pub fn new() -> Self {
        match Gilrs::new() {
            Ok(gilrs) => {
                let connected_gamepads = gilrs.gamepads().count();
                Self {
                    gilrs: Some(gilrs),
                    buttons: ControllerButtons::empty(),
                    status: GamepadStatus {
                        available: true,
                        connected_gamepads,
                        last_error: None,
                    },
                }
            }
            Err(error) => Self::unavailable(error.to_string()),
        }
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            gilrs: None,
            buttons: ControllerButtons::empty(),
            status: GamepadStatus {
                available: false,
                connected_gamepads: 0,
                last_error: Some(reason.into()),
            },
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> InputSnapshot {
        InputSnapshot::from_buttons(self.buttons)
    }

    #[must_use]
    pub const fn buttons(&self) -> ControllerButtons {
        self.buttons
    }

    #[must_use]
    pub const fn status(&self) -> &GamepadStatus {
        &self.status
    }

    pub fn poll(&mut self) -> InputSnapshot {
        let Some(gilrs) = self.gilrs.as_mut() else {
            return self.snapshot();
        };

        while let Some(event) = gilrs.next_event() {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(action) = action_for_gilrs_button(button) {
                        self.buttons.set(action, true);
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(action) = action_for_gilrs_button(button) {
                        self.buttons.set(action, false);
                    }
                }
                _ => {}
            }
        }

        self.status.connected_gamepads = gilrs.gamepads().count();
        self.snapshot()
    }
}

impl Default for GamepadRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub const fn action_for_gilrs_button(button: Button) -> Option<InputAction> {
    match button {
        Button::DPadUp => Some(InputAction::DPadUp),
        Button::DPadDown => Some(InputAction::DPadDown),
        Button::DPadLeft => Some(InputAction::DPadLeft),
        Button::DPadRight => Some(InputAction::DPadRight),
        Button::South => Some(InputAction::A),
        Button::East => Some(InputAction::B),
        Button::North => Some(InputAction::X),
        Button::West => Some(InputAction::Y),
        Button::LeftTrigger => Some(InputAction::L),
        Button::RightTrigger => Some(InputAction::R),
        Button::LeftTrigger2 => Some(InputAction::ZL),
        Button::RightTrigger2 => Some(InputAction::ZR),
        Button::Start => Some(InputAction::Plus),
        Button::Select => Some(InputAction::Minus),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ControllerButtons, InputAction, InputSnapshot, action_for_gilrs_button,
        neutral_controller_state,
    };
    use gilrs::Button;

    #[test]
    fn neutral_input_snapshot_has_no_buttons_pressed() {
        assert_eq!(InputSnapshot::neutral().packed(), 0);
        assert_eq!(neutral_controller_state(), 0);
        assert!(InputSnapshot::neutral().buttons().is_empty());
    }

    #[test]
    fn controller_buttons_pack_stable_bits() {
        let mut buttons = ControllerButtons::empty();

        buttons.set(InputAction::A, true);
        buttons.set(InputAction::DPadUp, true);
        buttons.set(InputAction::A, false);

        assert_eq!(buttons.bits(), ControllerButtons::D_PAD_UP);
        assert!(buttons.is_pressed(InputAction::DPadUp));
        assert!(!buttons.is_pressed(InputAction::A));
    }

    #[test]
    fn gilrs_buttons_map_to_controller_actions() {
        assert_eq!(action_for_gilrs_button(Button::South), Some(InputAction::A));
        assert_eq!(action_for_gilrs_button(Button::East), Some(InputAction::B));
        assert_eq!(
            action_for_gilrs_button(Button::DPadLeft),
            Some(InputAction::DPadLeft)
        );
        assert_eq!(
            action_for_gilrs_button(Button::RightTrigger2),
            Some(InputAction::ZR)
        );
        assert_eq!(action_for_gilrs_button(Button::Mode), None);
    }
}
