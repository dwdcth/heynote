// Build script. (probe v3: still diagnosing core-permission resolution)
fn main() {
    let mut found_core_window = false;
    for (k, v) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.contains("CORE:WINDOW") || k.contains("CORE:WINDOW__") {
            found_core_window = true;
            let vpath = std::path::PathBuf::from(v.to_string_lossy().to_string());
            println!(
                "cargo:warning=V3 core:window env present, file exists={}",
                vpath.exists()
            );
        }
    }
    if !found_core_window {
        println!("cargo:warning=V3 core:window env ABSENT");
    }
    tauri_build::build()
}
