//! launchd user agent (macOS). `launchctl bootstrap gui/$UID` is the modern
//! API; `load` is deprecated.

use std::path::Path;

use crate::error::{Result, RuntimeError};
use crate::service::{LABEL, ServiceSpec};

pub fn render(spec: &ServiceSpec) -> String {
    let exe = spec.exe.display();
    let out = spec.log_out.display();
    let err = spec.log_err.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#
    )
}

fn uid() -> u32 {
    // SAFETY: plain syscall wrapper.
    unsafe { libc::getuid() }
}

fn launchctl(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| RuntimeError::Other(format!("cannot run launchctl: {e}")))?;
    // `bootout` fails when the service isn't loaded — that's fine.
    if !output.status.success() && !args.first().is_some_and(|a| *a == "bootout") {
        return Err(RuntimeError::Other(format!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn reload(path: &Path) -> Result<()> {
    let label_target = format!("gui/{}/{}", uid(), LABEL);
    let _ = launchctl(&["bootout", &label_target]);
    launchctl(&["bootstrap", &format!("gui/{}", uid())])
        .map_err(|e| RuntimeError::Other(format!("bootstrap {}: {e}", path.display())))?;
    Ok(())
}

pub fn unload(path: &Path) -> Result<()> {
    let label_target = format!("gui/{}/{}", uid(), LABEL);
    let _ = launchctl(&["bootout", &label_target]);
    let _ = path;
    Ok(())
}
