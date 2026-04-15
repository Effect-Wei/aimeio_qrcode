#[cfg(not(windows))]
fn main() {
    eprintln!("dll_tester is only supported on Windows.");
}

#[cfg(windows)]
mod app {
    use std::env;
    use std::ffi::{CString, c_char, c_void};
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

    type Hresult = i32;
    type Hmodule = *mut c_void;

    const S_OK: Hresult = 0;
    const S_FALSE: Hresult = 1;
    const E_INVALIDARG: Hresult = 0x80070057_u32 as i32;
    const E_HANDLE: Hresult = 0x80070006_u32 as i32;
    const E_FAIL: Hresult = 0x80004005_u32 as i32;
    const ERROR_TIMEOUT: Hresult = 0x800705B4_u32 as i32;

    type GetApiVersionFn = unsafe extern "C" fn() -> u16;
    type InitFn = unsafe extern "C" fn() -> Hresult;
    type PollFn = unsafe extern "C" fn(u8) -> Hresult;
    type GetAimeIdFn = unsafe extern "C" fn(u8, *mut u8, usize) -> Hresult;
    type RadioFn = unsafe extern "C" fn(u8) -> Hresult;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> Hmodule;
        fn GetProcAddress(h_module: Hmodule, lp_proc_name: *const c_char) -> *mut c_void;
        fn FreeLibrary(h_lib_module: Hmodule) -> i32;
    }

    struct Options {
        dll_path: PathBuf,
        cam_id: i32,
        show_window: bool,
        list_cameras: bool,
        unit_no: u8,
        read_interval: Duration,
        poll_interval: Option<Duration>,
    }

    impl Options {
        fn parse() -> Result<Self, String> {
            let mut dll_path = default_dll_path()?;
            let mut cam_id = 0;
            let mut show_window = false;
            let mut list_cameras = false;
            let mut unit_no = 0;
            let mut read_interval = Duration::from_millis(200);
            let mut poll_interval = None;

            let mut args = env::args().skip(1);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--dll" => {
                        let value = args.next().ok_or("--dll requires a path")?;
                        dll_path = PathBuf::from(value);
                    }
                    "--cam-id" => {
                        let value = args.next().ok_or("--cam-id requires a number")?;
                        cam_id = value
                            .parse::<i32>()
                            .map_err(|_| format!("invalid --cam-id: {value}"))?;
                    }
                    "--unit" => {
                        let value = args.next().ok_or("--unit requires a number")?;
                        unit_no = value
                            .parse::<u8>()
                            .map_err(|_| format!("invalid --unit: {value}"))?;
                    }
                    "--interval-ms" => {
                        let value = args.next().ok_or("--interval-ms requires a number")?;
                        let ms = value
                            .parse::<u64>()
                            .map_err(|_| format!("invalid --interval-ms: {value}"))?;
                        read_interval = Duration::from_millis(ms);
                    }
                    "--poll-ms" => {
                        let value = args.next().ok_or("--poll-ms requires a number")?;
                        let ms = value
                            .parse::<u64>()
                            .map_err(|_| format!("invalid --poll-ms: {value}"))?;
                        poll_interval = Some(Duration::from_millis(ms));
                    }
                    "--show-window" => show_window = true,
                    "--list-cameras" => list_cameras = true,
                    "--help" | "-h" => {
                        print_usage();
                        std::process::exit(0);
                    }
                    _ => return Err(format!("unknown argument: {arg}")),
                }
            }

            Ok(Self {
                dll_path,
                cam_id,
                show_window,
                list_cameras,
                unit_no,
                read_interval,
                poll_interval,
            })
        }
    }

    struct DllConfig {
        path: PathBuf,
    }

    impl DllConfig {
        fn create(options: &Options) -> io::Result<Self> {
            let mut path = env::temp_dir();
            path.push("aimeio_qrcode_dll_tester.ini");

            let content = format!(
                "[aimeio]\r\ncamId={}\r\nlistCameras={}\r\nshowWindow={}\r\n",
                options.cam_id,
                bool_to_int(options.list_cameras),
                bool_to_int(options.show_window)
            );

            fs::write(&path, content)?;
            Ok(Self { path })
        }
    }

    struct AimeDll {
        module: Hmodule,
        get_api_version: GetApiVersionFn,
        init: InitFn,
        poll: PollFn,
        get_aime_id: GetAimeIdFn,
        radio_on: RadioFn,
    }

    impl AimeDll {
        fn load(path: &Path) -> Result<Self, String> {
            let wide = to_wide(path);
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                return Err(format!("failed to load DLL: {}", path.display()));
            }

            let get_api_version =
                unsafe { load_symbol::<GetApiVersionFn>(module, "aime_io_get_api_version")? };
            let init = unsafe { load_symbol::<InitFn>(module, "aime_io_init")? };
            let poll = unsafe { load_symbol::<PollFn>(module, "aime_io_nfc_poll")? };
            let get_aime_id =
                unsafe { load_symbol::<GetAimeIdFn>(module, "aime_io_nfc_get_aime_id")? };
            let radio_on = unsafe { load_symbol::<RadioFn>(module, "aime_io_nfc_radio_on")? };

            Ok(Self {
                module,
                get_api_version,
                init,
                poll,
                get_aime_id,
                radio_on,
            })
        }
    }

    impl Drop for AimeDll {
        fn drop(&mut self) {
            if !self.module.is_null() {
                unsafe {
                    FreeLibrary(self.module);
                }
            }
        }
    }

    pub fn run() -> Result<(), String> {
        let options = Options::parse()?;
        let config = DllConfig::create(&options)
            .map_err(|e| format!("failed to create temporary config: {e}"))?;

        unsafe {
            env::set_var("SEGATOOLS_CONFIG_PATH", &config.path);
        }

        println!("DLL path: {}", options.dll_path.display());
        println!("Config path: {}", config.path.display());
        println!(
            "camId={}, showWindow={}, listCameras={}",
            options.cam_id, options.show_window, options.list_cameras
        );

        let dll = AimeDll::load(&options.dll_path)?;

        let version = unsafe { (dll.get_api_version)() };
        println!("API version: 0x{version:04X}");

        let init_hr = unsafe { (dll.init)() };
        println!("aime_io_init -> {}", describe_hresult(init_hr));
        if init_hr != S_OK {
            return Err("DLL initialization failed".to_string());
        }

        let radio_hr = unsafe { (dll.radio_on)(options.unit_no) };
        println!("aime_io_nfc_radio_on -> {}", describe_hresult(radio_hr));

        if let Some(poll_interval) = options.poll_interval {
            let poll_hr = unsafe { (dll.poll)(options.unit_no) };
            println!("aime_io_nfc_poll -> {}", describe_hresult(poll_hr));
            read_loop(&dll, &options, Some((poll_interval, Instant::now())))
        } else {
            read_loop(&dll, &options, None)
        }
    }

    fn read_loop(
        dll: &AimeDll,
        options: &Options,
        mut poll_state: Option<(Duration, Instant)>,
    ) -> Result<(), String> {
        println!("Reading aime IDs. Press Ctrl+C to stop.");

        let mut last_seen = None;

        loop {
            if let Some((interval, last_poll)) = poll_state.as_mut() {
                if last_poll.elapsed() >= *interval {
                    let hr = unsafe { (dll.poll)(options.unit_no) };
                    println!("aime_io_nfc_poll -> {}", describe_hresult(hr));
                    *last_poll = Instant::now();
                }
            }

            let mut aime_id = [0u8; 10];
            let hr =
                unsafe { (dll.get_aime_id)(options.unit_no, aime_id.as_mut_ptr(), aime_id.len()) };

            match hr {
                S_OK => {
                    if last_seen != Some(aime_id) {
                        println!("Aime ID detected: {}", format_hex(&aime_id));
                        last_seen = Some(aime_id);
                    }
                }
                S_FALSE => {
                    if last_seen.take().is_some() {
                        println!("Aime ID removed");
                    }
                }
                _ => {
                    return Err(format!(
                        "aime_io_nfc_get_aime_id failed: {}",
                        describe_hresult(hr)
                    ));
                }
            }

            thread::sleep(options.read_interval);
        }
    }

    unsafe fn load_symbol<T: Copy>(module: Hmodule, name: &str) -> Result<T, String> {
        let name = CString::new(name).map_err(|_| format!("invalid symbol name: {name}"))?;
        let symbol = unsafe { GetProcAddress(module, name.as_ptr()) };
        if symbol.is_null() {
            return Err(format!("missing symbol: {}", name.to_string_lossy()));
        }

        Ok(unsafe { std::mem::transmute_copy(&symbol) })
    }

    fn default_dll_path() -> Result<PathBuf, String> {
        let exe = env::current_exe().map_err(|e| format!("cannot locate current executable: {e}"))?;
        let dir = exe
            .parent()
            .ok_or("cannot determine executable directory")?;
        Ok(dir.join("aimeio_qrcode.dll"))
    }

    fn to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn bool_to_int(value: bool) -> u8 {
        if value { 1 } else { 0 }
    }

    fn format_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0F));
        }
        out
    }

    fn hex_digit(value: u8) -> char {
        match value {
            0..=9 => (b'0' + value) as char,
            10..=15 => (b'A' + (value - 10)) as char,
            _ => '?',
        }
    }

    fn describe_hresult(hr: Hresult) -> String {
        let meaning = match hr {
            S_OK => "S_OK",
            S_FALSE => "S_FALSE (NotPresent)",
            E_INVALIDARG => "E_INVALIDARG",
            E_HANDLE => "E_HANDLE",
            E_FAIL => "E_FAIL",
            ERROR_TIMEOUT => "ERROR_TIMEOUT",
            _ => "UNKNOWN",
        };

        format!("{meaning} (0x{:08X})", hr as u32)
    }

    fn print_usage() {
        println!("Usage:");
        println!("  cargo run --bin dll_tester -- [options]");
        println!();
        println!("Options:");
        println!("  --dll <path>         Path to the DLL to load");
        println!("  --cam-id <id>        Camera ID written into a temporary segatools.ini");
        println!("  --show-window        Enable the debug camera window");
        println!("  --list-cameras       Ask the DLL to print available cameras during init");
        println!("  --unit <id>          NFC unit number passed to the DLL (default: 0)");
        println!("  --interval-ms <ms>   Delay between aime ID reads (default: 200)");
        println!("  --poll-ms <ms>       Call aime_io_nfc_poll periodically");
    }
}

#[cfg(windows)]
fn main() {
    if let Err(err) = app::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
