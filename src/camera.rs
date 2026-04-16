use crate::config::{
    CAMERA_READ_ERROR_LIMIT, CAMERA_RETRY_DELAY_SECS, CARD_ABSENT_FRAME_LIMIT, ConfigSnapshot,
};
use crate::debug_window::DebugWindow;
use crate::display::{create_debug_windows, sync_debug_windows};
use crate::qr_decoder::QrScanner;

use std::thread;
use std::time::{Duration, Instant};

use nokhwa::pixel_format::LumaFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::{Buffer, Camera, query};

pub const CAM_WIDTH: u32 = 640;
pub const CAM_HEIGHT: u32 = 480;
pub const FPS: u32 = 30;

pub struct CameraWorkerHooks<GetConfigSnapshot, IsRadioOn, OnCardFound, OnCardLost>
where
    GetConfigSnapshot: Fn() -> ConfigSnapshot + Send + 'static,
    IsRadioOn: Fn() -> bool + Send + 'static,
    OnCardFound: Fn([u8; 10]) + Send + 'static,
    OnCardLost: Fn() + Send + 'static,
{
    pub get_config_snapshot: GetConfigSnapshot,
    pub is_radio_on: IsRadioOn,
    pub on_card_found: OnCardFound,
    pub on_card_lost: OnCardLost,
}

pub fn enumerate_cameras() {
    let cameras = match query(ApiBackend::Auto) {
        Ok(cameras) => cameras,
        Err(error) => {
            eprintln!("AimeIO DLL: Cannot query cameras: {}", error);
            return;
        }
    };

    if cameras.is_empty() {
        println!("AimeIO DLL: No available cameras found!");
        return;
    }

    println!("AimeIO DLL: Found {} camera(s):\n", cameras.len());

    for (index, info) in cameras.iter().enumerate() {
        println!("AimeIO DLL: --- Camera #{} ---", index);
        println!("AimeIO DLL: Index : {}", info.index());
        println!("AimeIO DLL: Name   : {}", info.human_name());
        println!("AimeIO DLL: Description   : {}", info.description());
        if !info.misc().is_empty() {
            println!("AimeIO DLL: Misc   : {}", info.misc());
        }
        println!();
    }
}

pub fn start_camera_thread<GetConfigSnapshot, IsRadioOn, OnCardFound, OnCardLost>(
    hooks: CameraWorkerHooks<GetConfigSnapshot, IsRadioOn, OnCardFound, OnCardLost>,
) where
    GetConfigSnapshot: Fn() -> ConfigSnapshot + Send + 'static,
    IsRadioOn: Fn() -> bool + Send + 'static,
    OnCardFound: Fn([u8; 10]) + Send + 'static,
    OnCardLost: Fn() + Send + 'static,
{
    let frame_duration = Duration::from_secs_f32(1.0 / FPS as f32);

    thread::spawn(move || {
        let initial_config = (hooks.get_config_snapshot)();
        let mut windows = if initial_config.configured_show_window {
            create_debug_windows(&initial_config.window, FPS as usize)
        } else {
            Vec::new()
        };
        let mut scanner = QrScanner::new(CAM_WIDTH, CAM_HEIGHT, FPS as f64);

        loop {
            let current_config = (hooks.get_config_snapshot)();
            let current_cam_id = current_config.cam_id;
            let mut error_count: u32 = 0;
            let mut absent_count: u32 = 0;

            let mut camera = match init_camera(current_cam_id) {
                Ok(camera) => camera,
                Err(error) => {
                    println!("AimeIO DLL: Camera initialization error: {}", error);
                    thread::sleep(Duration::from_secs(CAMERA_RETRY_DELAY_SECS));
                    continue;
                }
            };

            loop {
                let loop_config = (hooks.get_config_snapshot)();
                let loop_cam_id = loop_config.cam_id;
                let show_window = loop_config.configured_show_window;
                let radio_on = (hooks.is_radio_on)();

                if current_cam_id != loop_cam_id {
                    println!(
                        "AimeIO DLL: Camera ID changed ({} -> {}). Reinitializing camera...",
                        current_cam_id, loop_cam_id
                    );
                    break;
                }

                let loop_start = Instant::now();
                sync_debug_windows(&mut windows, &loop_config.window, show_window, FPS as usize);

                match camera.frame() {
                    Ok(frame) => {
                        error_count = 0;
                        process_frame(
                            &mut scanner,
                            &frame,
                            &mut windows,
                            &mut absent_count,
                            radio_on,
                            show_window,
                            &hooks.on_card_found,
                            &hooks.on_card_lost,
                        );
                    }
                    Err(error) => {
                        error_count += 1;
                        if error_count >= CAMERA_READ_ERROR_LIMIT {
                            eprintln!(
                                "AimeIO DLL: Camera frame read failed {} times, reconnecting: {}",
                                error_count, error
                            );
                            break;
                        }
                    }
                }

                let elapsed = loop_start.elapsed();
                thread::sleep(frame_duration.saturating_sub(elapsed));
            }

            drop(camera);
            thread::sleep(Duration::from_secs(CAMERA_RETRY_DELAY_SECS));
        }
    });
}

fn init_camera(index: i32) -> Result<Camera, String> {
    let camera_index = CameraIndex::Index(index as u32);
    let requested =
        RequestedFormat::new::<LumaFormat>(RequestedFormatType::Closest(CameraFormat::new(
            Resolution::new(CAM_WIDTH, CAM_HEIGHT),
            FrameFormat::YUYV,
            FPS,
        )));

    let mut camera = Camera::new(camera_index, requested)
        .map_err(|error| format!("Cannot create camera instance: {}", error))?;

    camera
        .open_stream()
        .map_err(|error| format!("Cannot open stream: {}", error))?;

    Ok(camera)
}

fn process_frame<OnCardFound, OnCardLost>(
    scanner: &mut QrScanner,
    frame: &Buffer,
    windows: &mut Vec<DebugWindow>,
    absent_count: &mut u32,
    radio_on: bool,
    show_window: bool,
    on_card_found: &OnCardFound,
    on_card_lost: &OnCardLost,
) where
    OnCardFound: Fn([u8; 10]),
    OnCardLost: Fn(),
{
    if radio_on {
        let found_id = scanner.decode_qr(frame, windows);

        if let Some(id) = found_id {
            *absent_count = 0;
            on_card_found(id);
        } else {
            *absent_count = absent_count.saturating_add(1);
            if *absent_count >= CARD_ABSENT_FRAME_LIMIT {
                on_card_lost();
            }
        }
    } else if show_window {
        let _ = scanner.decode_qr(frame, windows);
    } else {
        on_card_lost();
    }
}
