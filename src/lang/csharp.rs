//! C# beyond its index: what changes what scip-dotnet writes.

use super::registry::starts_with_any;

/// MSBuild reads every environment variable as a property, by a name of any
/// case (`DefineConstants=X` turns on `#if X`). Keying all of them would miss on
/// every CI run, so the key holds the SDK's own and the properties that decide
/// what is compiled; a project that reads another variable needs `--no-cache`
/// after changing it.
pub fn env(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    starts_with_any(&name, &["DOTNET_", "MSBUILD", "NUGET_"])
        || matches!(
            name.as_str(),
            "DEFINECONSTANTS"
                | "CONFIGURATION"
                | "PLATFORM"
                | "TARGETFRAMEWORK"
                | "TARGETFRAMEWORKS"
                | "RUNTIMEIDENTIFIER"
                | "RUNTIMEIDENTIFIERS"
                | "LANGVERSION"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variables_of_the_sdk_and_the_compiled_properties_are_keyed() {
        for name in ["DefineConstants", "MSBuildSDKsPath", "DOTNET_ROOT"] {
            assert!(env(name), "{name}");
        }
        assert!(!env("GITHUB_RUN_ID"));
    }
}
