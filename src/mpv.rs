//! libmpv — основной движок встроенного видеопроигрывателя (mpv со встроенным FFmpeg:
//! играет практически любые форматы). DLL вшита в exe в сжатом виде (`vendor/libmpv`),
//! при первом использовании распаковывается в `%LOCALAPPDATA%\Presentator\` и подгружается
//! динамически. Если это не удалось — видео играет запасной движок Windows (MFPlay).

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::core::{s, HSTRING, PCSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const MPV_TAG: &str = "20260924";
const MPV_DLL: &str = "libmpv-2.dll";
const MPV_DLL_SIZE: u64 = 120_797_184;
static MPV_ZIP: &[u8] = include_bytes!("../vendor/libmpv/libmpv-2.zip");

pub const FORMAT_FLAG: i32 = 3;
pub const FORMAT_DOUBLE: i32 = 5;
pub const EVENT_NONE: i32 = 0;
pub const EVENT_SHUTDOWN: i32 = 1;
pub const EVENT_END_FILE: i32 = 7;
pub const EVENT_PLAYBACK_RESTART: i32 = 21;
pub const END_FILE_REASON_ERROR: i32 = 4;

#[repr(C)]
pub struct Event {
    pub event_id: i32,
    pub error: i32,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct EventEndFile {
    pub reason: i32,
    pub error: i32,
}

type Handle = *mut c_void;

struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> i32,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> i32,
    get_property: unsafe extern "C" fn(Handle, *const c_char, i32, *mut c_void) -> i32,
    command: unsafe extern "C" fn(Handle, *const *const c_char) -> i32,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut Event,
    terminate_destroy: unsafe extern "C" fn(Handle),
    error_string: unsafe extern "C" fn(i32) -> *const c_char,
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// Распаковывает DLL (если её ещё нет) и возвращает путь к ней.
fn extract() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Presentator").join(format!("libmpv-{MPV_TAG}"));
    let dll = dir.join(MPV_DLL);
    if std::fs::metadata(&dll).is_ok_and(|m| m.len() == MPV_DLL_SIZE) {
        return Ok(dll);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(MPV_ZIP)).map_err(|e| format!("архив libmpv повреждён: {e}"))?;
    let mut entry = zip.by_name(MPV_DLL).map_err(|e| format!("в архиве нет {MPV_DLL}: {e}"))?;
    // Пишем во временный файл и переименовываем — недописанная DLL не останется под настоящим именем.
    let tmp = dir.join(format!("{MPV_DLL}.{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    std::io::copy(&mut entry, &mut f).map_err(|e| format!("не удалось распаковать libmpv: {e}"))?;
    drop(f);
    if std::fs::rename(&tmp, &dll).is_err() {
        // DLL могла появиться параллельно (второй экземпляр программы) — тогда берём её.
        let _ = std::fs::remove_file(&tmp);
        if !std::fs::metadata(&dll).is_ok_and(|m| m.len() == MPV_DLL_SIZE) {
            return Err("не удалось сохранить libmpv".into());
        }
    }
    Ok(dll)
}

fn load() -> Result<Api, String> {
    if std::env::var("PRESENTATOR_VIDEO").is_ok_and(|v| v == "mf") {
        return Err("libmpv отключён (PRESENTATOR_VIDEO=mf)".into());
    }
    let dll = extract()?;
    unsafe {
        let lib = LoadLibraryW(&HSTRING::from(dll.as_os_str())).map_err(|e| format!("не удалось загрузить libmpv: {e}"))?;
        let sym = |name: PCSTR| GetProcAddress(lib, name).ok_or_else(|| format!("в libmpv нет функции {}", name.display()));
        Ok(Api {
            create: std::mem::transmute(sym(s!("mpv_create"))?),
            initialize: std::mem::transmute(sym(s!("mpv_initialize"))?),
            set_option_string: std::mem::transmute(sym(s!("mpv_set_option_string"))?),
            set_property_string: std::mem::transmute(sym(s!("mpv_set_property_string"))?),
            get_property: std::mem::transmute(sym(s!("mpv_get_property"))?),
            command: std::mem::transmute(sym(s!("mpv_command"))?),
            wait_event: std::mem::transmute(sym(s!("mpv_wait_event"))?),
            terminate_destroy: std::mem::transmute(sym(s!("mpv_terminate_destroy"))?),
            error_string: std::mem::transmute(sym(s!("mpv_error_string"))?),
        })
    }
}

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Экземпляр mpv. Уничтожается вместе с объектом.
pub struct Mpv {
    api: &'static Api,
    h: Handle,
}

impl Mpv {
    /// Создаёт mpv с опциями (до инициализации). Err — libmpv недоступен или не запустился.
    pub fn new(options: &[(&str, &str)]) -> Result<Mpv, String> {
        let api = api()?;
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create вернул пустой указатель".into());
        }
        let m = Mpv { api, h };
        for (k, v) in options {
            m.check(unsafe { (api.set_option_string)(h, cstr(k).as_ptr(), cstr(v).as_ptr()) })
                .map_err(|e| format!("опция {k}={v}: {e}"))?;
        }
        m.check(unsafe { (api.initialize)(h) })?;
        Ok(m)
    }

    fn check(&self, code: i32) -> Result<(), String> {
        if code >= 0 {
            return Ok(());
        }
        Err(self.error(code))
    }

    pub fn error(&self, code: i32) -> String {
        unsafe { CStr::from_ptr((self.api.error_string)(code)).to_string_lossy().into_owned() }
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        self.check(unsafe { (self.api.command)(self.h, ptrs.as_ptr()) })
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.check(unsafe { (self.api.set_property_string)(self.h, cstr(name).as_ptr(), cstr(value).as_ptr()) })
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let mut v = 0f64;
        let r = unsafe { (self.api.get_property)(self.h, cstr(name).as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let mut v = 0i32;
        let r = unsafe { (self.api.get_property)(self.h, cstr(name).as_ptr(), FORMAT_FLAG, &mut v as *mut i32 as *mut c_void) };
        (r >= 0).then_some(v != 0)
    }

    /// Все накопившиеся события: (id, код ошибки, для END_FILE — причина и её код ошибки).
    pub fn events(&self) -> Vec<(i32, i32, Option<(i32, i32)>)> {
        let mut out = Vec::new();
        loop {
            let e = unsafe { &*(self.api.wait_event)(self.h, 0.0) };
            if e.event_id == EVENT_NONE {
                return out;
            }
            let end = (e.event_id == EVENT_END_FILE && !e.data.is_null()).then(|| {
                let d = unsafe { &*(e.data as *const EventEndFile) };
                (d.reason, d.error)
            });
            out.push((e.event_id, e.error, end));
            if e.event_id == EVENT_SHUTDOWN {
                return out;
            }
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.h) };
    }
}
