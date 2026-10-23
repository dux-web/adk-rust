//! Temporary fork-only diagnosis of nested shell commands under the Windows enforcer.
#![cfg(all(windows, feature = "sandbox-windows"))]

use std::ffi::OsString;
use std::time::Duration;

use adk_sandbox::sandbox::SandboxPolicyBuilder;
use adk_sandbox::sandbox::windows::WindowsEnforcer;

#[test]
fn native_child_launch() {
    let root = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(root.path()).unwrap();
    let executable = std::path::PathBuf::from(
        std::env::var_os("ADK_WINDOWS_PROBE_EXE").expect("probe executable"),
    );
    let system_cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join("cmd.exe");
    std::fs::write(workspace.join("localcmd.exe"), std::fs::read(&system_cmd).unwrap()).unwrap();
    let acl = std::process::Command::new("icacls").arg(&system_cmd).output().unwrap();
    println!("HOST CMD ACL: {}", String::from_utf8_lossy(&acl.stdout));
    let mut policy =
        SandboxPolicyBuilder::new().allow_read_write(&workspace).allow_read(&executable).build();
    for key in ["SystemRoot", "PATH", "ComSpec", "PATHEXT", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(key) {
            policy.env.insert(key.into(), value);
        }
    }
    policy.env.insert("ADK_WINDOWS_PROBE_PEER".into(), "1".into());
    let result = WindowsEnforcer::new().run_confined(
        &policy,
        executable.as_os_str(),
        &[],
        &workspace,
        Some(Duration::from_secs(20)),
    );
    let observed = std::fs::read_to_string(workspace.join("peer-outcome.txt"));
    println!("NATIVE PEER: {result:?}; OBSERVED {observed:?}");
    assert!(result.is_ok(), "the enforcer must launch the native peer");
}

#[test]
fn nested_shell_commands() {
    for (name, script) in [
        ("caret", "cmd /c echo child^> child.txt 2> spawn-err.txt"),
        ("plain", "cmd /c echo child> child.txt 2> spawn-err.txt"),
        ("batch", "cmd /c child.cmd 2> spawn-err.txt"),
        ("granted_shell", ".\\localcmd.exe /c child.cmd 2> spawn-err.txt"),
        (
            "granted_descendant",
            "start /b .\\localcmd.exe /c writer.cmd 2> start-err.txt & for /l %j in (1,1,1000000) do @rem",
        ),
        ("absolute_batch", "%SystemRoot%\\System32\\cmd.exe /c child.cmd 2> spawn-err.txt"),
        (
            "start_caret",
            "start /b cmd /c for /l %i in (1,1,1000000) do @echo x^>^> tick.txt 2> start-err.txt & for /l %j in (1,1,1000000) do @rem",
        ),
        (
            "start_batch",
            "start /b cmd /c writer.cmd 2> start-err.txt & for /l %j in (1,1,1000000) do @rem",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(root.path()).unwrap();
        std::fs::write(workspace.join("child.cmd"), "@echo off\r\necho child> child.txt\r\n")
            .unwrap();
        std::fs::write(
            workspace.join("writer.cmd"),
            "@echo off\r\nfor /l %%i in (1,1,1000000) do @echo x>> tick.txt\r\n",
        )
        .unwrap();
        let system_cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        std::fs::write(workspace.join("localcmd.exe"), std::fs::read(&system_cmd).unwrap())
            .unwrap();
        let policy = SandboxPolicyBuilder::new().allow_read_write(&workspace).build();
        let result = WindowsEnforcer::new().run_confined(
            &policy,
            "cmd.exe".as_ref(),
            &[OsString::from("/c"), OsString::from(script)],
            &workspace,
            Some(Duration::from_secs(20)),
        );
        let files: Vec<_> = std::fs::read_dir(&workspace)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let len = std::fs::metadata(&path).unwrap().len();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    len,
                    text.chars().take(300).collect::<String>(),
                )
            })
            .collect();
        println!("PROBE {name}: {result:?}; FILES {files:?}");
        assert!(result.is_ok(), "the enforcer must launch the probe shell");
    }
}
