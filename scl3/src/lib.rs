use std::path::Path;
use scl_core::version::structs::VersionInfo;


fn from_minecraft_dir(dir: impl AsRef<Path>) -> VersionInfo {
    VersionInfo {
        version_base: dir.as_ref().join("versions").to_string_lossy().to_string(),
        ..Default::default()
    }
}