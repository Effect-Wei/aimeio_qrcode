use crate::camera::{CAM_HEIGHT, CAM_WIDTH};
use crate::display::{WindowConfig, parse_display_selection};
use crate::ini_parser::IniParser;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

pub const CAMERA_RETRY_DELAY_SECS: u64 = 2;
pub const CAMERA_READ_ERROR_LIMIT: u32 = 5;
pub const CARD_ABSENT_FRAME_LIMIT: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub cam_id: i32,
    pub list_cameras: bool,
    pub configured_show_window: bool,
    pub window: WindowConfig,
}

impl ConfigSnapshot {
    fn defaults() -> Self {
        Self {
            cam_id: 0,
            list_cameras: false,
            configured_show_window: false,
            window: WindowConfig::new(20, 20, CAM_WIDTH as usize, CAM_HEIGHT as usize, None),
        }
    }
}

pub struct Config {
    parser: IniParser,
    snapshot: ConfigSnapshot,
}

impl Config {
    fn load() -> std::io::Result<Self> {
        let parser = load_ini_parser()?;
        let snapshot = snapshot_from_ini(&parser);
        Ok(Self { parser, snapshot })
    }

    fn reload(&mut self) -> std::io::Result<()> {
        self.parser.reload()?;
        self.snapshot = snapshot_from_ini(&self.parser);
        Ok(())
    }
}

static CONFIG: LazyLock<RwLock<Config>> = LazyLock::new(|| {
    RwLock::new(Config::load().unwrap_or_else(|error| {
        eprintln!(
            "AimeIO DLL: Failed to load config, falling back to defaults: {}",
            error
        );
        Config {
            parser: IniParser::from_map(default_config_path(), Default::default()),
            snapshot: ConfigSnapshot::defaults(),
        }
    }))
});

pub fn current_cam_id() -> i32 {
    current_config().cam_id
}

pub fn list_cameras_enabled() -> bool {
    current_config().list_cameras
}

pub fn current_config() -> ConfigSnapshot {
    CONFIG
        .read()
        .map(|config| config.snapshot.clone())
        .unwrap_or_else(|_| ConfigSnapshot::defaults())
}

pub fn reload_config() {
    if let Ok(mut config) = CONFIG.write() {
        if let Err(error) = config.reload() {
            eprintln!("AimeIO DLL: Failed to reload config: {}", error);
        }
    }
}

fn snapshot_from_ini(ini: &IniParser) -> ConfigSnapshot {
    let monitor_ids_raw = ini.get_string("aimeio", "monitorIds", "");
    let parsed_monitor_ids = parse_display_selection(&monitor_ids_raw);
    if !monitor_ids_raw.trim().is_empty() && parsed_monitor_ids.is_none() {
        eprintln!(
            "AimeIO DLL: Invalid monitorIds '{}', falling back to all displays.",
            monitor_ids_raw
        );
    }

    ConfigSnapshot {
        cam_id: ini.get_int("aimeio", "camId", 0),
        list_cameras: ini.get_int("aimeio", "listCameras", 0) == 1,
        configured_show_window: ini.get_int("aimeio", "showWindow", 0) == 1,
        window: WindowConfig::from_parts(
            ini.get_int("aimeio", "windowX", 20) as isize,
            ini.get_int("aimeio", "windowY", 20) as isize,
            ini.get_int("aimeio", "windowWidth", CAM_WIDTH as i32)
                .max(1) as usize,
            ini.get_int("aimeio", "windowHeight", CAM_HEIGHT as i32)
                .max(1) as usize,
            parsed_monitor_ids,
        ),
    }
}

fn load_ini_parser() -> std::io::Result<IniParser> {
    IniParser::new(&default_config_path())
}

fn default_config_path() -> PathBuf {
    let path_os =
        env::var_os("SEGATOOLS_CONFIG_PATH").unwrap_or_else(|| OsString::from(".\\segatools.ini"));
    PathBuf::from(path_os)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn snapshot_uses_defaults_for_invalid_monitor_ids() {
        let mut section = HashMap::new();
        section.insert("monitorIds".to_string(), "0,abc".to_string());

        let mut data = HashMap::new();
        data.insert("aimeio".to_string(), section);

        let ini = IniParser::from_map(PathBuf::from("test.ini"), data);
        let snapshot = snapshot_from_ini(&ini);

        assert_eq!(snapshot.window.monitor_ids, None);
    }

    #[test]
    fn snapshot_reads_expected_fields() {
        let mut section = HashMap::new();
        section.insert("camId".to_string(), "3".to_string());
        section.insert("listCameras".to_string(), "1".to_string());
        section.insert("showWindow".to_string(), "1".to_string());
        section.insert("windowX".to_string(), "42".to_string());
        section.insert("windowY".to_string(), "24".to_string());
        section.insert("windowWidth".to_string(), "800".to_string());
        section.insert("windowHeight".to_string(), "600".to_string());
        section.insert("monitorIds".to_string(), "1,2".to_string());

        let mut data = HashMap::new();
        data.insert("aimeio".to_string(), section);

        let ini = IniParser::from_map(PathBuf::from("test.ini"), data);
        let snapshot = snapshot_from_ini(&ini);

        assert_eq!(snapshot.cam_id, 3);
        assert!(snapshot.list_cameras);
        assert!(snapshot.configured_show_window);
        assert_eq!(snapshot.window.x, 42);
        assert_eq!(snapshot.window.y, 24);
        assert_eq!(snapshot.window.width, 800);
        assert_eq!(snapshot.window.height, 600);
        assert_eq!(snapshot.window.monitor_ids, Some(vec![1, 2]));
    }
}
