use std::{env, fs};

use toybox::bundle::windows::{windows_bundle_paths, WindowsBundleFormat};

fn windows_rustc_link_arg(output_path: &std::path::Path) -> String {
    // `cargo:rustc-cdylib-link-arg` forwards this value directly to `link.exe`.
    // Quoting here can become part of the literal argument and break path parsing.
    let normalized = output_path.to_string_lossy().replace('/', "\\");
    format!("/OUT:{normalized}")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let format = match env::var("TOYBOX_ACTIVE_ARTIFACT")
        .unwrap_or_else(|_| {
            if env::var_os("CARGO_FEATURE_VST3").is_some() {
                "vst3".to_owned()
            } else {
                "clap".to_owned()
            }
        })
        .as_str()
    {
        "clap" => WindowsBundleFormat::Clap,
        "vst3" => WindowsBundleFormat::Vst3,
        other => panic!("unsupported TOYBOX_ACTIVE_ARTIFACT value: {other}"),
    };

    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must provide the package version");
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_owned());
    let name = env::var("CARGO_PKG_NAME").expect("Cargo must provide the package name");
    let paths = windows_bundle_paths(format, &name, &version);
    let output_path = paths.output_path(profile == "release");

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("create Windows bundle output directory");
    }

    println!(
        "cargo:rustc-cdylib-link-arg={}",
        windows_rustc_link_arg(output_path)
    );
}

#[cfg(test)]
mod tests {
    use super::windows_rustc_link_arg;
    use std::path::Path;

    #[test]
    fn windows_link_arg_has_no_quotes() {
        let arg = windows_rustc_link_arg(Path::new("dist/plugin.vst3"));

        assert_eq!(arg, r"/OUT:dist\plugin.vst3");
        assert!(!arg.contains('"'));
    }

    #[test]
    fn windows_link_arg_normalizes_windows_like_path_separators() {
        let arg =
            windows_rustc_link_arg(Path::new("D:/workspace/gainsnap/dist/gainsnap-v0.1.0.vst3"));

        assert_eq!(arg, r"/OUT:D:\workspace\gainsnap\dist\gainsnap-v0.1.0.vst3");
        assert!(!arg.contains('"'));
    }
}
