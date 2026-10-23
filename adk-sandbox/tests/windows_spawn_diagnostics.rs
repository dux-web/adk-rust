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
    let private_root = tempfile::tempdir().unwrap();
    let private = private_root.path().join("owner-only.txt");
    std::fs::write(&private, "private-note").unwrap();
    let user = std::env::var("USERNAME").unwrap();
    let acl = std::process::Command::new("icacls")
        .arg(&private)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:F"))
        .output()
        .unwrap();
    assert!(acl.status.success());
    let readonly = tempfile::tempdir().unwrap();
    let readonly_file = readonly.path().join("readonly.txt");
    std::fs::write(&readonly_file, "readonly-note").unwrap();
    let mut policy = SandboxPolicyBuilder::new()
        .allow_read_write(&workspace)
        .allow_read(&executable)
        .allow_read(readonly.path())
        .build();
    for key in ["SystemRoot", "PATH", "ComSpec", "PATHEXT", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(key) {
            policy.env.insert(key.into(), value);
        }
    }
    policy.env.insert("ADK_WINDOWS_PROBE_PEER".into(), "1".into());
    policy.env.insert("ADK_WINDOWS_PRIVATE_PATH".into(), private.display().to_string());
    policy.env.insert("ADK_WINDOWS_READONLY_PATH".into(), readonly_file.display().to_string());
    let result = WindowsEnforcer::new().run_confined(
        &policy,
        executable.as_os_str(),
        &[],
        &workspace,
        Some(Duration::from_secs(20)),
    );
    let observed = std::fs::read_to_string(workspace.join("peer-outcome.txt"));
    println!("NATIVE PEER: {result:?}; OBSERVED {observed:?}");
    println!("HOST PRIVATE CONTENT: {:?}", std::fs::read_to_string(&private));
    println!("HOST READONLY CONTENT: {:?}", std::fs::read_to_string(&readonly_file));
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

#[test]
fn outside_acl_boundaries() {
    for (name, grant) in [
        ("private", None),
        ("users_write", Some("*S-1-5-32-545:(OI)(CI)M")),
        ("package_read", Some("*S-1-15-2-2:(OI)(CI)RX")),
        ("package_write", Some("*S-1-15-2-2:(OI)(CI)M")),
        ("everyone_write", Some("*S-1-1-0:(OI)(CI)M")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(root.path()).unwrap();
        let workspace = base.join("ws");
        let outside = base.join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let mut acl = std::process::Command::new("icacls");
        acl.arg(&outside).args(["/inheritance:r", "/grant:r"]).arg(format!("{user}:(OI)(CI)F"));
        if let Some(grant) = grant {
            acl.args(["/grant", grant]);
        }
        let output = acl.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        std::fs::write(outside.join("note.txt"), "outside-note").unwrap();
        let policy = SandboxPolicyBuilder::new().allow_read_write(&workspace).build();
        let outcome = WindowsEnforcer::new().run_confined(
            &policy, "cmd.exe".as_ref(),
            &[OsString::from("/c"), OsString::from(
                r"echo control> control.txt & type ..\outside\note.txt > read.txt 2> read-error.txt & echo outside-write> ..\outside\written.txt 2> write-error.txt"
            )], &workspace, Some(Duration::from_secs(20)),
        ).unwrap();
        let control = std::fs::read_to_string(workspace.join("control.txt")).unwrap();
        assert_eq!(control.trim(), "control");
        let read = std::fs::read_to_string(workspace.join("read.txt")).unwrap_or_default();
        let written = outside.join("written.txt").exists();
        println!("ACL BOUNDARY {name}: {outcome:?}; read={read:?}; outside_write={written}");
    }
}
