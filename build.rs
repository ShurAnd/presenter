//! Вшивает в exe значок (assets/icon.ico) и сведения о программе — их показывают
//! Проводник, ярлыки и панель задач.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
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
