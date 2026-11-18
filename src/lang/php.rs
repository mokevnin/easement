//! PHP beyond its index: what scip-php needs before it runs.

use std::path::Path;

/// `scip-php` reads the dependencies' code from `vendor`, through the autoloader.
pub fn needs(_root: &Path, dir: &Path, shown: &str) -> anyhow::Result<()> {
    if !dir.join("composer.lock").is_file() || !dir.join("vendor/autoload.php").is_file() {
        anyhow::bail!(
            "dependencies are not installed in {shown}: run `composer install` (scip-php needs composer.lock and vendor/)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_has_to_have_installed_the_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let err = needs(root, root, ".").unwrap_err();
        assert!(err.to_string().contains("composer install"), "{err}");
        std::fs::write(root.join("composer.lock"), "{}").unwrap();
        assert!(needs(root, root, ".").is_err());
        std::fs::create_dir(root.join("vendor")).unwrap();
        std::fs::write(root.join("vendor/autoload.php"), "<?php\n").unwrap();
        needs(root, root, ".").unwrap();
    }
}
