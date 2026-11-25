use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

// The SHA-256 checksums of the prebuilds attached to the release of this
// version, which the publish workflow checks before releasing them.
const CHECKSUMS: &[(&str, &str)] = &[];

fn main() {
    println!("cargo:rerun-if-env-changed=BARE_SIDECAR_DIR");
    println!("cargo:rerun-if-env-changed=BARE_SIDECAR_MIRROR");

    versions();

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let name = executable_name(&os);

    let executable = if env::var_os("DOCS_RS").is_some() {
        let executable = PathBuf::from(env::var("OUT_DIR").unwrap()).join(name);

        fs::write(&executable, []).unwrap();

        executable
    } else {
        let target = target(&os);

        match env::var_os("BARE_SIDECAR_DIR") {
            Some(dir) => local(PathBuf::from(dir), name, &target),
            None => {
                let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
                    .join("prebuilds")
                    .join(&target);

                if dir.is_dir() {
                    local(dir, name, &target)
                } else {
                    download(&target).join(name)
                }
            }
        }
    };

    let digest = digest(&fs::read(&executable).unwrap());

    println!(
        "cargo:rustc-env=BARE_SIDECAR_EXECUTABLE={}",
        executable.display()
    );
    println!("cargo:rustc-env=BARE_SIDECAR_DIGEST={}", &digest[..16]);
}

// Written out as Rust for `versions()`, with a parser that only accepts the
// flat object of strings that the manifest is, to avoid a JSON dependency.
fn versions() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("versions.json");

    println!("cargo:rerun-if-changed={}", manifest.display());

    let json = fs::read_to_string(&manifest).unwrap();

    let body = json
        .trim()
        .strip_prefix('{')
        .and_then(|json| json.strip_suffix('}'))
        .unwrap_or_else(|| panic!("Invalid versions in {}", manifest.display()));

    let string = |value: &str| {
        value
            .trim()
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .filter(|value| !value.contains(['"', '\\']))
            .unwrap_or_else(|| panic!("Invalid versions in {}", manifest.display()))
            .to_owned()
    };

    let pairs: Vec<(String, String)> = body
        .split(',')
        .map(|pair| {
            let (name, version) = pair
                .split_once(':')
                .unwrap_or_else(|| panic!("Invalid versions in {}", manifest.display()));

            (string(name), string(version))
        })
        .collect();

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("versions.rs");

    fs::write(out, format!("&{pairs:?}")).unwrap();
}

fn local(dir: PathBuf, name: &str, target: &str) -> PathBuf {
    let executable = dir.join(name);

    if !executable.is_file() {
        panic!("No Bare prebuild for {target} in {}", dir.display());
    }

    println!("cargo:rerun-if-changed={}", executable.display());

    executable
}

// Download the prebuild from the release of this version, and extract it to
// the output directory unless an earlier build already did.
fn download(target: &str) -> PathBuf {
    let checksum = CHECKSUMS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, checksum)| *checksum)
        .unwrap_or_else(|| panic!("No Bare prebuild for {target}"));

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let archive = out.join(format!("{target}.tar.gz"));
    let dir = out.join("prebuilds").join(target);

    if dir.is_dir() && verify(&archive, checksum) {
        return dir;
    }

    let mirror = env::var("BARE_SIDECAR_MIRROR").unwrap_or_else(|_| {
        "https://github.com/holepunchto/bare-sidecar-rust/releases/download".into()
    });

    let url = format!(
        "{}/v{}/{target}.tar.gz",
        mirror.trim_end_matches('/'),
        env::var("CARGO_PKG_VERSION").unwrap()
    );

    run(Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(&archive)
        .arg(&url));

    if !verify(&archive, checksum) {
        let _ = fs::remove_file(&archive);

        panic!("Checksum mismatch for {url}");
    }

    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&out));

    dir
}

fn verify(archive: &Path, checksum: &str) -> bool {
    fs::read(archive).is_ok_and(|data| digest(&data) == checksum)
}

fn digest(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("Failed to run {command:?}: {err}"));

    if !status.success() {
        panic!("{command:?} failed with {status}");
    }
}

// The target named as by Bare, such as `darwin-arm64`.
fn target(os: &str) -> String {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    let platform = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win32",
        _ => panic!("Unsupported target OS {os}"),
    };

    let arch = match arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => panic!("Unsupported target architecture {arch}"),
    };

    format!("{platform}-{arch}")
}

fn executable_name(os: &str) -> &'static str {
    if os == "windows" { "bare.exe" } else { "bare" }
}
