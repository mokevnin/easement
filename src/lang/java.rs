//! Java beyond its index: what changes what scip-java writes.

use super::language::starts_with_any;

/// scip-java runs Maven or Gradle with the JDK of `JAVA_HOME`: `MAVEN_ARGS`
/// can skip the build, `ORG_GRADLE_PROJECT_*` sets project properties. Not
/// `JAVA_*` as a prefix: CI runners name every JDK they carry
/// (`JAVA_HOME_17_X64`), used or not.
pub fn env(name: &str) -> bool {
    matches!(
        name,
        "JAVA_HOME"
            | "JAVA_OPTS"
            | "JAVA_TOOL_OPTIONS"
            | "JDK_JAVA_OPTIONS"
            | "_JAVA_OPTIONS"
            | "GRADLE_OPTS"
            | "GRADLE_USER_HOME"
    ) || starts_with_any(name, &["MAVEN_", "ORG_GRADLE_PROJECT_"])
}

/// Whether a module is Java's standard library: a package of the JDK.
pub fn std(module: &str) -> bool {
    jdk(module)
}

/// The packages of the JDK: `java.util`, `javax.crypto`, `org.w3c.dom`.
pub fn jdk(module: &str) -> bool {
    [
        "java.",
        "javax.",
        "jdk.",
        "sun.",
        "com.sun.",
        "org.w3c.",
        "org.xml.sax",
        "org.ietf.jgss",
    ]
    .iter()
    .any(|prefix| module.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_standard_library_is_the_jdk() {
        use crate::lang::registry::is_std;
        assert!(is_std("A.java", "java.util.List"));
        assert!(is_std("A.java", "org.w3c.dom.Node"));
        assert!(!is_std("A.java", "com.google.common.Lists"));
        assert!(!is_std("A.java", "org.apache.commons.Lang"));
    }

    use super::*;

    #[test]
    fn the_variables_of_the_jdk_maven_and_gradle_are_keyed_but_other_jdks() {
        for name in [
            "JAVA_HOME",
            "MAVEN_ARGS",
            "ORG_GRADLE_PROJECT_flavor",
            "GRADLE_USER_HOME",
        ] {
            assert!(env(name), "{name}");
        }
        assert!(!env("JAVA_HOME_17_X64"));
    }
}
