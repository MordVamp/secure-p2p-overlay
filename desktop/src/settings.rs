//! settings.rs — настройки приложения и адресная книга (JSON в каталоге конфигурации).

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Mode {
    /// Приложение само запускает `p2p-node` и управляет им.
    #[default]
    Launch,
    /// Подключиться к уже запущенному узлу по порту его панели.
    Attach,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Contact {
    pub name:    String,
    pub node_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode:        Mode,
    /// Путь к `p2p-node`; пусто — искать рядом с приложением и в target/.
    pub node_bin:    String,
    pub listen_port: u16,
    pub gui_port:    u16,
    /// Bootstrap-пиры через запятую, напр. `127.0.0.1:7001`. Пусто — узел станет seed.
    pub bootstrap:   String,
    pub state_dir:   String,
    pub attach_port: u16,
    pub autostart:   bool,
    /// Псевдоним, зарегистрированный этим узлом из приложения (для отображения).
    pub my_alias:    String,
    pub contacts:    Vec<Contact>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Launch,
            node_bin: String::new(),
            listen_port: 7600,
            gui_port: 7690,
            bootstrap: String::new(),
            state_dir: data_dir().join(if profile() == "default" { "node".to_owned() } else { format!("{}/node", profile()) })
                .to_string_lossy().into_owned(),
            attach_port: 7690,
            autostart: false,
            my_alias: String::new(),
            contacts: vec![],
        }
    }
}

static PROFILE: OnceLock<String> = OnceLock::new();

/// Профиль (`--profile NAME`): свои настройки, ключ узла и история — несколько
/// пользователей на одной машине. По умолчанию — `default`.
pub fn set_profile(name: &str) { let _ = PROFILE.set(name.to_owned()); }

pub fn profile() -> &'static str { PROFILE.get().map(String::as_str).unwrap_or("default") }

fn home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
        .join("p2p-overlay-desktop")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"))
        .join("p2p-overlay-desktop")
}

impl Settings {
    fn path() -> PathBuf {
        match profile() {
            "default" => config_dir().join("settings.json"),
            p => config_dir().join(format!("settings-{p}.json")),
        }
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&p, s).map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn contact_name(&self, node_id: &str) -> Option<&str> {
        self.contacts.iter().find(|c| c.node_id == node_id).map(|c| c.name.as_str())
    }
}
