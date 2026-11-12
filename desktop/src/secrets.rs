//! secrets.rs — ключ шифрования узла (KEK) для настольного клиента.
//!
//! Тот же механизм, что на Android: узел получает KEK через окружение (`P2P_IDENTITY_KEK`) и хранит ключ узла
//! только зашифрованным (`identity.key.enc`), а историю — построчно зашифрованной (`history.keys`). KEK лежит:
//!  • в хранилище секретов системы — Secret Service (KWallet, GNOME Keyring) на Linux, Keychain на macOS,
//!    диспетчер учётных данных на Windows;
//!  • если хранилища нет — в отдельном файле с правами 0600 рядом с настройками (отдельно от каталога узла:
//!    копия каталога узла без него не расшифровывается; от кражи всего диска это не защищает — клиент это пишет).
//! Где лежит ключ, запоминается в настройках: если хранилище было, а сейчас недоступно (кошелёк закрыт),
//! клиент сообщает об ошибке, а не создаёт новый ключ — иначе ключ узла и история стали бы нечитаемыми.

use std::path::PathBuf;

const SERVICE: &str = "p2p-overlay-desktop";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source { Keyring, File }

impl Source {
    pub fn id(self) -> &'static str { match self { Source::Keyring => "keyring", Source::File => "file" } }
    pub fn describe(self) -> &'static str {
        match self {
            Source::Keyring => "в хранилище секретов системы (KWallet / GNOME Keyring / Keychain)",
            Source::File => "в файле рядом с настройками — хранилища секретов нет; от кражи диска это не защищает",
        }
    }
}

fn account() -> String { format!("node-kek-{}", crate::settings::profile()) }

fn file_path() -> PathBuf { crate::settings::config_dir().join(format!("{}.kek", account())) }

fn new_kek() -> Result<String, String> {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).map_err(|e| format!("генератор случайных чисел: {e}"))?;
    Ok(k.iter().map(|b| format!("{b:02x}")).collect())
}

fn keyring_entry() -> Option<keyring::Entry> {
    // P2P_NO_KEYRING — не трогать хранилище секретов (тесты, снимки экрана без окна кошелька)
    if std::env::var_os("P2P_NO_KEYRING").is_some() { return None; }
    keyring::Entry::new(SERVICE, &account()).ok()
}

fn from_file(create: bool) -> Result<Option<String>, String> {
    let p = file_path();
    if let Ok(s) = std::fs::read_to_string(&p) { return Ok(Some(s.trim().to_owned())); }
    if !create { return Ok(None); }
    let k = new_kek()?;
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let mut o = std::fs::OpenOptions::new();
    o.create_new(true).write(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    use std::io::Write;
    o.open(&p).and_then(|mut f| f.write_all(k.as_bytes())).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(k))
}

/// KEK для своего узла. `remembered` — где ключ лежал в прошлый раз ("" — ещё не создавался).
pub fn kek(remembered: &str) -> Result<(String, Source), String> {
    match remembered {
        "file" => return from_file(true).map(|k| (k.unwrap_or_default(), Source::File)),
        "keyring" => {
            let e = keyring_entry().ok_or("хранилище секретов недоступно")?;
            return e.get_password().map(|k| (k, Source::Keyring)).map_err(|err| format!(
                "ключ шифрования узла был в хранилище секретов, но сейчас не получен ({err}). Откройте кошелёк \
                 (KWallet / GNOME Keyring) и перезапустите узел; без ключа узел и история не расшифровываются"));
        }
        _ => {}
    }
    // первый запуск: хранилище секретов, если работает, иначе файл
    if let Some(e) = keyring_entry() {
        match e.get_password() {
            Ok(k) => return Ok((k, Source::Keyring)),
            Err(keyring::Error::NoEntry) => {
                let k = new_kek()?;
                if e.set_password(&k).is_ok() && e.get_password().ok().as_deref() == Some(k.as_str()) {
                    return Ok((k, Source::Keyring));
                }
            }
            Err(_) => {}
        }
    }
    from_file(true).map(|k| (k.unwrap_or_default(), Source::File))
}
