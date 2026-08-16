//! Mux2 manager: Handles device setup and runtime lifecycle using GraphExecutor.
//!
//! This module uses a hybrid approach:
//! - Input thread: Uses GraphExecutor for forward event routing (Input → MuxNode → SinkNode)
//! - FF thread: Uses direct device access for efficient blocking (like original mux)
//!
//! This design enables efficient blocking I/O on both threads while leveraging
//! the graph architecture for input multiplexing.

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, InputEvent};
use gilrs::GamepadId;
use log::{error, info};
use parking_lot::RwLock;

use crate::HideType;
use crate::core::drivers::gilrs_driver::GilrsDriver;
use crate::core::graph::{Graph, GraphExecutor};
use crate::core::node::{EmptyState, NodeId, PortId, ports};
use crate::core::nodes::mux::{MuxModeType, MuxNode};
use crate::core::nodes::sink::SinkNode;
use crate::utils::evdev::{VirtualGamepadInfo, create_virtual_gamepad};
use crate::utils::ff::{EffectManager, PhysicalFFDev};
use crate::utils::gilrs::{GamepadResource, discover_gamepad_resources, new_gilrs};
use crate::utils::hide::ScopedDeviceHider;

/// Runtime settings that can be updated while running
pub struct Mux2RuntimeSettings {
    pub mode: RwLock<MuxModeType>,
    /// Which controller is currently "active" for toggle mode FF routing
    /// In toggle mode, this tracks which controller gets FF events
    active_primary: RwLock<bool>,
}

impl Mux2RuntimeSettings {
    pub fn new(mode: MuxModeType) -> Self {
        // In toggle mode, primary starts as active
        let active_primary = matches!(mode, MuxModeType::Toggle);
        Self {
            mode: RwLock::new(mode),
            active_primary: RwLock::new(active_primary),
        }
    }

    pub fn get_mode(&self) -> MuxModeType {
        *self.mode.read()
    }

    pub fn set_active_primary(&self, is_primary: bool) {
        *self.active_primary.write() = is_primary;
    }

    pub fn is_active_primary(&self) -> bool {
        *self.active_primary.read()
    }
}

/// Handle returned from start_mux2 to allow controlling the running session
pub struct Mux2Handle {
    pub shutdown: Arc<AtomicBool>,
    pub settings: Arc<Mux2RuntimeSettings>,
    pub input_thread: JoinHandle<()>,
    pub ff_thread: JoinHandle<()>,
    /// Path to virtual device (for unblocking FF thread on shutdown)
    virtual_device_path: PathBuf,
    // Keep the hider alive so it restores permissions on drop
    #[allow(dead_code)]
    hider: Option<ScopedDeviceHider>,
}

impl Mux2Handle {
    /// Request shutdown and wait for threads to complete
    pub fn shutdown(self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Unblock FF thread by sending no-op event to virtual device
        if let Ok(mut vdev) = Device::open(&self.virtual_device_path) {
            let _ = vdev.send_events(&[
                InputEvent::new(EventType::FORCEFEEDBACK.0, 0, 0),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ]);
        }

        let _ = self.input_thread.join();
        let _ = self.ff_thread.join();
    }
}

/// Node IDs for the mux2 graph (input path only)
struct Mux2GraphNodes {
    mux: NodeId,
    #[allow(dead_code)]
    sink: NodeId,
}

/// Build the mux2 graph topology for the input path.
///
/// Note: SourceNodes are not used in the graph for this implementation.
/// Input events are injected directly into MuxNode, and FF is handled
/// by a separate thread.
fn build_input_graph(mode: MuxModeType, sink: SinkNode) -> (GraphExecutor, Mux2GraphNodes) {
    let mut graph = Graph::new();

    // Add nodes for input path only
    let mux_id = graph.add_node::<EmptyState>(Box::new(MuxNode::new(mode)));
    let sink_id = graph.add_node::<EmptyState>(Box::new(sink));

    // Connect MuxNode output → SinkNode input
    graph.connect(mux_id, ports::MUX_OUTPUT, sink_id, ports::SINK_INPUT);

    let nodes = Mux2GraphNodes {
        mux: mux_id,
        sink: sink_id,
    };

    (GraphExecutor::new(graph), nodes)
}

/// Start a mux2 session with the graph-based architecture.
///
/// This is the main entry point for the mux2 functionality.
pub fn start_mux2(
    primary_id: GamepadId,
    assist_id: GamepadId,
    mode: MuxModeType,
    hide_type: HideType,
    spoof_info: Option<VirtualGamepadInfo>,
) -> Result<Mux2Handle, Box<dyn Error>> {
    info!("Starting mux2 with mode {:?}", mode);

    // Initialize gilrs
    let gilrs = new_gilrs()?;

    // Discover device paths
    let resources = discover_gamepad_resources(&gilrs);
    let primary_res = resources
        .get(&primary_id)
        .ok_or("Primary controller not found")?
        .clone();
    let assist_res = resources
        .get(&assist_id)
        .ok_or("Assist controller not found")?
        .clone();

    // Setup device hiding
    let mut hider = ScopedDeviceHider::new(hide_type.clone())?;
    hider.hide_gamepad_devices(&primary_res)?;
    hider.hide_gamepad_devices(&assist_res)?;

    // Create virtual gamepad info (use spoofed or default)
    let vgp_info = spoof_info.unwrap_or_else(|| VirtualGamepadInfo {
        name: "CtrlAssist Virtual Gamepad".to_string(),
        vendor_id: Some(0x045e),  // Xbox controller vendor
        product_id: Some(0x028e), // Xbox controller product
    });

    // Create virtual gamepad with FF support
    let mut virtual_uinput = create_virtual_gamepad(&vgp_info, Some("mux2"))?;

    // Wait for the device to be registered and get its path
    let v_resource = crate::utils::gilrs::wait_for_virtual_device(&mut virtual_uinput)?;
    let virtual_device_path = v_resource.path.clone();

    let virtual_msg = format!(
        "Virtual: {} @ {}",
        v_resource.name,
        v_resource.path.display()
    );
    info!("{}", virtual_msg);
    println!("{}", virtual_msg);

    // Create shared state
    let shutdown = Arc::new(AtomicBool::new(false));
    let settings = Arc::new(Mux2RuntimeSettings::new(mode));

    // Create SinkNode for input path (writes to virtual device via Device handle)
    let sink = SinkNode::new("Virtual Gamepad", v_resource.device);

    // Build the input graph
    let (executor, nodes) = build_input_graph(mode, sink);

    // Create ID mapping for the input loop
    let mut id_to_port = HashMap::new();
    id_to_port.insert(primary_id, ports::MUX_PRIMARY_IN);
    id_to_port.insert(assist_id, ports::MUX_ASSIST_IN);

    // Create driver
    let driver = GilrsDriver::new(gilrs, vec![primary_id, assist_id]);

    // Clone resources for FF thread
    let all_resources = resources.clone();

    // Clone for input thread
    let shutdown_input = Arc::clone(&shutdown);
    let settings_input = Arc::clone(&settings);

    // Spawn input thread
    let input_thread = thread::Builder::new()
        .name("mux2-input".to_string())
        .spawn(move || {
            run_input_loop(
                executor,
                nodes,
                driver,
                id_to_port,
                shutdown_input,
                settings_input,
            );
        })?;

    // Clone for FF thread
    let shutdown_ff = Arc::clone(&shutdown);
    let settings_ff = Arc::clone(&settings);

    // Spawn FF thread (uses VirtualDevice for process_ff_upload/erase)
    let ff_thread = thread::Builder::new()
        .name("mux2-ff".to_string())
        .spawn(move || {
            run_ff_loop(
                virtual_uinput,
                all_resources,
                primary_id,
                assist_id,
                shutdown_ff,
                settings_ff,
            );
        })?;

    // Keep hider alive if we're actually hiding
    let hider_opt = if hide_type == HideType::None {
        None
    } else {
        Some(hider)
    };

    Ok(Mux2Handle {
        shutdown,
        settings,
        input_thread,
        ff_thread,
        virtual_device_path,
        hider: hider_opt,
    })
}

/// Input processing loop.
///
/// Polls gilrs for input events, injects them into the graph, and processes
/// through MuxNode → SinkNode (which writes to the virtual device).
fn run_input_loop(
    mut executor: GraphExecutor,
    nodes: Mux2GraphNodes,
    mut driver: GilrsDriver,
    id_to_port: HashMap<GamepadId, PortId>,
    shutdown: Arc<AtomicBool>,
    settings: Arc<Mux2RuntimeSettings>,
) {
    info!("Starting mux2 input loop");

    let timeout = Duration::from_millis(1000);

    while !shutdown.load(Ordering::Relaxed) {
        // Poll gilrs with timeout (efficient blocking)
        while let Some((gamepad_id, ctrl_event)) = driver.poll_event_blocking(Some(timeout)) {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Determine which MuxNode input port this event should go to
            if let Some(&input_port) = id_to_port.get(&gamepad_id) {
                // Track active controller for toggle mode FF routing
                // When primary sends input, it's active; when assist sends, assist is active
                if settings.get_mode() == MuxModeType::Toggle {
                    settings.set_active_primary(input_port == ports::MUX_PRIMARY_IN);
                }

                // Inject into MuxNode at the appropriate input port
                executor.inject(nodes.mux, input_port, ctrl_event);
            }
        }

        // Process any injected events (routes through graph to SinkNode)
        let _outputs = executor.process();
    }

    info!("Mux2 input loop stopped");
}

/// FF processing loop.
///
/// Blocks on the virtual device for FF events, then routes them to physical
/// devices based on the current mode setting.
fn run_ff_loop(
    mut virtual_uinput: VirtualDevice,
    resources: HashMap<GamepadId, GamepadResource>,
    primary_id: GamepadId,
    assist_id: GamepadId,
    shutdown: Arc<AtomicBool>,
    settings: Arc<Mux2RuntimeSettings>,
) {
    info!("Starting mux2 FF loop");

    // Create physical FF device wrappers
    let mut primary_ff = resources.get(&primary_id).and_then(|res| {
        if res.device.supported_ff().is_some() {
            Some(PhysicalFFDev::new(res.clone()))
        } else {
            None
        }
    });

    let mut assist_ff = resources.get(&assist_id).and_then(|res| {
        if res.device.supported_ff().is_some() {
            Some(PhysicalFFDev::new(res.clone()))
        } else {
            None
        }
    });

    // Effect manager for state synchronization
    let mut effect_manager = EffectManager::new();

    while !shutdown.load(Ordering::Relaxed) {
        // Block on FF events from the virtual device
        let events: Vec<_> = match virtual_uinput.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                if !shutdown.load(Ordering::Relaxed) {
                    error!("Error fetching FF events: {}", e);
                }
                continue;
            }
        };

        for event in events {
            // Determine which physical devices should receive this FF event
            let (to_primary, to_assist) = get_ff_targets(&settings);

            match event.destructure() {
                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_UPLOAD, ..) => {
                    if let Ok(upload_ev) = virtual_uinput.process_ff_upload(ev) {
                        let virt_id = upload_ev.effect_id();
                        let effect_data = upload_ev.effect();

                        // Record in manager
                        effect_manager.upload(virt_id, effect_data);

                        // Upload to target devices
                        if to_primary {
                            if let Some(ref mut ff_dev) = primary_ff {
                                if let Err(e) = ff_dev.upload_effect(virt_id, effect_data) {
                                    error!("Failed to upload effect {} to primary: {}", virt_id, e);
                                }
                            }
                        }
                        if to_assist {
                            if let Some(ref mut ff_dev) = assist_ff {
                                if let Err(e) = ff_dev.upload_effect(virt_id, effect_data) {
                                    error!("Failed to upload effect {} to assist: {}", virt_id, e);
                                }
                            }
                        }
                    }
                }

                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_ERASE, ..) => {
                    if let Ok(erase_ev) = virtual_uinput.process_ff_erase(ev) {
                        let virt_id = erase_ev.effect_id() as i16;

                        // Erase from target devices
                        if to_primary {
                            if let Some(ref mut ff_dev) = primary_ff {
                                if let Err(e) = ff_dev.erase_effect(virt_id) {
                                    error!("Failed to erase effect {} from primary: {}", virt_id, e);
                                }
                            }
                        }
                        if to_assist {
                            if let Some(ref mut ff_dev) = assist_ff {
                                if let Err(e) = ff_dev.erase_effect(virt_id) {
                                    error!("Failed to erase effect {} from assist: {}", virt_id, e);
                                }
                            }
                        }

                        // Remove from manager
                        effect_manager.erase(virt_id);
                    }
                }

                evdev::EventSummary::ForceFeedback(_, effect_id, status) => {
                    let virt_id = effect_id.0 as i16;
                    let is_playing = status == evdev::FFStatusCode::FF_STATUS_PLAYING.0 as i32;

                    // Update manager state
                    effect_manager.set_playing(virt_id, is_playing);

                    // Apply to target devices
                    if to_primary {
                        if let Some(ref mut ff_dev) = primary_ff {
                            if let Err(e) = ff_dev.control_effect(virt_id, is_playing) {
                                if e.raw_os_error() != Some(libc::ENODEV) {
                                    error!("Failed to control FF on primary: {}", e);
                                }
                            }
                        }
                    }

                    if to_assist {
                        if let Some(ref mut ff_dev) = assist_ff {
                            if let Err(e) = ff_dev.control_effect(virt_id, is_playing) {
                                if e.raw_os_error() != Some(libc::ENODEV) {
                                    error!("Failed to control FF on assist: {}", e);
                                }
                            }
                        }
                    }
                }

                _ => {
                    // Ignore other event types
                }
            }
        }
    }

    info!("Mux2 FF loop stopped");
}

/// Determine which physical devices should receive FF events based on mode.
fn get_ff_targets(settings: &Mux2RuntimeSettings) -> (bool, bool) {
    match settings.get_mode() {
        MuxModeType::Priority | MuxModeType::Average => {
            // Both controllers get FF
            (true, true)
        }
        MuxModeType::Toggle => {
            // Only active controller gets FF
            let primary_active = settings.is_active_primary();
            (primary_active, !primary_active)
        }
    }
}
