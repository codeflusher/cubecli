use std::path::Path;

use serde::{Deserialize, Serialize};

/// How to launch the server after provisioning. Persisted verbatim in
/// `.cubecli.meta` and consumed by both the native and compose backends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum LaunchSpec {
    /// `java {jvm_args} -jar {jar} {args}` — vanilla, paper, purpur, fabric.
    #[serde(rename = "jar")]
    Jar {
        jar: String,
        #[serde(default)]
        jvm_args: Vec<String>,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `java @user_jvm_args @argfile {args}` — forge / neoforge after `--installServer`.
    #[serde(rename = "argfile")]
    ArgFile {
        user_jvm_args: String,
        argfile: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Fallback: run a generated shell script (e.g. the core's own `run.sh`).
    #[serde(rename = "script")]
    Script {
        script: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl LaunchSpec {
    /// Full argv including the program (`java_bin`, or `sh` for scripts).
    pub fn argv(&self, java_bin: &Path) -> Vec<String> {
        match self {
            Self::Jar {
                jar,
                jvm_args,
                args,
            } => {
                let mut v = vec![java_bin.display().to_string()];
                v.extend(jvm_args.iter().cloned());
                v.push("-jar".into());
                v.push(jar.clone());
                v.extend(args.iter().cloned());
                v
            }
            Self::ArgFile {
                user_jvm_args,
                argfile,
                args,
            } => {
                let mut v = vec![
                    java_bin.display().to_string(),
                    format!("@{user_jvm_args}"),
                    format!("@{argfile}"),
                ];
                v.extend(args.iter().cloned());
                v
            }
            Self::Script { script, args } => {
                let mut v = vec!["sh".into(), script.clone()];
                v.extend(args.iter().cloned());
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_argv() {
        let spec = LaunchSpec::Jar {
            jar: "paper-1.21.8-132.jar".into(),
            jvm_args: vec!["-Xmx4G".into()],
            args: vec!["nogui".into()],
        };
        assert_eq!(
            spec.argv(Path::new("/jdks/21/bin/java")),
            vec![
                "/jdks/21/bin/java",
                "-Xmx4G",
                "-jar",
                "paper-1.21.8-132.jar",
                "nogui"
            ]
        );
    }

    #[test]
    fn argfile_argv() {
        let spec = LaunchSpec::ArgFile {
            user_jvm_args: "user_jvm_args.txt".into(),
            argfile: "libraries/net/neoforged/neoforge/26.3.0.100/unix_args.txt".into(),
            args: vec!["nogui".into()],
        };
        assert_eq!(
            spec.argv(Path::new("java")),
            vec![
                "java",
                "@user_jvm_args.txt",
                "@libraries/net/neoforged/neoforge/26.3.0.100/unix_args.txt",
                "nogui"
            ]
        );
    }

    #[test]
    fn argfile_serde_tag() {
        let json = serde_json::json!({
            "type": "argfile",
            "user_jvm_args": "user_jvm_args.txt",
            "argfile": "a/b/unix_args.txt",
            "args": ["nogui"]
        });
        let spec: LaunchSpec = serde_json::from_value(json).unwrap();
        assert!(matches!(spec, LaunchSpec::ArgFile { .. }));
    }
}
