fn main() {
    tauri_build::build();

    // Link the native Vosk library on Windows.
    //
    // The `vosk-sys` crate does `#[link(name = "libvosk")]` on Windows but
    // ships no build script, so it relies on `libvosk.lib` being on the
    // linker search path. We vendor the prebuilt Vosk SDK under
    // `src-tauri/vosk/vosk-win64-0.3.45/` and tell the linker where to find
    // it, then copy the runtime DLLs next to the produced executable so
    // `cargo run` / `tauri dev` can load them.
    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR not set");
        let vosk_dir = std::path::Path::new(&manifest_dir)
            .join("vosk")
            .join("vosk-win64-0.3.45");

        if !vosk_dir.join("libvosk.lib").exists() {
            panic!(
                "libvosk.lib not found in `{}`. Download the prebuilt \
                 Vosk SDK (`vosk-win64-0.3.45.zip`) from \
                 https://github.com/alphacep/vosk-api/releases and extract it \
                 so that `libvosk.lib` is at \
                 `src-tauri/vosk/vosk-win64-0.3.45/libvosk.lib`.",
                vosk_dir.display()
            );
        }

        println!(
            "cargo:rustc-link-search=native={}",
            vosk_dir.display()
        );

        // Copy the runtime DLLs next to the built artifact so they load at
        // runtime. `OUT_DIR` is `<target>/<profile>/build/<crate>-<hash>/out`,
        // so the artifact directory is four levels up.
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        let target_dir = std::path::Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .expect("could not resolve target dir from OUT_DIR");

        for dll in [
            "libvosk.dll",
            "libgcc_s_seh-1.dll",
            "libstdc++-6.dll",
            "libwinpthread-1.dll",
        ] {
            let src = vosk_dir.join(dll);
            let dst = target_dir.join(dll);
            if src.exists() {
                if let Err(e) = std::fs::copy(&src, &dst) {
                    println!("cargo:warning=failed to copy {dll}: {e}");
                }
            }
        }

        println!("cargo:rerun-if-changed=vosk/vosk-win64-0.3.45/libvosk.lib");
    }
}