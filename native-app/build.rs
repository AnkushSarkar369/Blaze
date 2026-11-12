use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=BLAZE_LIBMPV_PATH");
    println!("cargo:rerun-if-changed=assets/blaze.ico");

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/blaze.ico");
        resource.set("ProductName", "Blaze");
        resource.set("FileDescription", "Blaze Video Editor");
        resource.set("OriginalFilename", "Blaze.exe");
        resource.compile().expect("Failed to embed the Blaze Windows application icon");
        return;
    }

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("linux") {
        return;
    }

    let library = locate_libmpv().unwrap_or_else(|| {
        panic!(
            "libmpv runtime library was not found. Install it with your system package manager (Fedora: sudo dnf install mpv-libs)."
        )
    });

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    let linker_name = out_dir.join("libmpv.so");

    if linker_name.exists() {
        fs::remove_file(&linker_name)
            .unwrap_or_else(|error| panic!("Could not replace {}: {error}", linker_name.display()));
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&library, &linker_name).unwrap_or_else(|error| {
        panic!(
            "Could not create libmpv linker symlink {} -> {}: {error}",
            linker_name.display(),
            library.display()
        )
    });

    println!("cargo:rustc-link-search=native={}", out_dir.display());
}

fn locate_libmpv() -> Option<PathBuf> {
    if let Some(path) = env::var_os("BLAZE_LIBMPV_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(output) = Command::new("ldconfig").arg("-p").output() {
        if output.status.success() {
            let listing = String::from_utf8_lossy(&output.stdout);
            for line in listing.lines() {
                if line.contains("libmpv.so.2") {
                    if let Some(path) = line.split_once("=>").map(|(_, path)| Path::new(path.trim())) {
                        if path.is_file() {
                            return Some(path.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    [
        "/lib64/libmpv.so.2",
        "/usr/lib64/libmpv.so.2",
        "/lib/x86_64-linux-gnu/libmpv.so.2",
        "/usr/lib/x86_64-linux-gnu/libmpv.so.2",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}
