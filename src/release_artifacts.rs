//! Release artifact names and README download links.

pub const LINUX_ARTIFACT: &str = "le3-linux-x86_64.tar.gz";
pub const MACOS_ARTIFACT: &str = "le3-macos-aarch64.dmg";
pub const WINDOWS_ARTIFACT: &str = "le3-windows-x86_64.exe";

pub const RELEASES_BASE: &str = "https://github.com/iffy/LE3/releases/latest/download";

pub fn download_url(artifact: &str) -> String {
    format!("{RELEASES_BASE}/{artifact}")
}

pub const ALL_ARTIFACTS: &[&str] = &[LINUX_ARTIFACT, MACOS_ARTIFACT, WINDOWS_ARTIFACT];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_links_directly_to_each_platform_artifact() {
        let readme = include_str!("../README.md");
        for artifact in ALL_ARTIFACTS {
            let url = download_url(artifact);
            assert!(
                readme.contains(&url),
                "README should link directly to {url}"
            );
        }
    }
}