use std::{
    env,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=BLAZE_LIBMPV_PATH");
    println!("cargo:rerun-if-env-changed=MPV_LIBRARY_DIR");
    println!("cargo:rerun-if-changed=assets/blaze.ico");

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => embed_windows_resources(),
        Ok("linux") => configure_linux_mpv(),
        Ok("macos") => configure_macos_mpv(),
        _ => {}
    }
}

// winres is a Windows-only build dependency. The release workflow builds
// natively on Windows; cross-compiling from a non-Windows host does not embed
// the icon because the resource compiler is not available there.
#[cfg(windows)]
fn embed_windows_resources() {
    let mut resource = winres::WindowsResource::new();
    resource.set_icon("assets/blaze.ico");
    resource.set("ProductName", "Blaze");
    resource.set("FileDescription", "Blaze Video Editor");
    resource.set("OriginalFilename", "Blaze.exe");
    resource.compile().expect("Failed to embed the Blaze Windows application icon");
}

#[cfg(not(windows))]
fn embed_windows_resources() {}

fn explicit_library_file() -> Option<PathBuf> {
    env::var_os("BLAZE_LIBMPV_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn library_in_directory(directory: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

fn locate_linux_libmpv() -> Option<PathBuf> {
    if let Some(path) = explicit_library_file() {
        return Some(path);
    }

    if let Some(directory) = env::var_os("MPV_LIBRARY_DIR").map(PathBuf::from) {
        if let Some(path) = library_in_directory(&directory, &["libmpv.so.2", "libmpv.so"]) {
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

fn configure_linux_mpv() {
    let library = locate_linux_libmpv().unwrap_or_else(|| {
        panic!(
            "libmpv runtime library was not found. Install it with your package manager              (Fedora: sudo dnf install mpv-libs), or set BLAZE_LIBMPV_PATH / MPV_LIBRARY_DIR."
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

fn locate_macos_libmpv() -> Option<PathBuf> {
    if let Some(path) = explicit_library_file() {
        return Some(path);
    }

    if let Some(directory) = env::var_os("MPV_LIBRARY_DIR").map(PathBuf::from) {
        if let Some(path) = library_in_directory(
            &directory,
            &["libmpv.dylib", "libmpv.2.dylib", "libmpv.1.dylib"],
        ) {
            return Some(path);
        }
    }

    // Homebrew is the supported development installation. The command is
    // optional so custom and CI installations can use MPV_LIBRARY_DIR instead.
    if let Ok(output) = Command::new("brew").args(["--prefix", "mpv"]).output() {
        if output.status.success() {
            let prefix = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let library_dir = PathBuf::from(prefix).join("lib");
            if let Some(path) = library_in_directory(
                &library_dir,
                &["libmpv.dylib", "libmpv.2.dylib", "libmpv.1.dylib"],
            ) {
                return Some(path);
            }
        }
    }

    [
        "/opt/homebrew/opt/mpv/lib",
        "/usr/local/opt/mpv/lib",
        "/opt/homebrew/lib",
        "/usr/local/lib",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find_map(|directory| {
        library_in_directory(
            &directory,
            &["libmpv.dylib", "libmpv.2.dylib", "libmpv.1.dylib"],
        )
    })
}

fn configure_macos_mpv() {
    let library = locate_macos_libmpv().unwrap_or_else(|| {
        panic!(
            "libmpv was not found. Install it with 'brew install mpv', or set MPV_LIBRARY_DIR              to the directory containing libmpv.dylib."
        )
    });
    let library_dir = library.parent().expect("libmpv library has no parent directory");

    // MPV_NO_PKG_CONFIG is set in .cargo/config.toml, so the native library
    // linker name must be present in a search directory. A linker alias in
    // OUT_DIR supports Homebrew and versioned dylib names consistently.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    let linker_name = out_dir.join("libmpv.dylib");
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
    // Keep development builds able to load Homebrew's libmpv at runtime. The
    // Frameworks rpath is also used by a future signed .app bundle layout.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", library_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
}
