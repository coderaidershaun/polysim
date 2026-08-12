//! Position persistence, mandatory separate block.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureConfig {
    #[serde(default = "default_exposure_dir")]
    pub dir: PathBuf,
}

impl Default for ExposureConfig {
    fn default() -> Self {
        ExposureConfig {
            dir: default_exposure_dir(),
        }
    }
}

fn default_exposure_dir() -> PathBuf {
    PathBuf::from("./exposure")
}
