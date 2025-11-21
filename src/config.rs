use crate::fonts::{FontConfig, Pattern};
use crate::user_cache; // For detecting the active desktop user's home dir
use crate::FunctionLayer;
use anyhow::Error;
use cairo::FontFace;
use freetype::Library as FtLibrary;
use input_linux::Key;
use nix::{
    errno::Errno,
    sys::inotify::{AddWatchFlags, InitFlags, Inotify, InotifyEvent, WatchDescriptor},
};
use serde::{Deserialize, Deserializer};
use serde::de::value;
use std::{fs::read_to_string, os::fd::AsFd, collections::HashMap};

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ButtonColor {
    Grayscale(f64),
    Rgb([f64; 3]),
}

impl ButtonColor {
    pub fn set_cairo_source(&self, c: &cairo::Context) {
        match self {
            ButtonColor::Grayscale(gray) => c.set_source_rgb(*gray, *gray, *gray),
            ButtonColor::Rgb([r, g, b]) => c.set_source_rgb(*r, *g, *b),
        }
    }
}

// System-wide override locations
const ETC_CFG_PATH: &str = "/etc/tiny-dfr/config.toml";
const ETC_COMMANDS_PATH: &str = "/etc/tiny-dfr/commands.toml";
const ETC_ENV_PATH: &str = "/etc/tiny-dfr/user-env.toml";
const ETC_EXPANDABLES_PATH: &str = "/etc/tiny-dfr/expandables.toml";

// Per-user override (highest priority):
// ~/.config/tiny-dfr/{config,commands,expandables,hyprland}.toml
#[derive(Clone, Debug, Default)]
struct UserConfigPaths {
    config: Option<String>,
    commands: Option<String>,
    expandables: Option<String>,
    hyprland: Option<String>,
}

fn detect_user_config_paths() -> UserConfigPaths {
    // Try to use the cached desktop user env if available
    if let Some(env) = user_cache::get_cached_user_environment() {
        let base = format!("{}/.config/tiny-dfr", env.home_dir);
        return UserConfigPaths {
            config: Some(format!("{}/config.toml", base)),
            commands: Some(format!("{}/commands.toml", base)),
            expandables: Some(format!("{}/expandables.toml", base)),
            hyprland: Some(format!("{}/hyprland.toml", base)),
        };
    }

    // No detected desktop user yet; return empty so callers can gracefully skip
    UserConfigPaths::default()
}

#[derive(Debug, Clone, PartialEq)]
pub enum ButtonAction {
    Key(Key),
    Command(String), // Command_1, Command_2, etc.
    Expand(String),  // Expand_Something
    HyprlandExpand(String), // Hyprland_Expand_ActiveWindow
    KeyCombos(Vec<Key>), // KeyCombos_CTRL_SHIFT_I
}

impl<'de> Deserialize<'de> for ButtonAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        // Check for Hyprland expand actions
        if s.starts_with("Hyprland_Expand_") {
            return Ok(ButtonAction::HyprlandExpand(s));
        }

        // Check for key combinations
        if s.starts_with("KeyCombos_") {
            let keys = crate::hyprland::parse_key_combos(&s);
            if !keys.is_empty() {
                return Ok(ButtonAction::KeyCombos(keys));
            }
        }

        // Check if it's an Expand action
        if s.starts_with("Expand_") {
            return Ok(ButtonAction::Expand(s));
        }

        // Try to deserialize as Key using serde
        let key_result: Result<Key, _> = serde::de::Deserialize::deserialize(
            value::StringDeserializer::<serde::de::value::Error>::new(s.clone())
        );

        if let Ok(key) = key_result {
            return Ok(ButtonAction::Key(key));
        }

        // Otherwise treat as Command
        Ok(ButtonAction::Command(s))
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct UserEnvironment {
    pub username: String,
    pub uid: u32,
    pub home_dir: String,
    pub runtime_dir: String,
    pub wayland_display: String,
    pub user_paths: String,
}

#[derive(Deserialize)]
struct UserEnvConfig {
    user_environment: UserEnvironment,
}

pub struct Config {
    pub show_button_outlines: bool,
    pub enable_pixel_shift: bool,
    pub font_face: FontFace,
    pub adaptive_brightness: bool,
    pub active_brightness: u32,
    pub keyboard_brightness_step: u32,
    pub keyboard_brightness_enabled: bool,
    pub commands: HashMap<String, String>,
    pub user_env: Option<UserEnvironment>,
    pub back_button_show_outlines: bool,
    pub back_button_outline_color: Option<ButtonColor>,
    pub expandable_timeout_seconds: u32,
    pub expandables: HashMap<String, Vec<ButtonConfig>>,
    pub hyprland_expandables: HashMap<String, Vec<HyprlandExpandConfig>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct HyprlandExpandConfig {
    pub class: String,
    pub button_title: Option<String>, // "title", "class", "initialTitle", "initialClass"
    pub show_app_icon_alongside_text: Option<bool>,
    pub app_icon: Option<String>,
    pub layer_keys: Vec<ButtonConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ConfigProxy {
    media_layer_default: Option<bool>,
    show_button_outlines: Option<bool>,
    enable_pixel_shift: Option<bool>,
    font_template: Option<String>,
    adaptive_brightness: Option<bool>,
    active_brightness: Option<u32>,
    primary_layer_keys: Option<Vec<ButtonConfig>>,
    media_layer_keys: Option<Vec<ButtonConfig>>,
    keyboard_brightness_step: Option<u32>,
    keyboard_brightness_enabled: Option<bool>,
    back_button_show_outlines: Option<bool>,
    back_button_outline_color: Option<ButtonColor>,
    expandable_timeout_seconds: Option<u32>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ButtonConfig {
    #[serde(alias = "Svg")]
    pub icon: Option<String>,
    pub text: Option<String>,
    pub theme: Option<String>,
    pub time: Option<String>,
    pub battery: Option<String>,
    pub locale: Option<String>,
    pub action: ButtonAction,
    pub stretch: Option<usize>,
    pub show_button_outlines: Option<bool>,
    pub button_outlines_color: Option<ButtonColor>,
    pub show_app_icon_alongside_text: Option<bool>,
    pub app_icon: Option<String>,
}

fn load_commands() -> HashMap<String, String> {
    let mut commands = HashMap::new();

    // Load base commands from /usr/share/tiny-dfr/commands.toml
    if let Ok(content) = read_to_string("/usr/share/tiny-dfr/commands.toml") {
        if let Ok(base_commands) = toml::from_str::<HashMap<String, String>>(&content) {
            commands.extend(base_commands);
        }
    }

    // Override with system-wide commands from /etc/tiny-dfr/commands.toml
    if let Ok(content) = read_to_string(ETC_COMMANDS_PATH) {
        if let Ok(user_commands) = toml::from_str::<HashMap<String, String>>(&content) {
            commands.extend(user_commands);
        }
    }

    // Highest priority: per-user commands from ~/.config/tiny-dfr/commands.toml
    let user_paths = detect_user_config_paths();
    if let Some(p) = user_paths.commands {
        if let Ok(content) = read_to_string(p) {
            if let Ok(user_commands) = toml::from_str::<HashMap<String, String>>(&content) {
                commands.extend(user_commands);
            }
        }
    }

    commands
}

fn load_user_environment() -> Option<UserEnvironment> {
    if let Ok(content) = read_to_string(ETC_ENV_PATH) {
        if let Ok(env_config) = toml::from_str::<UserEnvConfig>(&content) {
            return Some(env_config.user_environment);
        }
    }
    None
}

fn load_expandables() -> HashMap<String, Vec<ButtonConfig>> {
    let mut expandables = HashMap::new();

    // Load base expandables from /usr/share/tiny-dfr/expandables.toml
    if let Ok(content) = read_to_string("/usr/share/tiny-dfr/expandables.toml") {
        if let Ok(base_expandables) = toml::from_str::<HashMap<String, Vec<ButtonConfig>>>(&content) {
            expandables.extend(base_expandables);
        }
    }

    // Override with system-wide expandables from /etc/tiny-dfr/expandables.toml
    if let Ok(content) = read_to_string(ETC_EXPANDABLES_PATH) {
        if let Ok(user_expandables) = toml::from_str::<HashMap<String, Vec<ButtonConfig>>>(&content) {
            expandables.extend(user_expandables);
        }
    }

    // Highest priority: per-user expandables from ~/.config/tiny-dfr/expandables.toml
    let user_paths = detect_user_config_paths();
    if let Some(p) = user_paths.expandables {
        if let Ok(content) = read_to_string(p) {
            if let Ok(user_expandables) = toml::from_str::<HashMap<String, Vec<ButtonConfig>>>(&content) {
                expandables.extend(user_expandables);
            }
        }
    }

    expandables
}

fn load_hyprland_expandables() -> HashMap<String, Vec<HyprlandExpandConfig>> {
    let mut hyprland_expandables = HashMap::new();

    // Load base hyprland expandables from /usr/share/tiny-dfr/hyprland.toml
    if let Ok(content) = read_to_string("/usr/share/tiny-dfr/hyprland.toml") {
        if let Ok(base_hyprland_expandables) = toml::from_str::<HashMap<String, Vec<HyprlandExpandConfig>>>(&content) {
            hyprland_expandables.extend(base_hyprland_expandables);
        }
    }

    // Override with system-wide hyprland expandables from /etc/tiny-dfr/hyprland.toml
    if let Ok(content) = read_to_string("/etc/tiny-dfr/hyprland.toml") {
        if let Ok(user_hyprland_expandables) = toml::from_str::<HashMap<String, Vec<HyprlandExpandConfig>>>(&content) {
            hyprland_expandables.extend(user_hyprland_expandables);
        }
    }

    // Highest priority: per-user hyprland expandables from ~/.config/tiny-dfr/hyprland.toml
    let user_paths = detect_user_config_paths();
    if let Some(p) = user_paths.hyprland {
        if let Ok(content) = read_to_string(p) {
            if let Ok(user_hyprland_expandables) = toml::from_str::<HashMap<String, Vec<HyprlandExpandConfig>>>(&content) {
                hyprland_expandables.extend(user_hyprland_expandables);
            }
        }
    }

    hyprland_expandables
}

fn load_font(name: &str) -> FontFace {
    let fontconfig = FontConfig::new();
    let mut pattern = Pattern::new(name);
    fontconfig.perform_substitutions(&mut pattern);
    let pat_match = match fontconfig.match_pattern(&pattern) {
        Ok(pat) => pat,
        Err(_) => panic!("Unable to find specified font. If you are using the default config, make sure you have at least one font installed")
    };
    let file_name = pat_match.get_file_name();
    let file_idx = pat_match.get_font_index();
    let ft_library = FtLibrary::init().unwrap();
    let face = ft_library.new_face(file_name, file_idx).unwrap();
    FontFace::create_from_ft(&face).unwrap()
}

fn load_config(width: u16) -> (Config, [FunctionLayer; 2]) {
    // Ensure the user environment cache is initialized so we can resolve per-user config paths
    user_cache::initialize_user_environment_cache();

    let mut base =
        toml::from_str::<ConfigProxy>(&read_to_string("/usr/share/tiny-dfr/config.toml").unwrap())
            .unwrap();
    // Merge system-wide overrides from /etc
    let user = read_to_string(ETC_CFG_PATH)
        .map_err::<Error, _>(|e| e.into())
        .and_then(|r| Ok(toml::from_str::<ConfigProxy>(&r)?));
    if let Ok(user) = user {
        base.media_layer_default = user.media_layer_default.or(base.media_layer_default);
        base.show_button_outlines = user.show_button_outlines.or(base.show_button_outlines);
        base.enable_pixel_shift = user.enable_pixel_shift.or(base.enable_pixel_shift);
        base.font_template = user.font_template.or(base.font_template);
        base.adaptive_brightness = user.adaptive_brightness.or(base.adaptive_brightness);
        base.media_layer_keys = user.media_layer_keys.or(base.media_layer_keys);
        base.primary_layer_keys = user.primary_layer_keys.or(base.primary_layer_keys);
        base.active_brightness = user.active_brightness.or(base.active_brightness);
        base.keyboard_brightness_step = user.keyboard_brightness_step.or(base.keyboard_brightness_step);
        base.keyboard_brightness_enabled = user.keyboard_brightness_enabled.or(base.keyboard_brightness_enabled);
        base.back_button_show_outlines = user.back_button_show_outlines.or(base.back_button_show_outlines);
        base.back_button_outline_color = user.back_button_outline_color.or(base.back_button_outline_color);
        base.expandable_timeout_seconds = user.expandable_timeout_seconds.or(base.expandable_timeout_seconds);
    };

    // Merge per-user overrides from ~/.config/tiny-dfr/config.toml (highest priority)
    let user_paths = detect_user_config_paths();
    if let Some(user_cfg_path) = user_paths.config {
        let user_cfg = read_to_string(user_cfg_path)
            .map_err::<Error, _>(|e| e.into())
            .and_then(|r| Ok(toml::from_str::<ConfigProxy>(&r)?));
        if let Ok(user) = user_cfg {
            base.media_layer_default = user.media_layer_default.or(base.media_layer_default);
            base.show_button_outlines = user.show_button_outlines.or(base.show_button_outlines);
            base.enable_pixel_shift = user.enable_pixel_shift.or(base.enable_pixel_shift);
            base.font_template = user.font_template.or(base.font_template);
            base.adaptive_brightness = user.adaptive_brightness.or(base.adaptive_brightness);
            base.media_layer_keys = user.media_layer_keys.or(base.media_layer_keys);
            base.primary_layer_keys = user.primary_layer_keys.or(base.primary_layer_keys);
            base.active_brightness = user.active_brightness.or(base.active_brightness);
            base.keyboard_brightness_step = user.keyboard_brightness_step.or(base.keyboard_brightness_step);
            base.keyboard_brightness_enabled = user.keyboard_brightness_enabled.or(base.keyboard_brightness_enabled);
            base.back_button_show_outlines = user.back_button_show_outlines.or(base.back_button_show_outlines);
            base.back_button_outline_color = user.back_button_outline_color.or(base.back_button_outline_color);
            base.expandable_timeout_seconds = user.expandable_timeout_seconds.or(base.expandable_timeout_seconds);
        }
    }
    let mut media_layer_keys = base.media_layer_keys.unwrap();
    let mut primary_layer_keys = base.primary_layer_keys.unwrap();
    if width >= 2170 {
        for layer in [&mut media_layer_keys, &mut primary_layer_keys] {
            layer.insert(
                0,
                ButtonConfig {
                    icon: None,
                    text: Some("esc".into()),
                    theme: None,
                    action: ButtonAction::Key(Key::Esc),
                    stretch: None,
                    time: None,
                    locale: None,
                    battery: None,
                    show_button_outlines: None,
                    button_outlines_color: None,
                    show_app_icon_alongside_text: None,
                    app_icon: None,
                },
            );
        }
    }
    let media_layer = FunctionLayer::with_config(media_layer_keys);
    let fkey_layer = FunctionLayer::with_config(primary_layer_keys);
    let layers = if base.media_layer_default.unwrap() {
        [media_layer, fkey_layer]
    } else {
        [fkey_layer, media_layer]
    };
    let cfg = Config {
        show_button_outlines: base.show_button_outlines.unwrap(),
        enable_pixel_shift: base.enable_pixel_shift.unwrap(),
        adaptive_brightness: base.adaptive_brightness.unwrap(),
        font_face: load_font(&base.font_template.unwrap()),
        active_brightness: base.active_brightness.unwrap(),
        keyboard_brightness_step: base.keyboard_brightness_step.unwrap_or(32),
        keyboard_brightness_enabled: base.keyboard_brightness_enabled.unwrap_or(true),
        commands: load_commands(),
        user_env: load_user_environment(),
        back_button_show_outlines: base.back_button_show_outlines.unwrap_or(false),
        back_button_outline_color: base.back_button_outline_color,
        expandable_timeout_seconds: base.expandable_timeout_seconds.unwrap_or(5),
        expandables: load_expandables(),
        hyprland_expandables: load_hyprland_expandables(),
    };
    (cfg, layers)
}

pub struct ConfigManager {
    inotify_fd: Inotify,
    watch_desc_etc: Option<WatchDescriptor>,
    watch_desc_user: Option<WatchDescriptor>,
}

fn arm_inotify(inotify_fd: &Inotify, path: &str) -> Option<WatchDescriptor> {
    let flags = AddWatchFlags::IN_MOVED_TO | AddWatchFlags::IN_CLOSE | AddWatchFlags::IN_ONESHOT;
    match inotify_fd.add_watch(path, flags) {
        Ok(wd) => Some(wd),
        Err(Errno::ENOENT) => None,
        e => Some(e.unwrap()),
    }
}

impl ConfigManager {
    pub fn new() -> ConfigManager {
        let inotify_fd = Inotify::init(InitFlags::IN_NONBLOCK).unwrap();
        let watch_desc_etc = arm_inotify(&inotify_fd, ETC_CFG_PATH);
        // Try to set up a watch for the per-user config if it exists now
        let user_paths = detect_user_config_paths();
        let watch_desc_user = user_paths
            .config
            .as_deref()
            .and_then(|p| arm_inotify(&inotify_fd, p));
        ConfigManager {
            inotify_fd,
            watch_desc_etc,
            watch_desc_user,
        }
    }
    pub fn load_config(&self, width: u16) -> (Config, [FunctionLayer; 2]) {
        load_config(width)
    }
    pub fn update_config(
        &mut self,
        cfg: &mut Config,
        layers: &mut [FunctionLayer; 2],
        width: u16,
    ) -> bool {
        if self.watch_desc_etc.is_none() {
            self.watch_desc_etc = arm_inotify(&self.inotify_fd, ETC_CFG_PATH);
        }
        if self.watch_desc_user.is_none() {
            if let Some(user_cfg) = detect_user_config_paths().config {
                self.watch_desc_user = arm_inotify(&self.inotify_fd, &user_cfg);
            }
            return false;
        }
        match self.inotify_fd.read_events() {
            Err(Errno::EAGAIN) => false,
            r => self.handle_events(cfg, layers, width, r),
        }
    }
    #[cold]
    fn handle_events(&mut self, cfg: &mut Config, layers: &mut [FunctionLayer; 2], width: u16, evts: Result<Vec<InotifyEvent>, Errno>) -> bool {
        let mut ret = false;
        for evt in evts.unwrap() {
            // React to either /etc or per-user config changes
            if Some(evt.wd) == self.watch_desc_etc || Some(evt.wd) == self.watch_desc_user {
                let parts = load_config(width);
                *cfg = parts.0;
                *layers = parts.1;
                ret = true;
                // Re-arm both watches
                self.watch_desc_etc = arm_inotify(&self.inotify_fd, ETC_CFG_PATH);
                if let Some(user_cfg) = detect_user_config_paths().config {
                    self.watch_desc_user = arm_inotify(&self.inotify_fd, &user_cfg);
                }
            }
        }
        ret
    }
    pub fn fd(&self) -> &impl AsFd {
        &self.inotify_fd
    }
}
