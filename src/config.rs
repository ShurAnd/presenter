//! Настройки программы и список сохранённых серверов.

use super::*;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub(crate) struct Config {
    pub(crate) ip: String,
    pub(crate) port: String,
    pub(crate) password: String,
    /// Название сервера для списка (необязательно), например «Зал 1».
    pub(crate) name: String,
    pub(crate) windowed: bool,
    /// Клавиши в окне программы (стрелки, PageUp/PageDown, пробел, цифры + Enter) управляют показом.
    pub(crate) keyboard: bool,
    /// Монитор для показа: системное имя `\\.\DISPLAY2`; пусто — основной.
    pub(crate) monitor: String,
    /// Серверы, к которым подключались, — последний использованный первым.
    #[serde(deserialize_with = "null_as_empty")]
    pub(crate) servers: Vec<SavedServer>,
    /// Резервный сервер («адрес:порт» из списка) — на него можно переключиться одной кнопкой.
    pub(crate) reserve: String,
    /// Загружаемые файлы дублировать на резервный сервер.
    pub(crate) mirror: bool,
    /// Плеер (на сервере): играть по списку, по кругу, громкость 0–100.
    pub(crate) audio_list: bool,
    pub(crate) audio_repeat: bool,
    pub(crate) audio_volume: u8,
    /// Выбранный фон сцены — имя исходного файла (для показа); пусто — стандартный.
    pub(crate) background: String,
}

/// `"servers": null` (файл правили руками или другой программой) читаем как пустой список,
/// иначе из-за одного поля пропали бы все настройки.
pub(crate) fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SavedServer>, D::Error> {
    Ok(Option::<Vec<SavedServer>>::deserialize(d)?.unwrap_or_default())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ip: "127.0.0.1".into(),
            port: "14999".into(),
            password: "default_password".into(),
            name: String::new(),
            windowed: false,
            keyboard: true,
            monitor: String::new(),
            servers: Vec::new(),
            reserve: String::new(),
            mirror: true,
            audio_list: true,
            audio_repeat: false,
            audio_volume: 100,
            background: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub(crate) struct SavedServer {
    pub(crate) name: String,
    pub(crate) ip: String,
    pub(crate) port: String,
    pub(crate) password: String,
}

impl SavedServer {
    /// Сервер узнаём по адресу и порту.
    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.ip.trim(), self.port.trim())
    }

    pub(crate) fn label(&self) -> String {
        if self.name.trim().is_empty() { self.key() } else { format!("{} — {}", self.name.trim(), self.key()) }
    }
}

/// Добавить сервер в список сохранённых, не меняя порядок остальных. Такой адрес уже есть —
/// обновить его на том же месте; список переполнен — уходит самый давний, но не только что добавленный.
pub(crate) fn upsert_server(list: &mut Vec<SavedServer>, srv: SavedServer) {
    let key = srv.key();
    match list.iter_mut().find(|s| s.key() == key) {
        Some(s) => *s = srv,
        None => {
            list.push(srv);
            if list.len() > MAX_SERVERS {
                list.remove(MAX_SERVERS - 1);
            }
        }
    }
}

/// Сколько серверов помнить.
pub(crate) const MAX_SERVERS: usize = 20;

/// Добавить в `ours` серверы из `other`, которых там нет (кроме забытых в этом сеансе).
/// Наш порядок сохраняется: свои — первыми.
pub(crate) fn merge_servers(ours: &mut Vec<SavedServer>, other: Vec<SavedServer>, forgotten: &HashSet<String>) {
    for srv in other {
        let key = srv.key();
        if !forgotten.contains(&key) && !ours.iter().any(|s| s.key() == key) {
            ours.push(srv);
        }
    }
    ours.truncate(MAX_SERVERS);
}
