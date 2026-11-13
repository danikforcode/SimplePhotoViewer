// Вшивает в exe иконку и манифест (DPI, Common Controls v6), а также DLL
// миниатюр для Проводника (thumbs/), если она уже собрана: build.bat собирает
// её первой. Без DLL программа работает, но без миниатюр в Проводнике.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rustc-check-cfg=cfg(has_thumbs)");
    let _ = embed_resource::compile("assets/app.rc", embed_resource::NONE);

    // OUT_DIR = <target>/<профиль>/build/<пакет>/out → <target>/<профиль>
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let dll = std::env::var("SPV_THUMBS_DLL").map(std::path::PathBuf::from).ok().or_else(|| out.ancestors().nth(3).map(|d| d.join("spv_thumbs.dll")));
    if let Some(dll) = dll {
        println!("cargo:rerun-if-changed={}", dll.display());
        println!("cargo:rerun-if-env-changed=SPV_THUMBS_DLL");
        if dll.is_file() {
            // в exe — сжатой (вдвое меньше); распаковывается при подключении к Проводнику
            let raw = std::fs::read(&dll).expect("DLL миниатюр");
            let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            std::io::Write::write_all(&mut z, &raw).unwrap();
            let packed = out.join("spv_thumbs.dll.deflate");
            std::fs::write(&packed, z.finish().unwrap()).unwrap();
            println!("cargo:rustc-cfg=has_thumbs");
            println!("cargo:rustc-env=SPV_THUMBS_DLL={}", packed.display());
        } else {
            println!("cargo:warning=Нет {} — exe соберётся без миниатюр для Проводника (сначала: cargo build --release -p spv_thumbs)", dll.display());
        }
    }
}
