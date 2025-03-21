use std::pin::pin;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rayhunter::diag_device::DiagDevice;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender};
use rayhunter::qmdl::QmdlWriter;
use log::{error, info};
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use tokio_util::task::TaskTracker;
use futures::TryStreamExt;

use crate::framebuffer;
use crate::qmdl_store::RecordingStore;
use crate::server::ServerState;
use crate::analysis::AnalysisWriter;

pub enum DiagDeviceCtrlMessage {
    StopRecording,
    StartRecording((QmdlWriter<File>, File)),
    Exit,
}

// Helper struct to track warning state
#[derive(Clone, Default)]
struct WarningStats {
    count: usize,
    last_message: Option<String>,
}

// Direct UI update function without references
async fn send_detailed_status_direct(
    entry_name: String, 
    qmdl_size_bytes: usize,
    analysis_size_bytes: usize,
    warning_stats: WarningStats,
    ui_update_sender: &Sender<framebuffer::DisplayState>,
) -> Result<(), &'static str> {
    // Send the detailed status update
    ui_update_sender.send(framebuffer::DisplayState::DetailedStatus {
        qmdl_name: entry_name,
        qmdl_size_bytes,
        analysis_size_bytes,
        num_warnings: warning_stats.count,
        last_warning: warning_stats.last_message,
    }).await
    .map_err(|_| "couldn't send detailed status update")
}

pub fn run_diag_read_thread(
    task_tracker: &TaskTracker,
    mut dev: DiagDevice,
    mut ctrl_rx: Receiver<DiagDeviceCtrlMessage>,
    ui_update_sender: Sender<framebuffer::DisplayState>,
    qmdl_store_lock: Arc<RwLock<RecordingStore>>,
    enable_dummy_analyzer: bool,
) {
    task_tracker.spawn(async move {
        let mut maybe_qmdl_writer: Option<QmdlWriter<File>> = None;
        let mut maybe_analysis_writer: Option<AnalysisWriter> = None;
        let mut diag_stream = pin!(dev.as_stream().into_stream());

        loop {
            tokio::select! {
                maybe_msg = ctrl_rx.recv() => {
                    if let Some(msg) = maybe_msg {
                        match msg {
                            DiagDeviceCtrlMessage::StartRecording((new_writer, new_analysis_file)) => {
                                maybe_qmdl_writer = Some(new_writer);
                                if let Some(analysis_writer) = maybe_analysis_writer {
                                    analysis_writer.close().await.expect("failed to close analysis writer");
                                }
                                maybe_analysis_writer = Some(AnalysisWriter::new(new_analysis_file, enable_dummy_analyzer).await
                                    .expect("failed to write to analysis file"));
                            },
                            DiagDeviceCtrlMessage::StopRecording => {
                                maybe_qmdl_writer = None;
                                if let Some(analysis_writer) = maybe_analysis_writer {
                                    analysis_writer.close().await.expect("failed to close analysis writer");
                                }
                                maybe_analysis_writer = None;
                            },
                            // None means all the Senders have been dropped, so it's
                            // time to go
                            DiagDeviceCtrlMessage::Exit => {
                                info!("Diag reader thread exiting...");
                                return Ok(());
                            },
                        }
                    } else {
                        info!("Diag reader thread control channel closed, exiting...");
                        return Ok(());
                    }
                },
                maybe_result = diag_stream.try_next() => {
                    match maybe_result {
                        // We got a new container
                        Ok(Some(container)) => {
                            if let Some(qmdl_writer) = maybe_qmdl_writer.as_mut() {
                                qmdl_writer.write_container(&container).await
                                    .expect("failed to write to qmdl file");
                            }
                            if let Some(analysis_writer) = maybe_analysis_writer.as_mut() {
                                let analysis_output = analysis_writer.analyze(container).await
                                    .expect("failed to analyze container");
                                let (analysis_file_len, heuristic_warning) = analysis_output;
                                let mut qmdl_store = qmdl_store_lock.write().await;
                                let index = qmdl_store.current_entry.expect("DiagDevice had qmdl_writer, but QmdlStore didn't have current entry???");
                                qmdl_store.update_entry_analysis_size(index, analysis_file_len as usize).await
                                    .expect("failed to update analysis file size");
                                
                                // Get warning statistics
                                let warning_stats = WarningStats {
                                    count: analysis_writer.get_warning_count(),
                                    last_message: analysis_writer.get_last_warning().map(|w| w.message.clone()),
                                };
                                
                                if heuristic_warning {
                                    info!("a heuristic triggered on this run!");
                                    // Get the warning details from the analysis writer
                                    if let Some(warning_details) = analysis_writer.get_last_warning() {
                                        ui_update_sender.send(framebuffer::DisplayState::AnalysisWarning {
                                            message: warning_details.message.clone(),
                                            severity: warning_details.severity.clone(),
                                        }).await
                                        .expect("couldn't send ui update message with warning details");
                                    } else {
                                        // Fallback to the generic warning if we can't get details
                                        ui_update_sender.send(framebuffer::DisplayState::WarningDetected).await
                                            .expect("couldn't send ui update message");
                                    }
                                }
                                
                                // Track and update file size changes
                                if let Some(qmdl_writer) = maybe_qmdl_writer.as_ref() {
                                    let updated_size = qmdl_writer.total_written;
                                    
                                    // Update the file size in the qmdl store
                                    if let Some(index) = qmdl_store.current_entry {
                                        if qmdl_store.manifest.entries[index].qmdl_size_bytes != updated_size {
                                            // Only update if size has changed
                                            qmdl_store.update_entry_qmdl_size(index, updated_size).await
                                                .expect("failed to update qmdl file size");
                                            
                                            // Get latest timestamps and update last_message_time
                                            if let Err(e) = qmdl_store.update_entry_last_message_time(index, chrono::Local::now()).await {
                                                error!("failed to update last message time: {}", e);
                                            }
                                            
                                            // Get warning statistics for UI update
                                            let warning_stats = WarningStats {
                                                count: analysis_writer.get_warning_count(),
                                                last_message: analysis_writer.get_last_warning().map(|w| w.message.clone()),
                                            };
                                            
                                            // Send UI update more aggressively - update on every change
                                            // This ensures the display always shows current data
                                            let entry = &qmdl_store.manifest.entries[index];
                                            let formatted_timestamp = entry.start_time.format("%a %b %d %Y %H:%M:%S %Z").to_string();
                                            
                                            let _ = send_detailed_status_direct(
                                                formatted_timestamp,
                                                updated_size,
                                                entry.analysis_size_bytes,
                                                warning_stats,
                                                &ui_update_sender
                                            ).await;
                                        }
                                    }
                                }
                            }
                        },
                        // No more containers but the stream is still active
                        Ok(None) => {
                            info!("Diag stream ended but channel still open");
                            // Continue the loop to wait for more messages
                        },
                        // Error reading from the stream
                        Err(err) => {
                            error!("error reading diag device: {}", err);
                            return Err(err);
                        }
                    }
                }
            }
        }
    });
}

pub async fn start_recording(State(state): State<Arc<ServerState>>) -> Result<(StatusCode, String), (StatusCode, String)> {
    if state.debug_mode {
        return Err((StatusCode::FORBIDDEN, "server is in debug mode".to_string()));
    }
    let mut qmdl_store = state.qmdl_store_lock.write().await;
    let (qmdl_file, analysis_file) = qmdl_store.new_entry().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't create new qmdl entry: {}", e)))?;
    let qmdl_writer = QmdlWriter::new(qmdl_file);
    state.diag_device_ctrl_sender.send(DiagDeviceCtrlMessage::StartRecording((qmdl_writer, analysis_file))).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send stop recording message: {}", e)))?;

    // Send recording status to change icon
    let display_state: framebuffer::DisplayState;
    if state.colorblind_mode { 
        display_state = framebuffer::DisplayState::RecordingCBM;
    } else {
        display_state = framebuffer::DisplayState::Recording;
    }
    state.ui_update_sender.send(display_state).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send ui update message: {}", e)))?;
    
    // Also send a detailed status message if we have a current entry
    if qmdl_store.current_entry.is_some() {
        let entry_index = qmdl_store.current_entry.unwrap();
        let entry = &qmdl_store.manifest.entries[entry_index];
        
        // Use the actual timestamp with proper formatting that matches the web interface
        let formatted_timestamp = entry.start_time.format("%a %b %d %Y %H:%M:%S %Z").to_string();
        
        // Send initial detailed status with empty warning stats
        let warning_stats = WarningStats::default();
        
        send_detailed_status_direct(
            formatted_timestamp, // Use properly formatted timestamp
            entry.qmdl_size_bytes,
            entry.analysis_size_bytes,
            warning_stats,
            &state.ui_update_sender
        ).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send detailed status update: {}", e)))?;
    }

    Ok((StatusCode::ACCEPTED, "ok".to_string()))
}

pub async fn stop_recording(State(state): State<Arc<ServerState>>) -> Result<(StatusCode, String), (StatusCode, String)> {
    if state.debug_mode {
        return Err((StatusCode::FORBIDDEN, "server is in debug mode".to_string()));
    }
    
    // Get detailed status before closing the current entry
    let mut qmdl_store = state.qmdl_store_lock.write().await;
    if qmdl_store.current_entry.is_some() {
        let entry_index = qmdl_store.current_entry.unwrap();
        let entry = &qmdl_store.manifest.entries[entry_index];
        
        // Send final status update with empty warning stats
        let warning_stats = WarningStats::default();
        
        send_detailed_status_direct(
            entry.name.clone(),
            entry.qmdl_size_bytes,
            entry.analysis_size_bytes,
            warning_stats,
            &state.ui_update_sender
        ).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send detailed status update: {}", e)))?;
    }
    
    qmdl_store.close_current_entry().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't close current qmdl entry: {}", e)))?;
    state.diag_device_ctrl_sender.send(DiagDeviceCtrlMessage::StopRecording).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send stop recording message: {}", e)))?;
    state.ui_update_sender.send(framebuffer::DisplayState::Paused).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't send ui update message: {}", e)))?;
    Ok((StatusCode::ACCEPTED, "ok".to_string()))
}

pub async fn get_analysis_report(State(state): State<Arc<ServerState>>, Path(qmdl_name): Path<String>) -> Result<Response, (StatusCode, String)> {
    let qmdl_store = state.qmdl_store_lock.read().await;
    let (entry_index, _) = if qmdl_name == "live" {
        qmdl_store.get_current_entry().ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "No QMDL data's being recorded to analyze, try starting a new recording!".to_string()
        ))?
    } else {
        qmdl_store.entry_for_name(&qmdl_name).ok_or((
            StatusCode::NOT_FOUND,
            format!("Couldn't find QMDL entry with name \"{}\"", qmdl_name)
        ))?
    };
    let analysis_file = qmdl_store.open_entry_analysis(entry_index).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{:?}", e)))?;
    let analysis_stream = ReaderStream::new(analysis_file);

    let headers = [(CONTENT_TYPE, "application/x-ndjson")];
    let body = Body::from_stream(analysis_stream);
    Ok((headers, body).into_response())
}
