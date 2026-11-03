//! Вшивает в exe значок (assets/icon.ico) и сведения о программе — их показывают
//! Проводник, ярлыки и панель задач.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=vendor/pdfium/pdfium.dll");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
            panic!("Встроенная PDFium 7881 предназначена для Windows x64; другая архитектура не поддерживается этой сборкой");
        }
        if !std::path::Path::new("vendor/pdfium/pdfium.dll").is_file() {
            panic!("Встроенный PDFium не найден: запустите scripts\\build_release.ps1, чтобы автоматически загрузить PDFium 7881 и собрать один presentator.exe");
        }
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Presentator")
            .set("FileDescription", "Presentator — показ презентаций и видео на видеостене")
            .set("LegalCopyright", "");
        if let Err(e) = res.compile() {
            // Без Windows SDK (rc.exe) собираемся без значка, а не падаем.
            println!("cargo:warning=значок не вшит: {e}");
        }
    }
}
