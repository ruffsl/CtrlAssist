//! SourceNode: Wraps a physical gamepad as a graph node.
//!
//! A SourceNode:
//! - Polls gilrs events and emits them as CtrlEvents on tick
//! - Receives FF events and forwards them to the physical device

use gilrs::GamepadId;
use log::{debug, error};

use crate::core::event::{CtrlEvent, FFEvent};
use crate::core::node::{Node, NodeCapability, NodePorts, PortDef, PortId, ProcessContext, ports};
use crate::utils::ff::PhysicalFFDev;
use crate::utils::gilrs::GamepadResource;

/// A node that wraps a physical gamepad.
///
/// - On `tick()`: Polls gilrs for events and emits them on SOURCE_OUTPUT
/// - On `process(FF)`: Forwards FF events to the physical device
pub struct SourceNode {
    /// Name of this node (e.g., "Primary Controller")
    name: String,
    /// The gamepad ID this source represents
    gamepad_id: GamepadId,
    /// Physical FF device wrapper (if FF-capable)
    ff_dev: Option<PhysicalFFDev>,
}

impl SourceNode {
    /// Create a new source node for a specific gamepad.
    ///
    /// The `resource` is used to set up FF forwarding if the device supports it.
    pub fn new(
        name: impl Into<String>,
        gamepad_id: GamepadId,
        resource: Option<GamepadResource>,
    ) -> Self {
        let ff_dev = resource.and_then(|res| {
            if res.device.supported_ff().is_some() {
                Some(PhysicalFFDev::new(res))
            } else {
                None
            }
        });

        Self {
            name: name.into(),
            gamepad_id,
            ff_dev,
        }
    }

    /// Poll for events from gilrs.
    ///
    /// Note: This requires access to the shared Gilrs instance, which is passed
    /// during tick via the executor. For now, we use a different approach -
    /// the executor calls poll externally and injects events.
    pub fn gamepad_id(&self) -> GamepadId {
        self.gamepad_id
    }

    /// Handle an FF event by forwarding to the physical device.
    fn handle_ff(&mut self, ff: &FFEvent) {
        let Some(ref mut ff_dev) = self.ff_dev else {
            debug!("No FF device for source '{}', ignoring FF event", self.name);
            return;
        };

        match ff {
            FFEvent::Upload {
                effect_id,
                effect_data,
            } => {
                if let Err(e) = ff_dev.upload_effect(effect_id.0, *effect_data) {
                    error!(
                        "Failed to upload effect {} to {}: {}",
                        effect_id.0,
                        ff_dev.resource.path.display(),
                        e
                    );
                }
            }
            FFEvent::Erase { effect_id } => {
                if let Err(e) = ff_dev.erase_effect(effect_id.0) {
                    error!(
                        "Failed to erase effect {} from {}: {}",
                        effect_id.0,
                        ff_dev.resource.path.display(),
                        e
                    );
                }
            }
            FFEvent::Play { effect_id } => {
                if let Err(e) = ff_dev.control_effect(effect_id.0, true) {
                    if e.raw_os_error() != Some(libc::ENODEV) {
                        error!(
                            "Failed to play effect {} on {}: {}",
                            effect_id.0,
                            ff_dev.resource.path.display(),
                            e
                        );
                    }
                }
            }
            FFEvent::Stop { effect_id } => {
                if let Err(e) = ff_dev.control_effect(effect_id.0, false) {
                    if e.raw_os_error() != Some(libc::ENODEV) {
                        error!(
                            "Failed to stop effect {} on {}: {}",
                            effect_id.0,
                            ff_dev.resource.path.display(),
                            e
                        );
                    }
                }
            }
        }
    }
}

impl Node for SourceNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDef::new(ports::SOURCE_FF_IN, "ff_in")],
            outputs: vec![PortDef::new(ports::SOURCE_OUTPUT, "output")],
        }
    }

    fn process(
        &mut self,
        input_port: PortId,
        event: CtrlEvent,
        _ctx: &mut ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        // SourceNode only processes FF events on its FF input port
        if input_port == ports::SOURCE_FF_IN {
            if let CtrlEvent::ForceFeedback(ref ff) = event {
                self.handle_ff(ff);
            }
        }
        // SourceNode doesn't forward events - it's a terminal for FF
        vec![]
    }

    fn tick(&mut self) -> Vec<(PortId, CtrlEvent)> {
        // Note: The actual gilrs polling happens at the executor level,
        // which then injects events into the graph. SourceNode's tick
        // is a no-op because it needs shared access to Gilrs.
        vec![]
    }

    fn has_capability(&self, cap: NodeCapability) -> bool {
        match cap {
            NodeCapability::ProcessesInput => false, // Only processes FF
            NodeCapability::ProcessesFeedback => true,
            NodeCapability::Tickable => false, // Polling handled externally
        }
    }
}
