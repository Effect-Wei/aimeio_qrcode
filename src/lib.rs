mod debug_window;
mod ffi_catcher;
mod ini_parser;
mod qr_decoder;
use debug_window::DebugWindow;
use ffi_catcher::{AimeError, HRESULT, ffi_catch};
use ini_parser::IniParser;
use qr_decoder::QrScanner;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{LazyLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use nokhwa::pixel_format::LumaFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::{Camera, query};

const CAM_WIDTH: u32 = 640;
const CAM_HEIGHT: u32 = 480;
const FPS: u32 = 30;
const MAX_ERRORS: u32 = 10;
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static LIST_CAMERAS: AtomicBool = AtomicBool::new(false);
static RADIO_ON: AtomicBool = AtomicBool::new(true);
static SHOW_WINDOW: AtomicBool = AtomicBool::new(false);
static SHOW_WINDOW_FROM_LED: AtomicBool = AtomicBool::new(false);
static CAM_ID: AtomicI32 = AtomicI32::new(0);

struct AimeResult {
    aime_id_present: bool,
    aime_id: [u8; 10],
}

static AIME_RESULT: LazyLock<RwLock<AimeResult>> = LazyLock::new(|| {
    RwLock::new(AimeResult {
        aime_id_present: false,
        aime_id: [0; 10],
    })
});

static INI: LazyLock<RwLock<IniParser>> = LazyLock::new(|| {
    let path_os =
        env::var_os("SEGATOOLS_CONFIG_PATH").unwrap_or_else(|| OsString::from(".\\segatools.ini"));
    let path = PathBuf::from(path_os);
    RwLock::new(IniParser::new(&path).unwrap())
});

#[repr(C)]
pub struct AimeIoVfdState {
    _opaque: [u8; 0],
}

/* ========================================================================= */
/* =========================== 内部功能函数 ============================ */
/* ========================================================================= */

fn enumerate_cameras() {
    let cameras = match query(ApiBackend::Auto) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("AimeIO DLL: Cannot query cameras: {}", e);
            return;
        }
    };
    if cameras.is_empty() {
        println!("AimeIO DLL: No available cameras found!");
        return;
    }

    println!("AimeIO DLL: Found {} camera(s):\n", cameras.len());

    for (i, info) in cameras.iter().enumerate() {
        println!("AimeIO DLL: --- Camera #{} ---", i);
        println!("AimeIO DLL: Index : {}", info.index());
        println!("AimeIO DLL: Name   : {}", info.human_name());
        println!("AimeIO DLL: Description   : {}", info.description());
        if !info.misc().is_empty() {
            println!("AimeIO DLL: Misc   : {}", info.misc());
        }
        println!();
    }
}

fn init_camera(index: i32) -> Result<Camera, String> {
    // 1. 设置摄像头索引
    let camera_index = CameraIndex::Index(index as u32);

    // 2. 配置请求格式：
    // 我们指定 640x480 分辨率，最高帧率。
    // 这对于二维码扫描来说既清晰又轻量。
    let requested =
        RequestedFormat::new::<LumaFormat>(RequestedFormatType::Closest(CameraFormat::new(
            Resolution::new(CAM_WIDTH, CAM_HEIGHT),
            FrameFormat::YUYV,
            FPS,
        )));

    // 3. 创建摄像头实例
    // 可以在这里指定后端 API。如果你只针对 Windows，可以指定 ApiBackend::MSMF
    let mut camera = Camera::new(camera_index, requested)
        .map_err(|e| format!("Cannot create camera instance: {}", e))?;

    // 4. 打开视频流
    camera
        .open_stream()
        .map_err(|e| format!("Cannot open stream: {}", e))?;

    Ok(camera)
}

fn should_show_window() -> bool {
    SHOW_WINDOW.load(Ordering::SeqCst) || SHOW_WINDOW_FROM_LED.load(Ordering::SeqCst)
}

fn sync_debug_window(window: &mut Option<DebugWindow>) {
    let is_open = matches!(window.as_ref(), Some(w) if w.is_open());
    if should_show_window() {
        if !is_open {
            *window = DebugWindow::new(CAM_WIDTH as usize, CAM_HEIGHT as usize, FPS as usize);
        }
    } else if window.is_some() {
        *window = None;
    }
}

fn is_white_led(r: u8, g: u8, b: u8) -> bool {
    r > 0 && r == g && g == b
}

fn init_camera_thread() {
    // 计算每帧应该消耗的时间（例如 10fps -> 100ms 每帧）
    let frame_duration = Duration::from_secs_f32(1.0 / FPS as f32);

    thread::spawn(move || {
        let mut window = if should_show_window() {
            DebugWindow::new(CAM_WIDTH as usize, CAM_HEIGHT as usize, FPS as usize)
        } else {
            None
        };
        let mut scanner = QrScanner::new(CAM_WIDTH, CAM_HEIGHT, FPS as f64);

        // --- 外层循环：负责重连 ---
        loop {
            let current_cam_id = CAM_ID.load(Ordering::SeqCst);
            let mut error_count: u32 = 0;
            let mut absent_count: u32 = 0;

            let mut camera = match init_camera(current_cam_id) {
                Ok(c) => c,
                Err(e) => {
                    println!("AimeIO DLL: Camera initialization error: {}", e);
                    thread::sleep(Duration::from_secs(2)); // 初始化失败，2秒后重试
                    continue;
                }
            };

            loop {
                if current_cam_id != CAM_ID.load(Ordering::SeqCst) {
                    println!(
                        "AimeIO DLL: Camera ID changed ({} -> {}). Reinitializing camera...",
                        current_cam_id,
                        CAM_ID.load(Ordering::SeqCst)
                    );
                    break; // 跳出内层循环，外层循环会重新初始化摄像头
                }

                let loop_start = Instant::now();
                sync_debug_window(&mut window);

                // 1. 尝试获取一帧
                let frame_result = camera.frame();

                match frame_result {
                    Ok(frame) => {
                        error_count = 0; // 重置错误计数

                        if RADIO_ON.load(Ordering::SeqCst) {
                            // 2. 二维码识别逻辑 - 根据帧格式分层处理
                            let found_id: Option<[u8; 10]> = scanner.decode_qr(&frame, &mut window);

                            if let Some(id) = found_id {
                                absent_count = 0;

                                // 获取写锁更新结果
                                if let Ok(mut res) = AIME_RESULT.write() {
                                    res.aime_id = id;
                                    res.aime_id_present = true;
                                }
                            } else {
                                // 如果没扫到，根据逻辑清除 present 状态
                                absent_count = absent_count.saturating_add(1);
                                if absent_count >= MAX_ERRORS {
                                    if let Ok(mut res) = AIME_RESULT.write() {
                                        res.aime_id = [0; 10];
                                        res.aime_id_present = false;
                                    }
                                }
                            }
                        } else if should_show_window() {
                            let _ = scanner.decode_qr(&frame, &mut window);
                        } else {
                            // 如果扫描停止，也清除 present 状态
                            if let Ok(mut res) = AIME_RESULT.write() {
                                res.aime_id = [0; 10];
                                res.aime_id_present = false;
                            }
                        }
                    }
                    Err(_) => {
                        error_count += 1;
                        if error_count >= 5 {
                            break; // 连续失败，断开连接，跳到外层循环重连
                        }
                    }
                }

                // 3. FPS 控制
                let elapsed = loop_start.elapsed();
                thread::sleep(frame_duration.saturating_sub(elapsed));
            }

            // 发生断开，清理资源并等待重连
            drop(camera);
            thread::sleep(Duration::from_secs(2));
        }
    });
}

/* ========================================================================= */
/* =========================== 外部 C-FFI 接口 ============================ */
/* ========================================================================= */

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_get_api_version() -> u16 {
    0x0101
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_init() -> HRESULT {
    ffi_catch(|| {
        CAM_ID.store(
            INI.read().unwrap().get_int("aimeio", "camId", 0),
            Ordering::SeqCst,
        );
        SHOW_WINDOW.store(
            INI.read().unwrap().get_int("aimeio", "showWindow", 0) == 1,
            Ordering::SeqCst,
        );
        LIST_CAMERAS.store(
            INI.read().unwrap().get_int("aimeio", "listCameras", 0) == 1,
            Ordering::SeqCst,
        );

        if INITIALIZED.load(Ordering::SeqCst) {
            return Ok(());
        }

        if LIST_CAMERAS.load(Ordering::SeqCst) {
            enumerate_cameras();
        }
        init_camera_thread();

        INITIALIZED.store(true, Ordering::SeqCst);
        // 初始化成功

        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_nfc_poll(_unit_no: u8) -> HRESULT {
    ffi_catch(|| {
        let _ = INI.write().unwrap().reload();
        CAM_ID.store(
            INI.read().unwrap().get_int("aimeio", "camId", 0),
            Ordering::SeqCst,
        );
        SHOW_WINDOW.store(
            INI.read().unwrap().get_int("aimeio", "showWindow", 0) == 1,
            Ordering::SeqCst,
        );
        println!(
            "AimeIO DLL: Current camera ID is {}",
            CAM_ID.load(Ordering::SeqCst)
        );
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_get_aime_id(
    _unit_no: u8,
    luid: *mut u8,
    luid_size: usize,
) -> HRESULT {
    ffi_catch(|| {
        if luid.is_null() || luid_size != 10 {
            return Err(AimeError::InvalidArg);
        }

        let aime_result = AIME_RESULT.read().map_err(|_| AimeError::Fail)?;
        if !aime_result.aime_id_present {
            return Err(AimeError::NotPresent);
        }

        unsafe {
            std::ptr::copy_nonoverlapping(aime_result.aime_id.as_ptr(), luid, 10);
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_get_felica_id(_unit_no: u8, _idm: *mut u64) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_get_mifare_uid(
    _unit_no: u8,
    _uid: *mut u8,
    _uid_size: usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_mifare_select(
    _unit_no: u8,
    _uid: *const u8,
    _uid_size: usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_mifare_set_key(
    _unit_no: u8,
    _key_type: u8,
    _key: *const u8,
    _key_size: usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_mifare_authenticate(
    _unit_no: u8,
    _key_type: u8,
    _payload: *const u8,
    _payload_size: usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_mifare_read_block(
    _unit_no: u8,
    _uid: *const u8,
    _uid_size: usize,
    _block_no: u8,
    _block: *mut u8,
    _block_size: usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_felica_transact(
    _unit_no: u8,
    _req: *const u8,
    _req_size: usize,
    _res: *mut u8,
    _res_size: usize,
    _res_size_written: *mut usize,
) -> HRESULT {
    ffi_catch(|| Err(AimeError::NotPresent))
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_nfc_radio_on(_unit_no: u8) -> HRESULT {
    ffi_catch(|| {
        RADIO_ON.store(true, Ordering::SeqCst);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_nfc_radio_off(_unit_no: u8) -> HRESULT {
    ffi_catch(|| {
        RADIO_ON.store(false, Ordering::SeqCst);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_nfc_to_update_mode(_unit_no: u8) -> HRESULT {
    ffi_catch(|| Ok(()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_send_hex_data(
    _unit_no: u8,
    _payload: *const u8,
    payload_size: usize,
    status_out: *mut u8,
) -> HRESULT {
    ffi_catch(|| {
        if !status_out.is_null() {
            unsafe {
                *status_out = if payload_size == 0x2b { 0x20 } else { 0x00 };
            }
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_led_set_color(_unit_no: u8, r: u8, g: u8, b: u8) {
    SHOW_WINDOW_FROM_LED.store(is_white_led(r, g, b), Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_vfd_set_text(
    _text: *const u8,
    _text_len: usize,
    _state: *const AimeIoVfdState,
) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_vfd_set_state(_state: *const AimeIoVfdState) {}
