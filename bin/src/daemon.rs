mod analysis;
mod config;
mod error;
mod pcap;
mod server;
mod stats;
mod qmdl_store;
mod diag;
mod framebuffer;
mod dummy_analyzer;

// Define a version constant that can be easily updated for releases
pub const VERSION: &str = "V1.2.0";

use crate::config::{parse_config, parse_args};
use crate::diag::run_diag_read_thread;
use crate::qmdl_store::RecordingStore;
use crate::server::{ServerState, get_qmdl, serve_static};
use crate::pcap::get_pcap;
use crate::stats::get_system_stats;
use crate::error::RayhunterError;
use crate::framebuffer::Framebuffer;

use analysis::{get_analysis_status, run_analysis_thread, start_analysis, AnalysisCtrlMessage, AnalysisStatus};
use axum::response::Redirect;
use diag::{get_analysis_report, start_recording, stop_recording, DiagDeviceCtrlMessage};
use log::{info, error, debug, log_enabled};
use rayhunter::diag_device::DiagDevice;
use axum::routing::{get, post};
use axum::Router;
use stats::get_qmdl_manifest;
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::oneshot::error::TryRecvError;
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, oneshot};
use std::sync::Arc;
use include_dir::{include_dir, Dir};
use simple_logger;
use std::fs::File as StdFile;
use std::io::Read;
use std::time::{Instant};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// Add a static for tracking UI visibility
static UI_VISIBLE: AtomicBool = AtomicBool::new(true);
// This static is no longer used and can be removed
// static BLACK_SCREEN_DRAWN: AtomicBool = AtomicBool::new(false);
// These are no longer needed with our single-thread approach
// static BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);
// static BUTTON_PRESS_TIME: AtomicU64 = AtomicU64::new(0);

// Runs the axum server, taking all the elements needed to build up our
// ServerState and a oneshot Receiver that'll fire when it's time to shutdown
// (i.e. user hit ctrl+c)
async fn run_server(
    task_tracker: &TaskTracker,
    config: &config::Config,
    qmdl_store_lock: Arc<RwLock<RecordingStore>>,
    server_shutdown_rx: oneshot::Receiver<()>,
    ui_update_tx: Sender<framebuffer::DisplayState>,
    diag_device_sender: Sender<DiagDeviceCtrlMessage>,
    analysis_sender: Sender<AnalysisCtrlMessage>,
    analysis_status_lock: Arc<RwLock<AnalysisStatus>>,
) -> JoinHandle<()> {
    info!("spinning up server");
    let state = Arc::new(ServerState {
        qmdl_store_lock,
        diag_device_ctrl_sender: diag_device_sender,
        ui_update_sender: ui_update_tx,
        debug_mode: config.debug_mode,
        analysis_status_lock,
        analysis_sender,
        colorblind_mode: config.colorblind_mode,
    });

    let app = Router::new()
        .route("/api/pcap/*name", get(get_pcap))
        .route("/api/qmdl/*name", get(get_qmdl))
        .route("/api/system-stats", get(get_system_stats))
        .route("/api/qmdl-manifest", get(get_qmdl_manifest))
        .route("/api/start-recording", post(start_recording))
        .route("/api/stop-recording", post(stop_recording))
        .route("/api/analysis-report/*name", get(get_analysis_report))
        .route("/api/analysis", get(get_analysis_status))
        .route("/api/analysis/*name", post(start_analysis))
        .route("/", get(|| async { Redirect::permanent("/index.html") }))
        .route("/*path", get(serve_static))
        .with_state(state);
    
    // Try configured port first
    let mut port = config.port;
    let mut listener_result = TcpListener::bind(&SocketAddr::from(([0, 0, 0, 0], port))).await;
    
    // If that fails, try port 8888
    if listener_result.is_err() {
        error!("Failed to bind to port {}: {:?}", port, listener_result.err());
        port = 8888;
        listener_result = TcpListener::bind(&SocketAddr::from(([0, 0, 0, 0], port))).await;
    }
    
    // If 8888 also fails, try port 9999
    if listener_result.is_err() {
        error!("Failed to bind to port {}: {:?}", port, listener_result.err());
        port = 9999;
        listener_result = TcpListener::bind(&SocketAddr::from(([0, 0, 0, 0], port))).await;
    }
    
    // If all ports fail, give up
    if listener_result.is_err() {
        error!("Failed to bind to any port. Last error: {:?}", listener_result.err());
        return task_tracker.spawn(async move {
            error!("Server could not start due to binding errors");
        });
    }
    
    let listener = listener_result.unwrap();
    info!("Successfully bound to port {}", port);
    
    task_tracker.spawn(async move {
        info!("The orca is hunting for stingrays...");
        axum::serve(listener, app)
            .with_graceful_shutdown(server_shutdown_signal(server_shutdown_rx))
            .await.unwrap_or_else(|e| error!("Server error: {:?}", e));
    })
}

async fn server_shutdown_signal(server_shutdown_rx: oneshot::Receiver<()>) {
    server_shutdown_rx.await.unwrap();
    info!("Server received shutdown signal, exiting...");
}

// Loads a QmdlStore if one exists, and if not, only create one if we're not in
// debug mode.
async fn init_qmdl_store(config: &config::Config) -> Result<RecordingStore, RayhunterError> {
    match (RecordingStore::exists(&config.qmdl_store_path).await?, config.debug_mode) {
        (true, _) => Ok(RecordingStore::load(&config.qmdl_store_path).await?),
        (false, false) => Ok(RecordingStore::create(&config.qmdl_store_path).await?),
        (false, true) => Err(RayhunterError::NoStoreDebugMode(config.qmdl_store_path.clone())),
    }
}

// Start a thread that'll track when user hits ctrl+c. When that happens,
// trigger various cleanup tasks, including sending signals to other threads to
// shutdown
fn run_ctrl_c_thread(
    task_tracker: &TaskTracker,
    diag_device_sender: Sender<DiagDeviceCtrlMessage>,
    server_shutdown_tx: oneshot::Sender<()>,
    maybe_ui_shutdown_tx: Option<oneshot::Sender<()>>,
    qmdl_store_lock: Arc<RwLock<RecordingStore>>,
    analysis_tx: Sender<AnalysisCtrlMessage>,
) -> JoinHandle<Result<(), RayhunterError>> {
    task_tracker.spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                let mut qmdl_store = qmdl_store_lock.write().await;
                if qmdl_store.current_entry.is_some() {
                    info!("Closing current QMDL entry...");
                    qmdl_store.close_current_entry().await?;
                    info!("Done!");
                }

                server_shutdown_tx.send(())
                    .expect("couldn't send server shutdown signal");
                info!("sending UI shutdown");
                if let Some(ui_shutdown_tx) = maybe_ui_shutdown_tx {
                    ui_shutdown_tx.send(())
                        .expect("couldn't send ui shutdown signal");
                }
                diag_device_sender.send(DiagDeviceCtrlMessage::Exit).await
                    .expect("couldn't send Exit message to diag thread");
                analysis_tx.send(AnalysisCtrlMessage::Exit).await
                    .expect("couldn't send Exit message to analysis thread");
            },
            Err(err) => {
                error!("Unable to listen for shutdown signal: {}", err);
            }
        }
        Ok(())
    })
}

fn update_ui(task_tracker: &TaskTracker, config: &config::Config, mut ui_shutdown_rx: oneshot::Receiver<()>, mut ui_update_rx: Receiver<framebuffer::DisplayState>) -> JoinHandle<()> {
    static IMAGE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static/images/");
    let mut display_color: framebuffer::Color565;
    let display_level = config.ui_level;
    // Share the qmdl_store_lock with the UI thread so it can access latest data
    let qmdl_store_path = config.qmdl_store_path.clone();
    
    if display_level == 0 {
        info!("Invisible mode, not spawning UI.");
        return task_tracker.spawn(async move {
            match ui_shutdown_rx.await {
                Ok(_) => info!("received UI shutdown, but we're in invisible mode"),
                Err(e) => error!("error receiving shutdown message: {e}")
            }
        });
    }

    // Read the config values once to avoid borrowing the reference in the task
    let config_clone = config::Config {
        qmdl_store_path: config.qmdl_store_path.clone(),
        port: config.port,
        debug_mode: config.debug_mode,
        ui_level: config.ui_level,
        enable_dummy_analyzer: config.enable_dummy_analyzer,
        colorblind_mode: config.colorblind_mode,
        full_background_color: config.full_background_color,
        show_screen_overlay: config.show_screen_overlay,
        enable_animation: config.enable_animation,
    };

    if config.colorblind_mode {
        display_color = framebuffer::Color565::Blue;
    } else {
        display_color = framebuffer::Color565::Pink;
    }

    task_tracker.spawn_blocking(move || {
        let mut fb: Framebuffer = Framebuffer::new();
        // this feels wrong, is there a more rusty way to do this?
        let mut img: Option<&[u8]> = None;
        if display_level == 2 {
            img = Some(IMAGE_DIR.get_file("orca.gif").expect("failed to read orca.gif").contents());
        } else if display_level == 3 {
            img = Some(IMAGE_DIR.get_file("eff.png").expect("failed to read eff.png").contents());
        }
        
        // Keep track of the current display state to handle rendering
        let mut current_state: framebuffer::DisplayState = framebuffer::DisplayState::DetailedStatus { 
            qmdl_name: "RAYHUNTER".to_string(),
            qmdl_size_bytes: 0,
            analysis_size_bytes: 0,
            num_warnings: 0,
            last_warning: None,
        };
        
        // Add a timer to periodically cycle to the detailed status view
        let _detail_timer_counter = 0;
        let _detail_display_interval = 100; // Show details every ~10 seconds (100 * 100ms)
        let _detail_display_duration = 50;  // Show details for ~5 seconds (50 * 100ms)
        
        // Create a blocking runtime for occasional filesystem operations
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime");
            
        // Draw black screen initially when UI is hidden
        if !UI_VISIBLE.load(Ordering::Relaxed) {
            // Draw a completely black screen to save power
            let black_buffer = fb.create_buffer(framebuffer::Color565::Black as u16);
            fb.write_buffer(&black_buffer).unwrap();
        }
            
        loop {
            match ui_shutdown_rx.try_recv() {
                Ok(_) => {
                    info!("received UI shutdown");
                    break;
                },
                Err(TryRecvError::Empty) => {},
                Err(e) => panic!("error receiving shutdown message: {e}")
            }
            match ui_update_rx.try_recv() {
                    Ok(state) => {
                        // If we receive a detailed status update, use it
                        // For other updates, convert to detailed status when appropriate
                        match &state {
                            framebuffer::DisplayState::DetailedStatus { .. } => {
                                current_state = state.clone();
                            },
                            _ => {
                                // Keep using current state if it's already detailed status
                                if let framebuffer::DisplayState::DetailedStatus { .. } = &current_state {
                                    // Only update the color
                                    display_color = state.clone().into();
                                } else {
                                    current_state = state.clone();
                                    display_color = current_state.clone().into();
                                }
                            }
                        }
                    },
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {},
                    Err(e) => error!("error receiving framebuffer update message: {e}")
            }

            // Only render UI when visible
            if UI_VISIBLE.load(Ordering::Relaxed) {
                // Signal for debug - helpful to see when this condition changes
                static mut LAST_UI_VISIBLE: bool = true;
                unsafe {
                    if LAST_UI_VISIBLE != true {
                        info!("UI is now VISIBLE");
                        LAST_UI_VISIBLE = true;
                    }
                }
                
                // Handle UI display based on level setting
                match display_level {
                    2 => {
                        fb.draw_gif(img.unwrap());
                    },
                    3 => {
                        fb.draw_img(img.unwrap())
                    },
                    128 => {
                        fb.draw_line(framebuffer::Color565::Cyan, 128);
                        fb.draw_line(framebuffer::Color565::Pink, 102);
                        fb.draw_line(framebuffer::Color565::White, 76);
                        fb.draw_line(framebuffer::Color565::Pink, 50);
                        fb.draw_line(framebuffer::Color565::Cyan, 25);
                    },
                    1 | _ => {
                        // If we have an analysis warning, use the new draw_warning method
                        match &current_state {
                            framebuffer::DisplayState::AnalysisWarning { message, severity } => {
                                fb.draw_warning(message, severity, display_color);
                            },
                            framebuffer::DisplayState::NoQmdlData => {
                                // Draw a red background with white text for the error message
                                let error_message = "No QMDL data is being recorded";
                                fb.draw_warning(error_message, "Error", framebuffer::Color565::Red);
                            },
                            framebuffer::DisplayState::DetailedStatus { 
                                qmdl_name, 
                                qmdl_size_bytes, 
                                analysis_size_bytes,
                                num_warnings,
                                last_warning
                            } => {
                                // Get the latest data directly from the store on occasion
                                // to ensure we always show the most current data
                                let updated_qmdl_name: String;
                                let updated_size: usize;
                                let updated_analysis_size: usize;
                                let updated_warnings: usize = *num_warnings;
                                let updated_last_warning = last_warning.clone();
                                let _last_msg_time: Option<String> = None;
                                
                                // Try to get fresh data from qmdl_store periodically
                                // This ensures we're showing the latest data even if messaging fails
                                let result = rt.block_on(async {
                                    // Only try to load the store if not in debug mode
                                    let store_result = RecordingStore::load(&qmdl_store_path).await;
                                    if let Ok(store) = store_result {
                                        // If there's an active recording, get its details
                                        if let Some(entry) = store.manifest.entries.last() {
                                            // Use the actual values from the last entry
                                            return Some((
                                                entry.start_time.format("%a %b %d %Y %H:%M:%S %Z").to_string(),
                                                entry.qmdl_size_bytes,
                                                entry.analysis_size_bytes,
                                                entry.last_message_time.map(|t| t.format("%a %b %d %Y %H:%M:%S %Z").to_string())
                                            ));
                                        }
                                    }
                                    None
                                });
                                
                                // Use the fresh data if available, otherwise use the current state
                                if let Some((name, size, analysis_size, last_time)) = result {
                                    updated_qmdl_name = name;
                                    updated_size = size;
                                    updated_analysis_size = analysis_size;
                                    let last_msg_time_value = last_time;
                                    
                                    // Update display with the latest information from the qmdl_store
                                    fb.draw_detailed_status(
                                        &updated_qmdl_name, 
                                        updated_size, 
                                        updated_analysis_size,
                                        updated_warnings,
                                        updated_last_warning.as_deref(),
                                        display_color,
                                        &config_clone,
                                        last_msg_time_value.as_deref()
                                    );
                                } else {
                                    // Fallback to the values in the current state
                                    fb.draw_detailed_status(
                                        qmdl_name, 
                                        *qmdl_size_bytes, 
                                        *analysis_size_bytes,
                                        *num_warnings,
                                        last_warning.as_deref(),
                                        display_color,
                                        &config_clone,
                                        None
                                    );
                                }
                            },
                            _ => {
                                // Always use a detailed status display for any other state
                                fb.draw_detailed_status(
                                    "RAYHUNTER", 
                                    0, 
                                    0,
                                    0,
                                    None,
                                    display_color,
                                    &config_clone,
                                    None
                                );
                            }
                        }
                    },
                }
            } else {
                // Signal for debug - helpful to see when this condition changes
                static mut LAST_UI_VISIBLE: bool = false;
                unsafe {
                    if LAST_UI_VISIBLE != false {
                        info!("UI is now HIDDEN");
                        LAST_UI_VISIBLE = false;
                    }
                }
                
                // When UI is hidden, don't draw anything
                // Let the device's standard UI show through
            }
            
            // Sleep a bit to avoid consuming too much CPU
            // When UI is hidden, we can sleep longer to save even more power
            if UI_VISIBLE.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            } else {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    })
}

// New function to monitor the menu button
fn monitor_menu_button(task_tracker: &TaskTracker) -> JoinHandle<()> {
    task_tracker.spawn_blocking(move || {
        // The hexdump clearly shows the menu button is on event1 with code 0x0A
        let input_path = "/dev/input/event1";
        
        // Button state tracking
        let mut button_down_time: Option<Instant> = None;
        let required_hold_time = Duration::from_secs(5);
        
        // Specific menu button code from hexdump
        let menu_button_code: u16 = 0x0A;
        
        info!("Menu button monitor started, watching {} for events", input_path);
        
        loop {
            // Try to open the input device
            let mut file = match StdFile::open(input_path) {
                Ok(f) => f,
                Err(e) => {
                    error!("Failed to open input device {}: {}", input_path, e);
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            
            info!("Successfully opened input device: {}", input_path);
            
            // Buffer to read input events - 16 bytes per event as seen in hexdump
            let mut event_buffer = [0u8; 16];
            
            loop {
                // Check if button has been held long enough (if button is being pressed)
                if let Some(start_time) = button_down_time {
                    let elapsed = start_time.elapsed();
                    
                    // Once we've held for 5 seconds, toggle immediately
                    if elapsed >= required_hold_time {
                        // Toggle the UI visibility
                        let current = UI_VISIBLE.load(Ordering::Relaxed);
                        let new_state = !current;
                        UI_VISIBLE.store(new_state, Ordering::Relaxed);
                        info!("UI visibility toggled to: {} (held for 5+ seconds)", new_state);
                        
                        // Reset the timer so we don't keep toggling
                        button_down_time = None;
                    }
                }
                
                // Read input events from device - exactly 16 bytes as seen in hexdump
                match file.read_exact(&mut event_buffer) {
                    Ok(_) => {
                        // Parse the event format according to the hexdump
                        // Event type at bytes 8-9
                        let event_type = event_buffer[8] as u16 | ((event_buffer[9] as u16) << 8);
                        // Key code at bytes 10-11
                        let key_code = event_buffer[10] as u16 | ((event_buffer[11] as u16) << 8);
                        // Value at bytes 12-15
                        let value = event_buffer[12] as i32 | 
                                   ((event_buffer[13] as i32) << 8) | 
                                   ((event_buffer[14] as i32) << 16) | 
                                   ((event_buffer[15] as i32) << 24);
                        
                        // Log the event for debugging
                        info!("Input event: type={}, code=0x{:02x}, value={}", 
                              event_type, key_code, value);
                        
                        // Check for menu button events (type=1, code=0x0A)
                        if event_type == 1 && key_code == menu_button_code {
                            if value == 1 {
                                // Button pressed down - start timing
                                if button_down_time.is_none() {
                                    button_down_time = Some(Instant::now());
                                    info!("Menu button pressed, starting timer");
                                }
                            } else if value == 0 {
                                // Button released
                                button_down_time = None;
                                info!("Menu button released");
                            }
                        }
                        
                        // Read the padding data (always 16 bytes after the event)
                        // This is consistently shown in the hexdump pattern
                        if let Err(e) = file.read_exact(&mut event_buffer) {
                            error!("Failed to read padding data: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        error!("Failed to read input event: {}", e);
                        break;
                    }
                }
                
                // Short sleep to avoid CPU spinning
                std::thread::sleep(Duration::from_millis(10));
            }
            
            // If we get here, there was an error reading. Wait and try to reopen.
            std::thread::sleep(Duration::from_secs(3));
        }
    })
}

#[tokio::main]
async fn main() -> Result<(), RayhunterError> {
    // We use the SimpleLogger simply to turn stdout logs into a log
    // file.
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .with_utc_timestamps()
        .env()
        .init()
        .unwrap();
    
    info!("R A Y H U N T E R");
    
    // Log the special version for verification
    info!("Starting rayhunter daemon - VERSION: {}", VERSION);
    
    // Parse the args from the commandline.
    let args = parse_args();
    
    // Parse the configuration file
    let config = parse_config(&args.config_path).unwrap_or_else(|err| {
        panic!("Error parsing config: {err}")
    });

    // TaskTrackers give us an interface to spawn tokio threads, and then
    // eventually await all of them ending
    let task_tracker = TaskTracker::new();

    // Start monitoring the menu button for UI toggle
    if !config.debug_mode {
        info!("Starting menu button monitor");
        monitor_menu_button(&task_tracker);
    }

    let qmdl_store_lock = Arc::new(RwLock::new(init_qmdl_store(&config).await?));
    let (tx, rx) = mpsc::channel::<DiagDeviceCtrlMessage>(1);
    let (ui_update_tx, ui_update_rx) = mpsc::channel::<framebuffer::DisplayState>(1);
    let (analysis_tx, analysis_rx) = mpsc::channel::<AnalysisCtrlMessage>(5);
    let mut maybe_ui_shutdown_tx = None;
    if !config.debug_mode {
        let (ui_shutdown_tx, ui_shutdown_rx) = oneshot::channel();
        maybe_ui_shutdown_tx = Some(ui_shutdown_tx);
        let mut dev = DiagDevice::new().await
            .map_err(RayhunterError::DiagInitError)?;
        dev.config_logs().await
            .map_err(RayhunterError::DiagInitError)?;

        info!("Starting Diag Thread");
        run_diag_read_thread(&task_tracker, dev, rx, ui_update_tx.clone(), qmdl_store_lock.clone(), config.enable_dummy_analyzer);
        info!("Starting UI");
        update_ui(&task_tracker, &config, ui_shutdown_rx, ui_update_rx);
    }
    let (server_shutdown_tx, server_shutdown_rx) = oneshot::channel::<()>();
    info!("create shutdown thread");
    let analysis_status_lock = Arc::new(RwLock::new(AnalysisStatus::default()));
    run_analysis_thread(&task_tracker, analysis_rx, qmdl_store_lock.clone(), analysis_status_lock.clone(), config.enable_dummy_analyzer);
    run_ctrl_c_thread(&task_tracker, tx.clone(), server_shutdown_tx, maybe_ui_shutdown_tx, qmdl_store_lock.clone(), analysis_tx.clone());
    run_server(&task_tracker, &config, qmdl_store_lock.clone(), server_shutdown_rx, ui_update_tx, tx, analysis_tx, analysis_status_lock).await;

    task_tracker.close();
    task_tracker.wait().await;

    info!("see you space cowboy...");
    Ok(())
}
