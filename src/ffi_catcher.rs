use std::io::Error;
use std::io::ErrorKind::{NotFound, TimedOut};
use std::panic::catch_unwind;

/* ========================================================================= */
/* ===================== HRESULT 与 FFI 错误处理层 ======================= */
/* ========================================================================= */

pub type HRESULT = i32;

pub const S_OK: HRESULT = 0;
pub const S_FALSE: HRESULT = 1;
pub const E_INVALIDARG: HRESULT = 0x80070057_u32 as i32;
pub const E_HANDLE: HRESULT = 0x80070006_u32 as i32;
pub const E_FAIL: HRESULT = 0x80004005_u32 as i32;
pub const ERROR_TIMEOUT: HRESULT = 0x800705B4_u32 as i32;

/// 自动将内部 Error 转换为标准的 HRESULT
impl From<AimeError> for HRESULT {
    fn from(err: AimeError) -> Self {
        match err {
            AimeError::InvalidArg => E_INVALIDARG,
            AimeError::NotPresent => S_FALSE,
            AimeError::Handle => E_HANDLE,
            AimeError::Timeout => ERROR_TIMEOUT,
            AimeError::Fail => E_FAIL,
        }
    }
}

/// 转换 SerialPort 错误为 AimeError
impl From<Error> for AimeError {
    fn from(err: Error) -> Self {
        match err.kind() {
            TimedOut => AimeError::Timeout,
            NotFound => AimeError::Handle,
            _ => AimeError::Fail,
        }
    }
}

/// 我们内部业务的安全 Error 枚举
#[derive(Debug)]
pub enum AimeError {
    InvalidArg,
    NotPresent,
    Handle,
    Timeout,
    Fail,
}

/// 【核心护城河】捕获 Rust 的 Panic 并将内部 Result 转为 HRESULT
/// 保护 C 宿主绝对不会因为 Rust 的 unwrap/数组越界 而崩溃
pub fn ffi_catch<F>(f: F) -> HRESULT
where
    F: FnOnce() -> Result<(), AimeError> + std::panic::UnwindSafe,
{
    match catch_unwind(f) {
        Ok(Ok(_)) => S_OK,
        Ok(Err(e)) => e.into(),
        Err(_) => {
            eprintln!("AimeIO DLL: internal panic caught!");
            E_FAIL
        }
    }
}
