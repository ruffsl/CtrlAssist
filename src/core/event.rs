//! Event types for the CtrlAssist graph architecture.
//!
//! [`CtrlEvent`] is the unified packet type that flows through the graph.
//! All values are normalized for consistency:
//! - Axes/sticks: -1.0 to 1.0
//! - Triggers: 0.0 to 1.0
//! - Buttons: pressed (true) or released (false)

use std::fmt;

/// Unique identifier for a device (physical or virtual)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u32);

/// Unique identifier for a button
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonId {
    South, // A / Cross
    East,  // B / Circle
    North, // Y / Triangle
    West,  // X / Square
    LeftBumper,
    RightBumper,
    LeftTriggerButton,  // L2 as button
    RightTriggerButton, // R2 as button
    Select,
    Start,
    Mode, // Guide/Home/PS button
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

/// Unique identifier for an axis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisId {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
    DPadX,
    DPadY,
}

/// Unique identifier for a force feedback effect
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectId(pub i16);

/// The unified event type that flows through the graph.
///
/// Events flow in two directions:
/// - Forward (input): Physical devices → Graph → Virtual devices
/// - Reverse (feedback): Virtual devices → Graph → Physical devices
#[derive(Debug, Clone)]
pub enum CtrlEvent {
    /// Input event (forward direction: physical → virtual)
    Input(InputEvent),

    /// Force feedback event (reverse direction: virtual → physical)
    ForceFeedback(FFEvent),

    /// LED control event (reverse direction: virtual → physical)
    Led(LedEvent),
    // Future extensions:
    // Motion(MotionEvent),
    // Touchpad(TouchpadEvent),
    // Raw { device_id: DeviceId, data: Vec<u8> },
}

/// An input event with normalized values.
#[derive(Debug, Clone)]
pub struct InputEvent {
    /// Source device that generated this event
    pub source: Option<DeviceId>,
    /// The actual input change
    pub input: InputKind,
}

/// Types of input changes
#[derive(Debug, Clone)]
pub enum InputKind {
    /// Button state change (pressed = true, released = false)
    Button { id: ButtonId, pressed: bool },
    /// Axis value change (normalized: -1.0 to 1.0 for sticks, 0.0 to 1.0 for triggers)
    Axis { id: AxisId, value: f32 },
}

/// Force feedback events
///
/// These carry evdev's FFEffectData directly for compatibility with physical devices.
#[derive(Debug, Clone)]
pub enum FFEvent {
    /// Upload a new effect
    Upload {
        effect_id: EffectId,
        effect_data: evdev::FFEffectData,
    },
    /// Remove an effect
    Erase { effect_id: EffectId },
    /// Start playing an effect
    Play { effect_id: EffectId },
    /// Stop playing an effect
    Stop { effect_id: EffectId },
}

/// LED control events
#[derive(Debug, Clone)]
pub enum LedEvent {
    /// Set player indicator LEDs
    PlayerIndicator { player: u8 },
    /// Set RGB lightbar color
    Lightbar { r: u8, g: u8, b: u8 },
}

impl fmt::Display for CtrlEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CtrlEvent::Input(e) => write!(f, "Input({:?})", e.input),
            CtrlEvent::ForceFeedback(e) => write!(f, "FF({:?})", e),
            CtrlEvent::Led(e) => write!(f, "LED({:?})", e),
        }
    }
}

impl InputEvent {
    /// Create a button event
    pub fn button(id: ButtonId, pressed: bool) -> Self {
        Self {
            source: None,
            input: InputKind::Button { id, pressed },
        }
    }

    /// Create an axis event
    pub fn axis(id: AxisId, value: f32) -> Self {
        Self {
            source: None,
            input: InputKind::Axis { id, value },
        }
    }

    /// Set the source device
    pub fn with_source(mut self, device: DeviceId) -> Self {
        self.source = Some(device);
        self
    }
}
