//! Release artifact names and README download links.

pub const GITHUB_REPO: &str = "https://github.com/iffy/BearCAD";

pub const LINUX_ARTIFACT: &str = "bearcad-linux-x86_64.tar.gz";
pub const MACOS_ARTIFACT: &str = "bearcad.dmg";
pub const WINDOWS_ARTIFACT: &str = "bearcad.exe";

pub const RELEASES_BASE: &str = "https://github.com/iffy/BearCAD/releases/latest/download";

pub fn download_url(artifact: &str) -> String {
    format!("{RELEASES_BASE}/{artifact}")
}

pub const ALL_ARTIFACTS: &[&str] = &[LINUX_ARTIFACT, MACOS_ARTIFACT, WINDOWS_ARTIFACT];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_urls_use_github_repo() {
        assert!(RELEASES_BASE.starts_with(GITHUB_REPO));
    }

    #[test]
    fn readme_links_to_github_repo() {
        let readme = include_str!("../README.md");
        assert!(
            readme.contains(GITHUB_REPO),
            "README should link to {GITHUB_REPO}"
        );
    }

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