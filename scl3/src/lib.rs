use scl_core::version::structs::VersionInfo;
use std::path::Path;

fn from_minecraft_dir(dir: impl AsRef<Path>) -> VersionInfo {
    VersionInfo {
        version_base: dir.as_ref().join("versions").to_string_lossy().to_string(),
        ..Default::default()
    }
}
