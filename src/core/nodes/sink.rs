//! SinkNode: Wraps a virtual evdev device as a graph node.
//!
//! A SinkNode:
//! - Receives input events and writes them to the virtual device
//!
//! Note: FF event polling is handled separately by the FF thread in the manager,
//! which uses the VirtualDevice directly for process_ff_upload/erase support.

use evdev::{Device, EventType, InputEvent as EvdevInputEvent};
use log::debug;

use crate::core::drivers::sink::ctrl_event_to_evdev;
use crate::core::event::CtrlEvent;
use crate::core::node::{Node, NodeCapability, NodePorts, PortDef, PortId, ProcessContext, ports};

/// A node that wraps a virtual evdev device for input writing.
///
/// - On `process(INPUT)`: Writes input events to the virtual device
///
/// Note: This node uses evdev::Device (not VirtualDevice) for writing.
/// FF handling requires VirtualDevice and is done separately.
pub struct SinkNode {
    /// Name of this node
    name: String,
    /// The evdev device for writing input events
    device: Device,
}

impl SinkNode {
    /// Create a new sink node wrapping an evdev device.
    pub fn new(name: impl Into<String>, device: Device) -> Self {
        Self {
            name: name.into(),
            device,
        }
    }

    /// Write an input event to the virtual device.
    fn write_input(&mut self, event: &CtrlEvent) {
        let mut evdev_events = ctrl_event_to_evdev(event);
        if !evdev_events.is_empty() {
            // Add sync event
            evdev_events.push(EvdevInputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            if let Err(e) = self.device.send_events(&evdev_events) {
                debug!("Failed to send event to virtual device: {}", e);
            }
        }
    }

    /// Get a reference to the underlying device.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Get a mutable reference to the underlying device.
    pub fn device_mut(&mut self) -> &mut Device {
        &mut self.device
    }
}

impl Node for SinkNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDef::new(ports::SINK_INPUT, "input")],
            outputs: vec![], // No outputs - FF is handled separately
        }
    }

    fn process(
        &mut self,
        input_port: PortId,
        event: CtrlEvent,
        _ctx: &mut ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        // SinkNode only processes input events on its INPUT port
        if input_port == ports::SINK_INPUT {
            if let CtrlEvent::Input(_) = event {
                self.write_input(&event);
            }
        }
        // SinkNode doesn't forward input events - it's a terminal
        vec![]
    }

    fn tick(&mut self) -> Vec<(PortId, CtrlEvent)> {
        // FF event polling is handled separately by the manager's FF thread
        vec![]
    }

    fn has_capability(&self, cap: NodeCapability) -> bool {
        match cap {
            NodeCapability::ProcessesInput => true,
            NodeCapability::ProcessesFeedback => false, // FF is handled by separate thread
            NodeCapability::Tickable => false,          // FF polling is external
        }
    }
}
