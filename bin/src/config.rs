use crate::error::RayhunterError;

use serde::Deserialize;

#[derive(Deserialize)]
struct ConfigFile {
    qmdl_store_path: Option<String>,
    port: Option<u16>,
    debug_mode: Option<bool>,
    ui_level: Option<u8>,
    enable_dummy_analyzer: Option<bool>,
    colorblind_mode: Option<bool>,
    full_background_color: Option<bool>,
    show_screen_overlay: Option<bool>,
    enable_animation: Option<bool>,
}

#[derive(Debug)]
pub struct Config {
    pub qmdl_store_path: String,
    pub port: u16,
    pub debug_mode: bool,
    pub ui_level: u8,
    pub enable_dummy_analyzer: bool,
    pub colorblind_mode: bool,
    pub full_background_color: bool,
    pub show_screen_overlay: bool,
    pub enable_animation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            qmdl_store_path: "/data/rayhunter/qmdl".to_string(),
            port: 8080,
            debug_mode: false,
            ui_level: 1,
            enable_dummy_analyzer: false,
            colorblind_mode: false,
            full_background_color: false,
            show_screen_overlay: true,
            enable_animation: true,
        }
    }
}

pub fn parse_config<P>(path: P) -> Result<Config, RayhunterError> where P: AsRef<std::path::Path> {
    let mut config = Config::default();
    if let Ok(config_file) = std::fs::read_to_string(&path) {
        let parsed_config: ConfigFile = toml::from_str(&config_file)
            .map_err(RayhunterError::ConfigFileParsingError)?;
        parsed_config.qmdl_store_path.map(|v| config.qmdl_store_path = v);
        parsed_config.port.map(|v| config.port = v);
        parsed_config.debug_mode.map(|v| config.debug_mode = v);
        parsed_config.ui_level.map(|v| config.ui_level = v);
        parsed_config.enable_dummy_analyzer.map(|v| config.enable_dummy_analyzer = v);
        parsed_config.colorblind_mode.map(|v| config.colorblind_mode = v);
        parsed_config.full_background_color.map(|v| config.full_background_color = v);
        parsed_config.show_screen_overlay.map(|v| config.show_screen_overlay = v);
        parsed_config.enable_animation.map(|v| config.enable_animation = v);
    }
    Ok(config)
}

pub struct Args {
    pub config_path: String,
}

pub fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("Usage: {} /path/to/config/file", args[0]);
        std::process::exit(1);
    }
    Args {
        config_path: args[1].clone(),
    }
}
