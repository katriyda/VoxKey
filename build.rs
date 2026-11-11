//! Compiles the Win32 resource block into `VoxKey.exe`.
//!
//! VoxKey ships as a bare, unsigned, single-file executable, and Rust attaches
//! no icon, no `VERSIONINFO` and no manifest by default. A fresh 42 MB PE with
//! no publisher, no version metadata and no icon is exactly the shape antivirus
//! ML classifiers and SmartScreen distrust, so the executable gets the same
//! metadata any commercial Windows application carries:
//!
//! * the application icon (also used by Explorer, the taskbar and Alt-Tab);
//! * a `VERSIONINFO` block, so `文件属性 → 详细信息` shows product name,
//!   description, version and copyright instead of empty fields;
//! * an `asInvoker` / `supportedOS` manifest, which documents the intended
//!   privilege level and stops the OS from reporting a shimmed Windows version.
//!
//! `cargo` does not put `rc.exe` on `PATH`, so the resource compiler is located
//! the same way `.github/workflows/build.yml` locates `dumpbin`: explicit `RC`,
//! then `PATH`, then the Visual Studio / Windows SDK install trees.
//!
//! Set `VOXKEY_SKIP_RC=1` to build without resources (only useful when
//! experimenting on a machine without the MSVC toolchain). Release builds must
//! not use it.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/voxkey.rc");
    println!("cargo:rerun-if-changed=assets/voxkey.manifest");
    println!("cargo:rerun-if-changed=assets/voxkey.ico");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=VOXKEY_SKIP_RC");

    // Only Windows PE targets can carry this resource block.
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os != "windows") {
        return;
    }

    if env::var_os("VOXKEY_SKIP_RC").is_some() {
        println!(
            "cargo:warning=VOXKEY_SKIP_RC is set: building VoxKey.exe without icon, VERSIONINFO and manifest"
        );
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");

    // Work in a scratch directory so the relative file names inside the .rc
    // ("voxkey.ico", "voxkey.manifest") resolve no matter where the build
    // directory lives.
    let res_dir = out_dir.join("win-resources");
    fs::create_dir_all(&res_dir).expect("create the resource output directory");

    let icon_source = manifest_dir.join("assets").join("voxkey.ico");
    fs::copy(&icon_source, res_dir.join("voxkey.ico"))
        .unwrap_or_else(|err| panic!("failed to copy {}: {err}", icon_source.display()));

    write_expanded(
        &manifest_dir.join("assets").join("voxkey.rc"),
        &res_dir.join("voxkey.rc"),
        &version,
    );
    write_expanded(
        &manifest_dir.join("assets").join("voxkey.manifest"),
        &res_dir.join("voxkey.manifest"),
        &version,
    );

    let rc = find_rc().unwrap_or_else(|| {
        panic!(
            "could not find rc.exe (the Windows Resource Compiler).\n\
             Install the Visual Studio \"Desktop development with C++\" workload (as the \
             GitHub Actions windows-2022 runner does), or point RC at rc.exe, e.g.\n\
             set RC=C:\\Program Files\\Microsoft Visual Studio\\2022\\Enterprise\\VC\\Tools\\MSVC\\<version>\\bin\\Hostx64\\x64\\rc.exe\n\
             To build without resources, set VOXKEY_SKIP_RC=1 (release builds must not do this)."
        )
    });

    let output = Command::new(&rc)
        .current_dir(&res_dir)
        .arg("/nologo")
        .arg("/fo")
        .arg("voxkey.res")
        .arg("voxkey.rc")
        .output()
        .unwrap_or_else(|err| panic!("failed to run {}: {err}", rc.display()));

    if !output.status.success() {
        panic!(
            "{} failed with {}.\nstdout:\n{}\nstderr:\n{}",
            rc.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    // Only link the resource into real binaries. Test harnesses and build
    // scripts stay untouched.
    let res = res_dir.join("voxkey.res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// Substitutes the version placeholders in a resource template into `out`.
fn write_expanded(source: &Path, out: &Path, version: &str) {
    let template = fs::read_to_string(source)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", source.display()));

    let parts = version_parts(version);
    let comma = parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let dot4 = parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(".");

    let expanded = template
        .replace("@VERSION_STR@", version)
        .replace("@VERSION_COMMA@", &comma)
        .replace("@VERSION_DOT4@", &dot4);

    // A typo in a placeholder would otherwise leak "@VERSION_..." into
    // Explorer's Details tab and into the manifest, where it would make the
    // whole resource block invalid.
    if expanded.contains("@VERSION_") {
        panic!(
            "{} still contains an unsubstituted @VERSION_ placeholder; \
             build.rs and the template are out of sync",
            source.display()
        );
    }

    fs::write(out, expanded)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", out.display()));
}

/// `0.2.0` -> `[0, 2, 0, 0]`; pre-release/build suffixes stop at the first
/// non-digit character, which is what `VERSIONINFO`'s numeric fields require.
fn version_parts(version: &str) -> [u16; 4] {
    let mut parts = [0u16; 4];
    for (index, raw) in version.split('.').take(4).enumerate() {
        let digits: String = raw.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        parts[index] = digits.parse().unwrap_or(0);
    }
    parts
}

fn find_rc() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("RC") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
        println!(
            "cargo:warning=RC={} is not a file; ignoring it",
            path.display()
        );
    }

    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // Visual Studio 2022/2019:
    //   <ProgramFiles>\Microsoft Visual Studio\<year>\<edition>\VC\Tools\MSVC\<version>\bin\Hostx64\x64\rc.exe
    for year in ["2022", "2019"] {
        for base in program_files() {
            let vs_root = base.join("Microsoft Visual Studio").join(year);
            if let Some(rc) = newest_msvc_rc(&vs_root) {
                return Some(rc);
            }
        }
    }

    // Windows SDK fallback:
    //   <ProgramFiles(x86)>\Windows Kits\10\bin\<version>\x64\rc.exe
    for base in program_files() {
        let sdk_root = base.join("Windows Kits").join("10").join("bin");
        if let Some(rc) = newest_sdk_rc(&sdk_root) {
            return Some(rc);
        }
    }

    None
}

fn newest_msvc_rc(vs_root: &Path) -> Option<PathBuf> {
    let mut best: Option<(String, PathBuf)> = None;
    for edition in fs::read_dir(vs_root).ok()?.flatten() {
        let msvc = edition.path().join("VC").join("Tools").join("MSVC");
        let entries = match fs::read_dir(&msvc) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for toolset in entries.flatten() {
            let rc = toolset
                .path()
                .join("bin")
                .join("Hostx64")
                .join("x64")
                .join("rc.exe");
            if !rc.is_file() {
                continue;
            }
            let key = toolset.file_name().to_string_lossy().into_owned();
            if best
                .as_ref()
                .is_none_or(|(best_key, _)| key.as_str() > best_key.as_str())
            {
                best = Some((key, rc));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn newest_sdk_rc(sdk_root: &Path) -> Option<PathBuf> {
    let mut best: Option<(String, PathBuf)> = None;
    for version in fs::read_dir(sdk_root).ok()?.flatten() {
        let rc = version.path().join("x64").join("rc.exe");
        if !rc.is_file() {
            continue;
        }
        let key = version.file_name().to_string_lossy().into_owned();
        if best
            .as_ref()
            .is_none_or(|(best_key, _)| key.as_str() > best_key.as_str())
        {
            best = Some((key, rc));
        }
    }
    best.map(|(_, path)| path)
}

fn program_files() -> Vec<PathBuf> {
    ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|var| env::var_os(var).map(PathBuf::from))
        .collect()
}
