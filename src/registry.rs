//! Строковые значения в реестре текущего пользователя (HKCU) — напрямую через Windows API.
//! Раньше автозапуск писался запуском `reg.exe`, а настройка монитора PowerPoint — из PowerShell:
//! чужие процессы, пишущие в реестр, антивирусы считают подозрительными.

/// Прочитать строку `name` из раздела `sub` (путь без `HKCU\`). Нет раздела или значения — `None`.
pub fn get(sub: &str, name: &str) -> Option<String> {
    imp::get(sub, name)
}

/// Записать строку (раздел создаётся, если его нет).
pub fn set(sub: &str, name: &str, value: &str) -> Result<(), String> {
    imp::set(sub, name, value)
}

/// Удалить значение; если его и так нет — не ошибка.
pub fn delete(sub: &str, name: &str) -> Result<(), String> {
    imp::delete(sub, name)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(key: isize, sub: *const u16, opts: u32, sam: u32, out: *mut isize) -> i32;
        fn RegCreateKeyExW(key: isize, sub: *const u16, reserved: u32, class: *const u16, opts: u32, sam: u32, sec: *const c_void, out: *mut isize, disp: *mut u32) -> i32;
        fn RegQueryValueExW(key: isize, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, len: *mut u32) -> i32;
        fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, len: u32) -> i32;
        fn RegDeleteValueW(key: isize, name: *const u16) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }

    /// HKEY_CURRENT_USER: (HKEY)(LONG)0x80000001 — с расширением знака на 64 битах.
    const HKCU: isize = 0x8000_0001u32 as i32 as isize;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn err(code: i32) -> String {
        std::io::Error::from_raw_os_error(code).to_string()
    }

    /// Открытый раздел, закрывается сам.
    struct Key(isize);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(sub: &str, sam: u32) -> Result<Key, i32> {
        let mut k = 0isize;
        match unsafe { RegOpenKeyExW(HKCU, wide(sub).as_ptr(), 0, sam, &mut k) } {
            0 => Ok(Key(k)),
            e => Err(e),
        }
    }

    pub fn get(sub: &str, name: &str) -> Option<String> {
        let key = open(sub, KEY_QUERY_VALUE).ok()?;
        let name = wide(name);
        let (mut kind, mut len) = (0u32, 0u32);
        unsafe {
            if RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut len) != 0 || !matches!(kind, REG_SZ | REG_EXPAND_SZ) {
                return None;
            }
            let mut buf = vec![0u16; len as usize / 2 + 1];
            let mut bytes = (buf.len() * 2) as u32;
            if RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, buf.as_mut_ptr() as *mut u8, &mut bytes) != 0 {
                return None;
            }
            buf.truncate(bytes as usize / 2);
            while buf.last() == Some(&0) {
                buf.pop();
            }
            Some(String::from_utf16_lossy(&buf))
        }
    }

    pub fn set(sub: &str, name: &str, value: &str) -> Result<(), String> {
        let mut k = 0isize;
        let r = unsafe { RegCreateKeyExW(HKCU, wide(sub).as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE, std::ptr::null(), &mut k, std::ptr::null_mut()) };
        if r != 0 {
            return Err(err(r));
        }
        let key = Key(k);
        let data = wide(value);
        match unsafe { RegSetValueExW(key.0, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) } {
            0 => Ok(()),
            e => Err(err(e)),
        }
    }

    pub fn delete(sub: &str, name: &str) -> Result<(), String> {
        let key = match open(sub, KEY_SET_VALUE) {
            Ok(k) => k,
            Err(ERROR_FILE_NOT_FOUND) => return Ok(()),
            Err(e) => return Err(err(e)),
        };
        match unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) } {
            0 | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(err(e)),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn get(_: &str, _: &str) -> Option<String> {
        None
    }
    pub fn set(_: &str, _: &str, _: &str) -> Result<(), String> {
        Err("нет реестра".into())
    }
    pub fn delete(_: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// Запись, чтение и удаление временного значения в HKCU\Software (после теста следов не остаётся).
    #[test]
    #[cfg(windows)]
    fn roundtrip() {
        let (sub, name) = ("Software", "Presentator тест");
        super::set(sub, name, "\"C:\\путь с пробелом\\a.exe\" --x").unwrap();
        assert_eq!(super::get(sub, name).as_deref(), Some("\"C:\\путь с пробелом\\a.exe\" --x"));
        super::delete(sub, name).unwrap();
        assert_eq!(super::get(sub, name), None);
        super::delete(sub, name).unwrap(); // повторно — не ошибка
    }
}
