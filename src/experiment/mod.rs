//! experiment/ — сценарии эксперимента 01–09 и проверка pcap (ТЗ §15.1) на реальных процессах
//! `p2p-node`, результаты с `meta.json` (§15.3) и графики из сохранённых CSV (SVG).
//!
//! Запуск: `p2p-orchestrator scenario <01..09|pcap|all>`, `p2p-orchestrator plot <каталог>`.
//! Запасной вариант на Python (тот же метод): `scripts/python/scenarios.py`, `scripts/python/analyze.py`.

pub mod net;
pub mod plot;
pub mod report;
pub mod scenarios;

/// Параметры прогона (одинаковы для всех сценариев).
pub struct Args {
    /// Число узлов N.
    pub nodes: usize,
    /// Наблюдений: lookup (02, 05 — на уровень), операций (03), пар сообщений (06).
    pub n:     usize,
    /// Независимых запусков холодного старта (01, 09 — на схему).
    pub runs:  usize,
    /// Инъекций отказа (04 — отказ одного хранителя, 07).
    pub inj:   usize,
    pub seed:  u64,
    /// Канал между узлами: None — из конфига; `tls` | `ake`.
    pub channel: Option<String>,
    /// Конфиг узлов (по умолчанию config/node_config.yaml; продвинутый уровень — config/node_config_advanced.yaml).
    pub config:  Option<String>,
}

/// Вариант прогона: конфиг, канал и суффикс каталога результатов (`_ake`, `_advanced`).
#[derive(Clone, Default)]
pub struct Variant { pub config: String, pub channel: Option<String>, pub suffix: String }

static VARIANT: std::sync::OnceLock<Variant> = std::sync::OnceLock::new();

pub fn set_variant(a: &Args) {
    let config = a.config.clone().unwrap_or_else(|| report::CONFIG.to_owned());
    let mut suffix = String::new();
    if config != report::CONFIG {
        let stem = std::path::Path::new(&config).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        suffix += &format!("_{}", stem.trim_start_matches("node_config_").trim_start_matches("node_config"));
    }
    if let Some(c) = &a.channel { if c != "tls" { suffix += &format!("_{c}"); } }
    if !cfg!(debug_assertions) { suffix += "_release"; }
    let _ = VARIANT.set(Variant { config, channel: a.channel.clone(), suffix: suffix.replace("__", "_") });
}

pub fn variant() -> Variant { VARIANT.get().cloned().unwrap_or_else(|| Variant { config: report::CONFIG.to_owned(), ..Default::default() }) }
