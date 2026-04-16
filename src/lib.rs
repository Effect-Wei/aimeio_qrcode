mod camera;
mod config;
mod debug_window;
mod display;
mod ffi_catcher;
mod ini_parser;
mod qr_decoder;

use camera::{CameraWorkerHooks, enumerate_cameras, start_camera_thread};
use config::{ConfigSnapshot, current_cam_id, current_config, list_cameras_enabled, reload_config};
use ffi_catcher::{AimeError, HRESULT, ffi_catch};

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{LazyLock, RwLock};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static RADIO_ON: AtomicBool = AtomicBool::new(true);
static SHOW_WINDOW_FROM_LED: AtomicBool = AtomicBool::new(false);
static LAST_LOGGED_CAM_ID: AtomicI32 = AtomicI32::new(i32::MIN);

struct CardResult {
    aime_id_present: bool,
    aime_id: [u8; 10],
    felica_id_present: bool,
    felica_id: u64,
}

static CARD_RESULT: LazyLock<RwLock<CardResult>> = LazyLock::new(|| {
    RwLock::new(CardResult {
        aime_id_present: false,
        aime_id: [0; 10],
        felica_id_present: false,
        felica_id: 0,
    })
});

#[repr(C)]
pub struct AimeIoVfdState {
    _opaque: [u8; 0],
}

fn runtime_config() -> ConfigSnapshot {
    let mut snapshot = current_config();
    snapshot.configured_show_window =
        snapshot.configured_show_window || SHOW_WINDOW_FROM_LED.load(Ordering::SeqCst);
    snapshot
}

fn is_white_led(r: u8, g: u8, b: u8) -> bool {
    r > 0 && r == g && g == b
}

fn clear_aime_result() {
    if let Ok(mut res) = CARD_RESULT.write() {
        res.aime_id = [0; 10];
        res.aime_id_present = false;
        res.felica_id = 0;
        res.felica_id_present = false;
    }
}

fn store_aime_result(id: [u8; 10]) {
    if let Ok(mut res) = CARD_RESULT.write() {
        res.aime_id = id;
        res.aime_id_present = true;
        res.felica_id = 0;
        res.felica_id_present = false;
    }
}

fn store_felica_result(idm: u64) {
    if let Ok(mut res) = CARD_RESULT.write() {
        res.aime_id = [0; 10];
        res.aime_id_present = false;
        res.felica_id = idm;
        res.felica_id_present = true;
    }
}

fn log_cam_id_if_changed() {
    let cam_id = current_cam_id();
    let previous = LAST_LOGGED_CAM_ID.swap(cam_id, Ordering::SeqCst);
    if previous != cam_id {
        println!("AimeIO DLL: Current camera ID is {}", cam_id);
    }
}

fn init_camera_thread() {
    start_camera_thread(CameraWorkerHooks {
        get_config_snapshot: runtime_config,
        is_radio_on: || RADIO_ON.load(Ordering::SeqCst),
        on_aime_found: store_aime_result,
        on_felica_found: store_felica_result,
        on_card_lost: clear_aime_result,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_get_api_version() -> u16 {
    0x0101
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_init() -> HRESULT {
    ffi_catch(|| {
        reload_config();

        if INITIALIZED.load(Ordering::SeqCst) {
            return Ok(());
        }

        if list_cameras_enabled() {
            enumerate_cameras();
        }
        init_camera_thread();
        log_cam_id_if_changed();

        INITIALIZED.store(true, Ordering::SeqCst);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aime_io_nfc_poll(_unit_no: u8) -> HRESULT {
    ffi_catch(|| {
        reload_config();
        log_cam_id_if_changed();
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

        let card_result = CARD_RESULT.read().map_err(|_| AimeError::Fail)?;
        if !card_result.aime_id_present {
            return Err(AimeError::NotPresent);
        }

        unsafe {
            std::ptr::copy_nonoverlapping(card_result.aime_id.as_ptr(), luid, 10);
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aime_io_nfc_get_felica_id(_unit_no: u8, idm: *mut u64) -> HRESULT {
    ffi_catch(|| {
        if idm.is_null() {
            return Err(AimeError::InvalidArg);
        }

        let card_result = CARD_RESULT.read().map_err(|_| AimeError::Fail)?;
        if !card_result.felica_id_present {
            return Err(AimeError::NotPresent);
        }

        unsafe {
            *idm = card_result.felica_id;
        }
        Ok(())
    })
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
